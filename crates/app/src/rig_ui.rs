//! Rig controller: move the rig by hand. A pad for pan and tilt, spring-return
//! sliders for zoom, focus and elevation, and the same from the keyboard.
//! Velocities go to the server 30 times a second while a control is held; the
//! server stops the rig if they stop arriving.

use crate::link::Cmd;
use crate::panels::{note, section};
use crate::session::Session;
use crate::theme::*;
use egui::{pos2, vec2, Align2, FontId, Key, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
use ptz_core::api::{Jog, StorePreset};
use ptz_core::pose::Axis;
use ptz_core::transport::{Output, Run};
use std::time::{Duration, Instant};

/// How often a held control repeats its velocity. The server's watchdog is 250 ms.
pub const SEND_EVERY: Duration = Duration::from_millis(33);
const DEADZONE: f64 = 0.08;
const SLIDER_AXES: [Axis; 3] = [Axis::Zoom, Axis::Focus, Axis::Elevation];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Pad,
    Slider(Axis),
}

pub struct Controller {
    /// Full deflection drives an axis at this fraction of its top speed.
    pub speed: f64,
    held: Option<Control>,
    /// The pointer was already down on the controller last frame.
    pressed: bool,
    last_sent: Option<Instant>,
    moving: bool,
    /// The controller has keyboard focus, so the arrow keys drive the rig, not the playhead.
    pub keyboard: bool,
    preset_name: String,
    /// Where the pad and sliders were drawn last frame, for the UI tests.
    pub probe: Option<Surface>,
}

impl Default for Controller {
    fn default() -> Self {
        Controller {
            speed: 0.25,
            held: None,
            pressed: false,
            last_sent: None,
            moving: false,
            keyboard: false,
            preset_name: String::new(),
            probe: None,
        }
    }
}

impl Controller {
    /// What to send now, if anything: the velocity again every `SEND_EVERY`
    /// while it is not zero, and a single stop when it returns to zero.
    pub fn due(&mut self, want: Jog, now: Instant) -> Option<Jog> {
        if want.is_zero() {
            if self.moving {
                self.moving = false;
                self.last_sent = Some(now);
                return Some(Jog::default());
            }
            return None;
        }
        if self.moving && self.last_sent.is_some_and(|t| now.duration_since(t) < SEND_EVERY) {
            return None;
        }
        self.moving = true;
        self.last_sent = Some(now);
        Some(want)
    }
}

/// Where the controls sit inside the controller's rectangle.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Surface {
    pub pad: Rect,
    pub sliders: [(Axis, Rect); 3],
}

pub fn surface(rect: Rect) -> Surface {
    let side = rect.height().min(rect.width() * 0.46).clamp(60.0, 180.0);
    let pad = Rect::from_min_size(rect.min, vec2(side, side));
    let left = pad.right() + 18.0;
    let right = (rect.right() - 6.0).max(left + 40.0);
    let row = side / 3.0;
    let track = |i: usize| {
        let top = pad.top() + row * i as f32;
        Rect::from_min_max(pos2(left, top + row * 0.52), pos2(right, top + row - 5.0))
    };
    Surface { pad, sliders: [(SLIDER_AXES[0], track(0)), (SLIDER_AXES[1], track(1)), (SLIDER_AXES[2], track(2))] }
}

impl Surface {
    pub fn control_at(&self, pos: Pos2) -> Option<Control> {
        if self.pad.contains(pos) {
            return Some(Control::Pad);
        }
        self.sliders.iter().find(|(_, r)| r.expand2(vec2(4.0, 10.0)).contains(pos)).map(|(a, _)| Control::Slider(*a))
    }

