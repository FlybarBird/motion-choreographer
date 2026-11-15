//! Rig controller: move the rig by hand. A pad for pan and tilt, spring-return
//! sliders for zoom, focus and elevation, and the same from the keyboard.
//! Velocities go to the server 30 times a second while a control is held; the
//! server stops the rig if they stop arriving.

use crate::link::Cmd;
use crate::panels::{note, section};
use crate::session::Session;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Key, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
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
    /// The camera this station is moving by hand now. Only [`Controller::settle`] changes it, and it
    /// never lets go of a camera without sending it a stop.
    moving: Option<String>,
    /// What the controller on the screen asked for in this frame. Nothing, when it was not drawn.
    ask: Option<Ask>,
    /// The rig was stopped, or kept from starting, with a control still held: the control has to be
    /// let go before it moves anything again.
    let_go_first: bool,
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
            moving: None,
            ask: None,
            let_go_first: false,
            keyboard: false,
            preset_name: String::new(),
            probe: None,
        }
    }
}

/// What the controller asks for in one frame.
#[derive(Clone, PartialEq, Debug)]
pub struct Ask {
    /// The camera in hand.
    pub camera: String,
    /// How far the pad, the sliders and the keys are pushed, as a velocity.
    pub jog: Jog,
    /// Whether this station may move the rig by hand now. See [`blocked`].
    pub allowed: bool,
}

impl Controller {
    /// The one place a jog is decided: what to send now, and to which camera.
    ///
    /// A camera is moved only while the controller is on the screen asking for it, the operator's hands
    /// are on this window (`hands_on`: it has the focus and no dialog is over it) and moving is allowed.
    /// The velocity goes out again every `SEND_EVERY`. The moment any of that stops being so, or the camera
    /// in hand is another one, the camera that was moving is sent a stop. After a stop that the operator
    /// did not make by letting go, nothing moves until the control has been let go and pressed again.
    pub fn settle(&mut self, hands_on: bool, now: Instant) -> Vec<(String, Jog)> {
        let ask = self.ask.take();
        match &ask {
            Some(ask) if ask.jog.is_zero() => self.let_go_first = false,
            Some(ask) => {
                let another_camera = self.moving.as_ref().is_some_and(|camera| *camera != ask.camera);
                if !hands_on || !ask.allowed || another_camera {
                    self.let_go_first = true;
                }
            }
            None => {
                if self.moving.is_some() {
                    self.let_go_first = true;
                }
            }
        }
        let want = ask.filter(|ask| hands_on && ask.allowed && !self.let_go_first && !ask.jog.is_zero());

        let mut send = Vec::new();
        if let Some(camera) = self.moving.take_if(|camera| want.as_ref().is_none_or(|ask| ask.camera != *camera)) {
            send.push((camera, Jog::default()));
            self.last_sent = Some(now);
        }
        if let Some(ask) = want {
            let due = self.moving.is_none() || self.last_sent.is_none_or(|t| now.duration_since(t) >= SEND_EVERY);
            if due {
                self.last_sent = Some(now);
                send.push((ask.camera.clone(), ask.jog));
            }
            self.moving = Some(ask.camera);
        }
        send
    }

    /// The control in hand moves nothing until it is let go and pressed again.
    pub fn waits_to_be_let_go(&self) -> bool {
        self.let_go_first
    }
}

/// Send what the hand controls ask for. Called once a frame, after the window has been drawn, whether the
/// controller was drawn or not: that is how a camera is stopped when the controller is no longer there.
pub fn settle(ctx: &egui::Context, s: &mut Session) {
    let hands_on = ctx.input(|i| i.focused) && !crate::app::modal_open(ctx);
    for (camera, jog) in s.rig.settle(hands_on, Instant::now()) {
        s.link.send(Cmd::Jog(camera, jog));
    }
    if s.rig.moving.is_some() {
        ctx.request_repaint_after(SEND_EVERY);
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
    // The pad is a well with a cross in it; it is ruled in the accent while it has the keyboard.
    p.rect(
        pad,
        RADIUS,
        BASE,
        Stroke::new(if focused { RULE_STRONG } else { RULE }, if focused { ACCENT } else { CONTROL_LINE }),
        StrokeKind::Inside,
    );
    let c = pad.center();
    p.hline(pad.shrink(GAP).x_range(), c.y, Stroke::new(RULE, LINE));
    p.vline(c.x, pad.shrink(GAP).y_range(), Stroke::new(RULE, LINE));
    p.text(pad.right_center() + vec2(-GAP, -GAP), Align2::RIGHT_CENTER, "Pan", micro(), ink(Axis::Pan));
    p.text(pad.center_top() + vec2(U + 2.0, GAP), Align2::LEFT_TOP, "Tilt", micro(), ink(Axis::Tilt));
    let half = pad.width() / 2.0 - 8.0;
    let unshape = |v: f64| if v == 0.0 { 0.0 } else { (v.abs() * (1.0 - DEADZONE) + DEADZONE) * v.signum() };
    let knob = c + vec2(unshape(stick.pan) as f32 * half, -unshape(stick.tilt) as f32 * half);
    let live = stick.pan != 0.0 || stick.tilt != 0.0;
    if live {
        p.line_segment([c, knob], Stroke::new(RULE_STRONG, ACCENT));
    }
    p.circle(knob, 9.0, if live { ACCENT } else { CONTROL }, Stroke::new(1.5, if dim { MUTED } else { OFF_WHITE }));

    for (axis, r) in surface.sliders {
        let v = stick.get(axis);
        p.text(r.left_top() + vec2(0.0, -U), Align2::LEFT_BOTTOM, axis.label(), small(), ink(axis));
        p.text(
            r.right_top() + vec2(0.0, -U),
            Align2::RIGHT_BOTTOM,
            axis.format_in(s.cam_status().rig_pose.get(axis), s.project.units),
            mono(SMALL),
            if dim { MUTED } else { OFF_WHITE },
        );
        let track = Rect::from_center_size(r.center(), vec2(r.width(), U));
        p.rect(track, 0.0, BASE, Stroke::new(RULE, CONTROL_LINE), StrokeKind::Inside);
        p.vline(r.center().x, r.y_range(), Stroke::new(RULE, CONTROL_LINE));
        let x = r.center().x + unshape(v) as f32 * (r.width() / 2.0 - 6.0);
        if v != 0.0 {
            p.line_segment([pos2(r.center().x, r.center().y), pos2(x, r.center().y)], Stroke::new(3.0, ACCENT));
        }
        p.circle(
            pos2(x, r.center().y),
            7.0,
            if v != 0.0 { ACCENT } else { CONTROL },
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
    // Asked for here; sent, or not, by `settle` when the frame is done.
    let ask = Ask { camera: s.cam_id().to_string(), jog: combine(from_pointer, from_keys, s.rig.speed), allowed: why.is_none() };
    s.rig.ask = Some(ask);
    let let_go = s.rig.waits_to_be_let_go() && why.is_none();

    let at_rest = why.is_some() || let_go;
    paint_surface(ui, s, &surface, if at_rest { Jog::default() } else { stick }, focused, why.is_some());
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
        None if let_go => {
            ui.label(RichText::new("The rig was stopped. Let go, then press again to move it.").color(AMBER).small());
        }
        None if focused => {
            ui.label(RichText::new("Keys: arrows pan and tilt,  [ ]  zoom,  , .  elevation").color(ACCENT).small());
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
    egui::Grid::new("rig_tracking").num_columns(3).spacing([PAD, U]).show(ui, |ui| {
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
            ui.label(RichText::new(format!("{error:+.2}")).monospace().color(ink)).on_hover_text(format!(
                "Rig minus timeline. A fault is raised beyond {}",
                axis.format_in(tol.get(axis), s.project.units)
            ));
            let (bar, p) = level_bar(ui, 96.0);
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * share.min(1.0), bar.height()));
            p.rect_filled(fill, 0.0, ink);
            ui.end_row();
        }
    });
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(panel_margin()).show(ui, |ui| {
            if s.project.cameras.len() > 1 {
                let i = s.cam_index();
                ui.horizontal(|ui| {
                    let (tag, _) = ui.allocate_exact_size(NUMBER_TAG, Sense::hover());
                    number_tag(ui.painter(), tag, i + 1, camera_color(i));
                    ui.label(RichText::new(&s.cam().name).font(semibold(BODY)));
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
                    // An adapter that answers is a plain dot: green is for output that reaches the rig.
                    let (dot, word) = match (a.connected, a.calibrated) {
                        (false, _) => (RED, "not answering".to_string()),
                        (true, Some(false)) => (AMBER, "not calibrated".to_string()),
                        (true, _) => match a.round_trip_ms {
                            Some(ms) => (OFF_WHITE, format!("answers in {ms:.1} ms")),
                            None => (OFF_WHITE, "connected".to_string()),
                        },
                    };
                    let (mark, _) = ui.allocate_exact_size(vec2(2.0 * U, 2.0 * U), Sense::hover());
                    if a.connected {
                        ui.painter().circle_filled(mark.center(), U, dot);
                    } else {
                        ui.painter().circle_stroke(mark.center(), U - 1.0, Stroke::new(1.5, dot));
                    }
                    ui.label(RichText::new(&a.name).font(semibold(BODY)));
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
                    // An axis Record will write is a red plate, as the Record button is while it is on.
                    let text = RichText::new(axis.label()).color(if on { ink_on(RED) } else { MUTED });
                    let button = egui::Button::new(text).fill(if on { RED } else { CONTROL });
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
            egui::Grid::new("rig_pose").num_columns(3).spacing([PAD, U]).show(ui, |ui| {
                for axis in Axis::ALL {
                    let lim = s.cam().axes.get(axis);
                    let value = s.cam_status().rig_pose.get(axis);
                    ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                    ui.label(RichText::new(axis.format_in(value, s.project.units)).monospace());
                    let (bar, p) = level_bar(ui, 96.0);
                    let t = ((value - lim.min) / (lim.max - lim.min).max(1e-9)).clamp(0.0, 1.0) as f32;
                    // Where the axis stands in its range: a mark on the bar, in the axis's colour.
                    let at = bar.left() + 1.0 + t * (bar.width() - 2.0);
                    p.vline(at, bar.top() - 2.0..=bar.bottom() + 2.0, Stroke::new(3.0, axis_color(axis)));
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
        let frame = |c: &mut Controller, jog: Jog, at: u64| {
            c.ask = Some(Ask { camera: "cam1".into(), jog, allowed: true });
            c.settle(true, ms(at))
        };
        let to_cam1 = |jog: Jog| vec![("cam1".to_string(), jog)];
        assert_eq!(frame(&mut c, Jog::default(), 0), vec![], "nothing is sent while nothing is held");
        // 60 frames a second for one second: about 30 messages, never more than 250 ms apart.
        let mut sent = Vec::new();
        for n in 0..60u64 {
            if !frame(&mut c, jog, 10 + n * 1000 / 60).is_empty() {
                sent.push(10 + n * 1000 / 60);
            }
        }
        assert_eq!(sent[0], 10, "the first movement goes out at once");
        assert!((28..=31).contains(&sent.len()), "{} messages in a second", sent.len());
        assert!(sent.windows(2).all(|w| w[1] - w[0] <= 50), "gaps stay far inside the server's 250 ms watchdog");
        assert_eq!(frame(&mut c, Jog::default(), 1020), to_cam1(Jog::default()), "release stops the rig at once");
        assert_eq!(frame(&mut c, Jog::default(), 1040), vec![], "and only once");
        assert_eq!(frame(&mut c, jog, 1041), to_cam1(jog), "a new press does not wait out the interval");
    }

    #[test]
    fn the_camera_being_moved_is_always_sent_a_stop_before_anything_else_happens() {
        let t0 = Instant::now();
        let jog = Jog { pan: 0.2, ..Jog::default() };
        let ask = |camera: &str, jog: Jog, allowed: bool| Some(Ask { camera: camera.into(), jog, allowed });
        let stop = |camera: &str| vec![(camera.to_string(), Jog::default())];
        // Each way the hand can come off a moving camera: what is asked next, and whether hands are on the window.
        let ways: [(&str, Option<Ask>, bool); 4] = [
            ("the controller is not drawn", None, true),
            ("another camera is in hand", ask("cam2", jog, true), true),
            ("moving is no longer allowed", ask("cam1", jog, false), true),
            ("a dialog or another window has the operator", ask("cam1", jog, true), false),
        ];
        for (way, next, hands_on) in ways {
            let mut c = Controller { ask: ask("cam1", jog, true), ..Controller::default() };
            assert_eq!(c.settle(true, t0), vec![("cam1".to_string(), jog)]);
            c.ask = next.clone();
            assert_eq!(c.settle(hands_on, t0 + Duration::from_millis(5)), stop("cam1"), "{way}");
            // Still held when that has passed: nothing moves, on either camera.
            for (n, camera) in ["cam1", "cam2", "cam1"].into_iter().enumerate() {
                c.ask = ask(camera, jog, true);
                assert_eq!(c.settle(true, t0 + Duration::from_millis(100 * (n as u64 + 1))), vec![], "{way}, then held");
            }
            // Let go, press again: it moves.
            c.ask = ask("cam2", Jog::default(), true);
            assert_eq!(c.settle(true, t0 + Duration::from_millis(500)), vec![], "{way}");
            c.ask = ask("cam2", jog, true);
            assert_eq!(c.settle(true, t0 + Duration::from_millis(520)), vec![("cam2".to_string(), jog)], "{way}");
        }

        // A control pressed while moving is not allowed does not start the rig when it becomes allowed.
        let mut c = Controller { ask: ask("cam1", jog, false), ..Controller::default() };
        assert_eq!(c.settle(true, t0), vec![]);
        c.ask = ask("cam1", jog, true);
        assert_eq!(c.settle(true, t0 + Duration::from_millis(40)), vec![]);
    }
}