    /// The stick position a pointer at `pos` gives the held control: each axis from -1 to 1.
    pub fn deflection(&self, control: Control, pos: Pos2) -> Jog {
        let shape = |v: f32| {
            let v = v.clamp(-1.0, 1.0) as f64;
            if v.abs() < DEADZONE {
                0.0
            } else {
                v.signum() * (v.abs() - DEADZONE) / (1.0 - DEADZONE)
            }
        };
        let mut jog = Jog::default();
        match control {
            Control::Pad => {
                let half = self.pad.width() / 2.0 - 8.0;
                let d = pos - self.pad.center();
                jog.pan = shape(d.x / half);
                jog.tilt = shape(-d.y / half);
            }
            Control::Slider(axis) => {
                if let Some((_, r)) = self.sliders.iter().find(|(a, _)| *a == axis) {
                    let v = shape((pos.x - r.center().x) / (r.width() / 2.0 - 6.0));
                    match axis {
                        Axis::Zoom => jog.zoom = v,
                        Axis::Focus => jog.focus = v,
                        Axis::Elevation => jog.elevation = v,
                        Axis::Pan => jog.pan = v,
                        Axis::Tilt => jog.tilt = v,
                    }
                }
            }
        }
        jog
    }
}

/// The stick the held keys give: arrows pan and tilt, `[` `]` zoom, `,` `.` elevation.
pub fn key_jog(down: impl Fn(Key) -> bool) -> Jog {
    let pair = |minus: Key, plus: Key| (down(plus) as i32 - down(minus) as i32) as f64;
    Jog {
        pan: pair(Key::ArrowLeft, Key::ArrowRight),
        tilt: pair(Key::ArrowDown, Key::ArrowUp),
        zoom: pair(Key::OpenBracket, Key::CloseBracket),
        focus: 0.0,
        elevation: pair(Key::Comma, Key::Period),
    }
}

fn combine(a: Jog, b: Jog, scale: f64) -> Jog {
    let f = |x: f64, y: f64| (x + y).clamp(-1.0, 1.0) * scale;
    Jog {
        pan: f(a.pan, b.pan),
        tilt: f(a.tilt, b.tilt),
        zoom: f(a.zoom, b.zoom),
        focus: f(a.focus, b.focus),
        elevation: f(a.elevation, b.elevation),
    }
}

/// Why the controller cannot move the rig now. The server applies the same rules.
pub fn blocked(s: &Session) -> Option<String> {
    let t = s.status.transport;
    if s.lost.is_some() {
        Some("The server is not reachable".into())
    } else if !s.status.you_hold_lease {
        Some("Take the edit lease to drive the rig".into())
    } else if t.estop {
        Some("Reset E-Stop to continue".into())
    } else if t.output != Output::Rehearse {
        Some("Disarm to drive the rig by hand".into())
    } else if t.run != Run::Stopped && !t.record {
        Some("Stop to position the rig by hand, or turn Record on to capture the move".into())
    } else {
        None
    }
}

/// During a pass only the record-enabled axes answer to the controller.
fn driven(s: &Session, axis: Axis) -> bool {
    let t = s.status.transport;
    !(t.record && t.run != Run::Stopped) || s.record_axes_shown().contains(&axis)
}

fn paint_surface(ui: &Ui, s: &Session, surface: &Surface, stick: Jog, focused: bool, dim: bool) {
    let p = ui.painter();
    let ink = |axis: Axis| if dim || !driven(s, axis) { MUTED } else { axis_color(axis) };
    let pad = surface.pad;
    p.rect(
        pad,
        8.0,
        PANEL_DARK,
        Stroke::new(if focused { 1.5 } else { 1.0 }, if focused { BLUE } else { LINE }),
        StrokeKind::Inside,
    );
    let c = pad.center();
    p.hline(pad.shrink(8.0).x_range(), c.y, Stroke::new(1.0, LINE));
    p.vline(c.x, pad.shrink(8.0).y_range(), Stroke::new(1.0, LINE));
    p.text(pad.right_center() + vec2(-8.0, -9.0), Align2::RIGHT_CENTER, "Pan", FontId::proportional(10.5), ink(Axis::Pan));
    p.text(pad.center_top() + vec2(6.0, 8.0), Align2::LEFT_TOP, "Tilt", FontId::proportional(10.5), ink(Axis::Tilt));
    let half = pad.width() / 2.0 - 8.0;
    let unshape = |v: f64| if v == 0.0 { 0.0 } else { (v.abs() * (1.0 - DEADZONE) + DEADZONE) * v.signum() };
    let knob = c + vec2(unshape(stick.pan) as f32 * half, -unshape(stick.tilt) as f32 * half);
    let live = stick.pan != 0.0 || stick.tilt != 0.0;
    if live {
        p.line_segment([c, knob], Stroke::new(2.0, BLUE));
    }
    p.circle(knob, 9.0, if live { BLUE } else { PANEL }, Stroke::new(1.5, if dim { MUTED } else { OFF_WHITE }));

    for (axis, r) in surface.sliders {
        let v = stick.get(axis);
        p.text(r.left_top() + vec2(0.0, -4.0), Align2::LEFT_BOTTOM, axis.label(), FontId::proportional(11.5), ink(axis));
        p.text(
            r.right_top() + vec2(0.0, -4.0),
            Align2::RIGHT_BOTTOM,
            axis.format(s.cam_status().rig_pose.get(axis)),
            FontId::monospace(11.0),
            if dim { MUTED } else { OFF_WHITE },
        );
        let track = Rect::from_center_size(r.center(), vec2(r.width(), 6.0));
        p.rect(track, 3.0, PANEL_DARK, Stroke::new(1.0, LINE), StrokeKind::Inside);
        p.vline(r.center().x, r.y_range(), Stroke::new(1.0, LINE));
        let x = r.center().x + unshape(v) as f32 * (r.width() / 2.0 - 6.0);
        if v != 0.0 {
            p.line_segment([pos2(r.center().x, r.center().y), pos2(x, r.center().y)], Stroke::new(3.0, BLUE));
        }
        p.circle(
            pos2(x, r.center().y),
            7.0,
            if v != 0.0 { BLUE } else { PANEL },
            Stroke::new(1.5, if dim { MUTED } else { OFF_WHITE }),
        );
    }
}

fn controller(ui: &mut Ui, s: &mut Session) {
    let why = blocked(s);
    let width = ui.available_width().max(200.0);
    let height = (width * 0.46).clamp(110.0, 180.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let surface = surface(rect);
    s.rig.probe = Some(surface);

    let pointer = ui.input(|i| i.pointer.interact_pos());
    // A control answers from the moment it is pressed, without waiting for the pointer to move.
    let down = resp.is_pointer_button_down_on();
    if down && s.rig.held.is_none() && !s.rig.pressed {
        resp.request_focus();
        s.rig.held = ui.input(|i| i.pointer.press_origin()).or(pointer).and_then(|p| surface.control_at(p));
        if let Some(why) = &why {
            s.toast(why.clone(), true);
        }
    }
    s.rig.pressed = down;
    if !down {
        // Spring return: letting go stops the axis.
        s.rig.held = None;
    }
    let focused = resp.has_focus();
    s.rig.keyboard = focused;
    if focused {
        // Keep the arrow keys for the rig instead of moving focus to another widget.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false },
            )
        });
    }

    let from_pointer = match (s.rig.held, pointer) {
        (Some(control), Some(p)) => surface.deflection(control, p),
        _ => Jog::default(),
    };
    let from_keys = if focused { ui.input(|i| key_jog(|k| i.key_down(k))) } else { Jog::default() };
    let stick = combine(from_pointer, from_keys, 1.0);
    let want = if why.is_some() { Jog::default() } else { combine(from_pointer, from_keys, s.rig.speed) };
    if let Some(jog) = s.rig.due(want, Instant::now()) {
        s.link.send(Cmd::Jog(s.cam_id().to_string(), jog));
    }
    if !want.is_zero() {
        ui.ctx().request_repaint_after(SEND_EVERY);
    }

    paint_surface(ui, s, &surface, if why.is_some() { Jog::default() } else { stick }, focused, why.is_some());
    if let Some(control) = resp.hover_pos().and_then(|p| surface.control_at(p)) {
        if why.is_none() {
            ui.ctx().set_cursor_icon(if matches!(control, Control::Pad) {
                egui::CursorIcon::Move
            } else {
                egui::CursorIcon::ResizeHorizontal
            });
        }
    }

    match &why {
        Some(why) => {
            ui.label(RichText::new(why).color(AMBER).small());
        }
        None if focused => {
            ui.label(RichText::new("Keys: arrows pan and tilt,  [ ]  zoom,  , .  elevation").color(BLUE).small());
        }
        None => {
            ui.label(RichText::new("Drag the pad or a slider. Click here to use the keys.").color(MUTED).small());
        }
    }
}

/// What the live loop is doing: how far each axis is from the timeline, and why output was cut.
fn live_state(ui: &mut Ui, s: &Session) {
    let tel = &s.status.telemetry;
    if let Some(fault) = &tel.fault {
        ui.label(RichText::new(fault).color(RED).strong());
    }
    if tel.halting {
        ui.label(RichText::new("Stopping: stop is being repeated until the rig stands still").color(RED));
    }
    if let Some(progress) = &tel.preset_sync {
        ui.label(RichText::new(progress).color(AMBER));
    }
    if !s.status.hardware_live {
        return;
    }
    section(ui, "Tracking");
    let tol = s.cam().rig.tolerance;
    let error = s.cam_status().error;
    egui::Grid::new("rig_tracking").num_columns(3).spacing([12.0, 5.0]).show(ui, |ui| {
        for axis in Axis::ALL {
            let error = error.get(axis);
            let share = (error.abs() / tol.get(axis).max(1e-9)) as f32;
            let ink = if share > 1.0 {
                RED
            } else if share > 0.6 {
                AMBER
            } else {
                GREEN
            };
            ui.label(RichText::new(axis.label()).color(axis_color(axis)));
            ui.label(RichText::new(format!("{error:+.2}")).monospace().color(ink))
                .on_hover_text(format!("Rig minus timeline. A fault is raised beyond {}", axis.format(tol.get(axis))));
            let (bar, _) = ui.allocate_exact_size(vec2(90.0, 6.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(bar, 3.0, PANEL_DARK);
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * share.min(1.0), bar.height()));
            p.rect_filled(fill, 3.0, ink);
            ui.end_row();
        }
    });
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            if s.project.cameras.len() > 1 {
                let i = s.cam_index();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!(" {} ", i + 1)).strong().color(BASE).background_color(camera_color(i)));
                    ui.label(RichText::new(&s.cam().name).strong());
                    ui.label(
                        RichText::new("The controller, the readout and the stored pose are this camera's.").color(MUTED).small(),
                    );
                });
            }
            if s.cam_status().adapters.is_empty() {
                note(ui, "The server has not reported a rig.");
            }
            for a in &s.cam_status().adapters {
                ui.horizontal(|ui| {
                    let (dot, word) = match (a.connected, a.calibrated) {
                        (false, _) => (RED, "not answering".to_string()),
                        (true, Some(false)) => (AMBER, "not calibrated".to_string()),
                        (true, _) => match a.round_trip_ms {
                            Some(ms) => (GREEN, format!("answers in {ms:.1} ms")),
                            None => (GREEN, "connected".to_string()),
                        },
                    };
                    ui.label(RichText::new("●").color(dot));
                    ui.label(RichText::new(&a.name).strong());
                    ui.label(RichText::new(word).color(MUTED));
                })
                .response
                .on_hover_text(&a.detail);
                if !a.connected && !a.detail.is_empty() {
                    ui.label(RichText::new(&a.detail).color(AMBER).small());
                }
            }
            live_state(ui, s);

            section(ui, "Move by hand");
            controller(ui, s);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Top speed").color(MUTED));
                let mut percent = s.rig.speed * 100.0;
                let slider = egui::Slider::new(&mut percent, 2.0..=100.0).suffix("%").fixed_decimals(0);
                if ui.add(slider).on_hover_text("How fast full deflection moves each axis, as a share of its limit").changed() {
                    s.rig.speed = percent / 100.0;
                }
            });

            section(ui, "Record writes");
            let recording = s.status.transport.record;
            ui.horizontal_wrapped(|ui| {
                for axis in Axis::ALL {
                    let on = s.record_axes_shown().contains(&axis);
                    let text = RichText::new(axis.label()).small().color(if on { OFF_WHITE } else { MUTED });
                    let button = egui::Button::new(text).small().fill(if on { with_alpha(RED, 170) } else { PANEL_DARK });
                    if ui.add(button).on_hover_text(format!("Record {} when Record is on", axis.label())).clicked() {
                        s.toggle_record_axis(axis);
                    }
                }
            });
            let state = s.status.record.clone();
            if state.active {
                ui.label(RichText::new(format!("Recording: {} frames", state.frames)).color(RED).small());
            } else if recording {
                ui.label(RichText::new("Record is on. Play starts the pass.").color(RED).small());
            } else {
                ui.label(RichText::new("Turn Record on, then Play, and move these axes.").color(MUTED).small());
            }

            section(ui, "Position");
            egui::Grid::new("rig_pose").num_columns(3).spacing([12.0, 5.0]).show(ui, |ui| {
                for axis in Axis::ALL {
                    let lim = s.cam().axes.get(axis);
                    let value = s.cam_status().rig_pose.get(axis);
                    ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                    ui.label(RichText::new(axis.format(value)).monospace());
                    let (bar, _) = ui.allocate_exact_size(vec2(90.0, 6.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(bar, 3.0, PANEL_DARK);
                    let t = ((value - lim.min) / (lim.max - lim.min).max(1e-9)).clamp(0.0, 1.0) as f32;
                    p.circle_filled(pos2(bar.left() + 3.0 + t * (bar.width() - 6.0), bar.center().y), 3.5, axis_color(axis));
                    ui.end_row();
                }
            });

            section(ui, "Store preset");
            ui.horizontal(|ui| {
                if s.rig.preset_name.is_empty() {
                    s.rig.preset_name = format!("Preset {}", s.presets.len() + 1);
                }
                ui.add(egui::TextEdit::singleline(&mut s.rig.preset_name).desired_width(140.0));
                let can = s.status.you_hold_lease && s.lost.is_none() && !s.rig.preset_name.trim().is_empty();
                let r = ui
                    .add_enabled(can, egui::Button::new("Store"))
                    .on_hover_text("Save where the rig is now as a preset")
                    .on_disabled_hover_text("Take the edit lease to store presets");
                if r.clicked() {
                    let name = s.rig.preset_name.trim().to_string();
                    let (pose, camera) = (Some(s.cam_status().rig_pose), Some(s.cam_id().to_string()));
                    s.link.send(Cmd::StorePreset(StorePreset { name, pose, camera }));
                    s.rig.preset_name.clear();
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Surface {
        surface(Rect::from_min_size(pos2(10.0, 20.0), vec2(340.0, 156.0)))
    }

    #[test]
    fn the_pad_gives_pan_and_tilt_and_the_centre_is_still() {
        let s = area();
        assert_eq!(s.pad.width(), s.pad.height());
        let c = s.pad.center();
        assert_eq!(s.control_at(c), Some(Control::Pad));
        assert!(s.deflection(Control::Pad, c + vec2(3.0, -2.0)).is_zero(), "a hand resting near the centre does not creep");
        let right = s.deflection(Control::Pad, pos2(s.pad.right() - 8.0, c.y));
        assert_eq!((right.pan, right.tilt), (1.0, 0.0));
        let up_left = s.deflection(Control::Pad, s.pad.left_top());
        assert_eq!((up_left.pan, up_left.tilt), (-1.0, 1.0), "up on the pad tilts up");
        let half = s.deflection(Control::Pad, c + vec2((s.pad.width() / 2.0 - 8.0) * 0.54, 0.0));
        assert!((half.pan - 0.5).abs() < 1e-6, "{}", half.pan);
        // Dragging far outside the pad holds full deflection, it does not let go.
        assert_eq!(s.deflection(Control::Pad, c + vec2(900.0, 0.0)).pan, 1.0);
        assert_eq!((right.zoom, right.focus, right.elevation), (0.0, 0.0, 0.0));
    }

    #[test]
    fn each_slider_drives_its_own_axis() {
        let s = area();
        for (axis, r) in s.sliders {
            assert_eq!(s.control_at(r.center()), Some(Control::Slider(axis)));
            let full = s.deflection(Control::Slider(axis), pos2(r.right(), r.center().y));
            let back = s.deflection(Control::Slider(axis), pos2(r.left(), r.center().y));
            assert_eq!((full.get(axis), back.get(axis)), (1.0, -1.0));
            for other in Axis::ALL.into_iter().filter(|a| *a != axis) {
                assert_eq!(full.get(other), 0.0);
            }
            assert!(s.deflection(Control::Slider(axis), r.center()).is_zero());
            assert!(r.left() > s.pad.right());
        }
        assert_eq!(s.control_at(pos2(s.pad.right() + 6.0, s.pad.bottom() + 30.0)), None);
    }

    #[test]
    fn keys_map_to_axes() {
        let held = |keys: &'static [Key]| key_jog(move |k| keys.contains(&k));
        assert_eq!(held(&[Key::ArrowRight]).pan, 1.0);
        assert_eq!(held(&[Key::ArrowLeft, Key::ArrowUp]), Jog { pan: -1.0, tilt: 1.0, ..Jog::default() });
        assert_eq!(held(&[Key::CloseBracket]).zoom, 1.0);
        assert_eq!(held(&[Key::OpenBracket]).zoom, -1.0);
        assert_eq!(held(&[Key::Comma]).elevation, -1.0);
        assert_eq!(held(&[Key::Period]).elevation, 1.0);
        assert!(held(&[Key::ArrowLeft, Key::ArrowRight]).is_zero(), "opposite keys cancel");
        let fast = combine(held(&[Key::ArrowRight]), Jog { pan: 0.8, zoom: -0.5, ..Jog::default() }, 0.25);
        assert_eq!((fast.pan, fast.zoom), (0.25, -0.125));
    }

    #[test]
    fn a_held_control_repeats_at_30_hz_and_release_sends_one_stop() {
        let mut c = Controller::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let jog = Jog { pan: 0.2, ..Jog::default() };
        assert_eq!(c.due(Jog::default(), ms(0)), None, "nothing is sent while nothing is held");
        // 60 frames a second for one second: about 30 messages, never more than 250 ms apart.
        let mut sent = Vec::new();
        for frame in 0..60u64 {
            if c.due(jog, ms(10 + frame * 1000 / 60)).is_some() {
                sent.push(10 + frame * 1000 / 60);
            }
        }
        assert_eq!(sent[0], 10, "the first movement goes out at once");
        assert!((28..=31).contains(&sent.len()), "{} messages in a second", sent.len());
        assert!(sent.windows(2).all(|w| w[1] - w[0] <= 50), "gaps stay far inside the server's 250 ms watchdog");
        assert_eq!(c.due(Jog::default(), ms(1020)), Some(Jog::default()), "release stops the rig at once");
        assert_eq!(c.due(Jog::default(), ms(1040)), None, "and only once");
        assert_eq!(c.due(jog, ms(1041)), Some(jog), "a new press does not wait out the interval");
    }
}
