//! Stage view: the stage from outside, every camera standing in it with what it sees, and the tools
//! that put the cameras where they will stand on site.

use crate::session::Session;
use crate::stage::{self, Key};
use crate::theme::*;
use egui::{vec2, Align2, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use ptz_core::camera::Mount;
use ptz_core::pose::{Pose, Units};
use ptz_scene::mesh::Mesh;
use ptz_scene::render::{self, Camera, Look};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Place,
    Aim,
    Measure,
}

impl Tool {
    pub const ALL: [Tool; 6] = [Tool::Select, Tool::Move, Tool::Rotate, Tool::Place, Tool::Aim, Tool::Measure];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Move => "Move",
            Tool::Rotate => "Rotate",
            Tool::Place => "Place",
            Tool::Aim => "Aim",
            Tool::Measure => "Measure",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click a camera to select it. Drag to move the view",
            Tool::Move => "Drag a camera across the floor",
            Tool::Rotate => "Drag around a camera to turn where its pan 0 points. Shift steps by 5°",
            Tool::Place => "Click where the selected camera should stand",
            Tool::Aim => "Click what the selected camera's pan 0 should face",
            Tool::Measure => "Click two points to measure between them",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Angle {
    Perspective,
    Top,
    Front,
    Side,
}

impl Angle {
    pub const ALL: [Angle; 4] = [Angle::Perspective, Angle::Top, Angle::Front, Angle::Side];

    pub fn label(self) -> &'static str {
        match self {
            Angle::Perspective => "Perspective",
            Angle::Top => "Top",
            Angle::Front => "Front",
            Angle::Side => "Side",
        }
    }

    /// The direction a flat view looks: yaw and pitch in degrees.
    fn direction(self) -> Option<(f32, f32)> {
        match self {
            Angle::Perspective => None,
            Angle::Top => Some((0.0, -90.0)),
            Angle::Front => Some((0.0, 0.0)),
            Angle::Side => Some((90.0, 0.0)),
        }
    }
}

/// What dragging the picture, and the arrows of the view pad, do to the view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    /// Turn the view about the point it looks at.
    Turn,
    /// Slide the view across the stage.
    Slide,
}

/// The lens of the free viewpoint, and the long lens that stands in for a flat projection.
const LENS_DEG: f32 = 50.0;
const FLAT_LENS_DEG: f32 = 2.0;
/// How near the pointer must be to a camera to take hold of it, in points.
const REACH: f32 = 26.0;
/// The nearest and the furthest the viewer comes to what it looks at, in metres.
const NEAREST_M: f32 = 0.3;
const FURTHEST_M: f32 = 4000.0;
/// One press on the view pad turns the view this many degrees, slides it this share of its width,
/// or brings it this much nearer.
const PAD_TURN_DEG: f32 = 15.0;
const PAD_SLIDE_SHARE: f32 = 0.15;
const PAD_ZOOM: f32 = 0.8;
/// A pad button held this long repeats, this many presses a second.
const PAD_HOLD_S: f64 = 0.35;
const PAD_REPEAT_PER_S: f32 = 4.0;

enum Drag {
    Orbit,
    Pan,
    Move { index: usize, from: Mount, grab: [f32; 2] },
    Rotate { index: usize, from: Mount, start: f64 },
}

/// Where the view was drawn last frame and through which camera, for hit tests and the UI tests.
#[derive(Clone, Copy)]
pub struct Probe {
    pub rect: Rect,
    pub camera: Camera,
}

impl Probe {
    /// Where a point of the world is on screen.
    pub fn screen(&self, p: [f32; 3]) -> Option<Pos2> {
        let (w, h) = size_of(self.rect);
        self.camera.project(p, w, h).map(|q| self.rect.min + vec2(q[0], q[1]))
    }

    /// The ray into the world under a point of the screen.
    pub fn ray(&self, at: Pos2) -> ([f32; 3], [f32; 3]) {
        let (w, h) = size_of(self.rect);
        let local = at - self.rect.min;
        (self.camera.eye, self.camera.ray(local.x, local.y, w, h))
    }
}

fn size_of(rect: Rect) -> (usize, usize) {
    (rect.width().round().max(1.0) as usize, rect.height().round().max(1.0) as usize)
}

pub struct View {
    /// Show a camera-eye preview beside the stage and edit its mounting position.
    pub placement: bool,
    pub placement_preview: Option<Probe>,
    pub preview_drag: Option<(String, Mount)>,
    pub tool: Tool,
    pub angle: Angle,
    /// What dragging the picture does, and the arrows of the view pad.
    pub nav: Nav,
    /// A pad button has been held long enough to repeat, so letting go of it is not one more press.
    pad_held: bool,
    /// The point the view turns about.
    pub target: [f32; 3],
    /// Degrees. The direction the viewer looks: 0 toward +Y, positive to the left.
    pub yaw: f32,
    /// Degrees. Negative looks down.
    pub pitch: f32,
    /// Metres from the viewer to the target.
    pub distance: f32,
    /// A moved camera follows the surface under the pointer.
    pub snap: bool,
    /// In a flat view, hide what is between the viewer and the cut.
    pub cut: bool,
    /// Top view: the height above which things are hidden.
    pub cut_height: f32,
    pub fit_requested: bool,
    drag: Option<Drag>,
    /// The points clicked with the Measure tool: none, one, or two.
    pub measure: Vec<[f32; 3]>,
    /// Mounts as they were before each change made here, newest last.
    pub history: Vec<(String, Mount)>,
    pub probe: Option<Probe>,
    /// Where each camera's name was drawn last, by camera; a name is as good a handle as the marker under it.
    pub labels: Vec<Rect>,
}

impl Default for View {
    fn default() -> Self {
        View {
            placement: false,
            placement_preview: None,
            preview_drag: None,
            tool: Tool::Select,
            angle: Angle::Perspective,
            nav: Nav::Turn,
            pad_held: false,
            target: [0.0, 6.0, 1.0],
            yaw: -28.0,
            pitch: -32.0,
            distance: 30.0,
            snap: false,
            cut: false,
            cut_height: 3.0,
            fit_requested: true,
            drag: None,
            measure: Vec::new(),
            history: Vec::new(),
            probe: None,
            labels: Vec::new(),
        }
    }
}

impl View {
    pub fn camera(&self) -> Camera {
        match self.angle.direction() {
            None => {
                let look = Camera::look([0.0; 3], self.yaw, self.pitch, LENS_DEG);
                Camera { eye: [0, 1, 2].map(|k| self.target[k] - look.forward[k] * self.distance), ..look }
            }
            Some((yaw, pitch)) => {
                // From far enough away that the long lens shows the same width as the free view does at `distance`.
                let far = self.distance * (LENS_DEG * 0.5).to_radians().tan() / (FLAT_LENS_DEG * 0.5).to_radians().tan();
                let look = Camera::look([0.0; 3], yaw, pitch, FLAT_LENS_DEG);
                let near = match (self.cut, self.angle) {
                    (true, Angle::Top) => (self.target[2] + far - self.cut_height).max(look.near),
                    (true, _) => far,
                    (false, _) => look.near,
                };
                Camera { eye: [0, 1, 2].map(|k| self.target[k] - look.forward[k] * far), near, ..look }
            }
        }
    }

    /// Metres of the world per point of the screen, at the target.
    fn metres_per_point(&self, rect: Rect) -> f32 {
        2.0 * self.distance * (LENS_DEG * 0.5).to_radians().tan() / rect.width().max(1.0)
    }

    /// Show everything between two corners in a picture `aspect` times as wide as it is high, with a margin round it.
    pub fn fit(&mut self, min: [f32; 3], max: [f32; 3], aspect: f32) {
        const MARGIN: f32 = 0.06;
        self.target = [0, 1, 2].map(|k| (min[k] + max[k]) / 2.0);
        let (width, height) = (1000.0, 1000.0 / aspect.clamp(0.2, 5.0));
        let corners: Vec<[f32; 3]> = (0..8).map(|n| [0, 1, 2].map(|k| if n >> k & 1 == 0 { min[k] } else { max[k] })).collect();
        let cut = std::mem::replace(&mut self.cut, false);
        let shows_all = |view: &View| {
            let camera = view.camera();
            corners.iter().all(|corner| {
                camera.project(*corner, width as usize, height as usize).is_some_and(|p| {
                    (MARGIN * width..=(1.0 - MARGIN) * width).contains(&p[0])
                        && (MARGIN * height..=(1.0 - MARGIN) * height).contains(&p[1])
                })
            })
        };
        // Further away always shows more, so the nearest distance that shows everything is found by halving.
        let (mut near, mut far) = (0.3f32, 4000.0f32);
        for _ in 0..32 {
            self.distance = (near * far).sqrt();
            if shows_all(self) {
                far = self.distance;
            } else {
                near = self.distance;
            }
        }
        self.distance = far;
        self.cut = cut;
    }

    /// Turn the view about its target, in degrees: the viewpoint goes round to the right and up.
    /// Turned, a flat view becomes the free one.
    pub fn turn(&mut self, right: f32, up: f32) {
        if let Some((yaw, pitch)) = self.angle.direction() {
            (self.yaw, self.pitch, self.angle) = (yaw, pitch.max(-89.0), Angle::Perspective);
        }
        self.yaw = (self.yaw + right).rem_euclid(360.0);
        self.pitch = (self.pitch - up).clamp(-89.0, 30.0);
    }

    /// Slide the viewpoint and what it looks at to the right and up the picture, in metres.
    pub fn slide(&mut self, right: f32, up: f32) {
        let camera = self.camera();
        for k in 0..3 {
            self.target[k] += camera.right[k] * right + camera.up[k] * up;
        }
    }

    /// Stand `ratio` times as far from the target: below 1 comes nearer.
    pub fn zoom(&mut self, ratio: f32) {
        self.distance = (self.distance * ratio).clamp(NEAREST_M, FURTHEST_M);
    }

    pub fn is_dragging_a_camera(&self) -> bool {
        matches!(self.drag, Some(Drag::Move { .. } | Drag::Rotate { .. }))
    }
}

/// The corners of everything shown: the stage and the cameras standing in it.
fn extent(s: &Session, poses: &[Pose]) -> ([f32; 3], [f32; 3]) {
    let (mut min, mut max) = s.world.bounds().unwrap_or(([-2.0, -2.0, 0.0], [2.0, 2.0, 2.0]));
    for (camera, pose) in s.project.cameras.iter().zip(poses) {
        let lens = camera.mount.lens_position(pose.elevation).map(|v| v as f32);
        let foot = [lens[0], lens[1], camera.mount.z as f32];
        for p in [lens, foot] {
            for k in 0..3 {
                min[k] = min[k].min(p[k] - 0.5);
                max[k] = max[k].max(p[k] + 0.5);
            }
        }
    }
    (min, max)
}

type Line = ([f32; 3], [f32; 3], [u8; 4], u32);

/// Floor grid lines over the extent: every metre, with every fifth brighter; every five metres on a large stage.
fn grid_lines(min: [f32; 3], max: [f32; 3]) -> (Vec<Line>, f32) {
    let span = (max[0] - min[0]).max(max[1] - min[1]);
    let step = if span > 400.0 {
        50.0
    } else if span > 80.0 {
        5.0
    } else {
        1.0
    };
    let major = step * 5.0;
    let outward = |v: f32, up: bool| if up { (v / major).ceil() * major } else { (v / major).floor() * major };
    let (x0, x1) = (outward(min[0] - 1.0, false), outward(max[0] + 1.0, true));
    let (y0, y1) = (outward(min[1] - 1.0, false), outward(max[1] + 1.0, true));
    let mut lines = Vec::new();
    let color = |v: f32| {
        if v == 0.0 {
            ([150, 150, 150, 210], 1)
        } else if (v / major).round() * major == v {
            ([104, 104, 104, 170], 1)
        } else {
            ([68, 68, 68, 120], 1)
        }
    };
    let count = |a: f32, b: f32| ((b - a) / step).round() as i32;
    for i in 0..=count(x0, x1) {
        let x = x0 + i as f32 * step;
        let (rgba, width) = color(x);
        lines.push(([x, y0, 0.0], [x, y1, 0.0], rgba, width));
    }
    for i in 0..=count(y0, y1) {
        let y = y0 + i as f32 * step;
        let (rgba, width) = color(y);
        lines.push(([x0, y, 0.0], [x1, y, 0.0], rgba, width));
    }
    (lines, step)
}

/// A camera's frustum out to `reach` metres: four edges from the lens and the frame at the far end.
fn frustum_lines(view: &Camera, reach: f32, color: [u8; 3], selected: bool) -> Vec<Line> {
    let corners = stage::frustum_corners(view, reach);
    let (alpha, width) = if selected { (255, 2) } else { (120, 1) };
    let rgba = [color[0], color[1], color[2], alpha];
    let mut lines = Vec::new();
    for (i, corner) in corners.iter().enumerate() {
        lines.push((view.eye, *corner, rgba, width));
        lines.push((*corner, corners[(i + 1) % 4], rgba, width));
    }
    if selected {
        let centre = [0, 1, 2].map(|k| view.eye[k] + view.forward[k] * reach);
        lines.push((view.eye, centre, [color[0], color[1], color[2], 130], 1));
    }
    lines
}

/// The camera whose marker is nearest the pointer, within reach; failing that, the one whose name is under it.
fn camera_under(s: &Session, probe: &Probe, poses: &[Pose], at: Pos2, reach: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, (camera, pose)) in s.project.cameras.iter().zip(poses).enumerate() {
        let lens = camera.mount.lens_position(pose.elevation).map(|v| v as f32);
        let foot = [lens[0], lens[1], camera.mount.z as f32];
        let (Some(a), Some(b)) = (probe.screen(lens), probe.screen(foot)) else { continue };
        let along = b - a;
        let t = if along.length_sq() > 0.0 { ((at - a).dot(along) / along.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
        let distance = (a + along * t - at).length();
        if distance <= reach && best.is_none_or(|(_, d)| distance < d) {
            best = Some((i, distance));
        }
    }
    best.map(|(i, _)| i).or_else(|| s.stage_view.labels.iter().rposition(|label| label.contains(at)))
}

fn round_to(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

/// A heading kept between -180 and 180 degrees.
pub fn wrap_heading(deg: f64) -> f64 {
    let d = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if d == -180.0 {
        180.0
    } else {
        d
    }
}

/// Send a mount changed in this view, remembering what it was.
pub(crate) fn commit(s: &mut Session, index: usize, from: Mount) {
    let Some(camera) = s.project.cameras.get(index) else { return };
    let (id, to) = (camera.id.clone(), camera.mount);
    if to == from {
        return;
    }
    // The change was shown from a local copy; put the old value back so the update is seen as a change.
    s.project.cameras[index].mount = from;
    if !may_place(s) {
        return;
    }
    if let Err(why) = to.validate() {
        s.toast(format!("The camera was not moved: {why}"), true);
        return;
    }
    let mut project = s.project.clone();
    project.cameras[index].mount = to;
    s.stage_view.history.push((id, from));
    if s.stage_view.history.len() > 100 {
        s.stage_view.history.remove(0);
    }
    s.put_project(project);
}

/// Put the last camera moved here back where it was.
pub fn undo(s: &mut Session) {
    while let Some((id, mount)) = s.stage_view.history.pop() {
        if let Some(index) = s.project.camera_index(&id) {
            let mut project = s.project.clone();
            project.cameras[index].mount = mount;
            s.put_project(project);
            s.select_camera(&id);
            return;
        }
    }
}

fn may_place(s: &mut Session) -> bool {
    match s.can_edit() {
        Ok(()) => true,
        Err(why) => {
            s.toast(why.replace("to edit the song", "to place cameras").replace("to edit", "to place cameras"), true);
            false
        }
    }
}

fn toolbar(ui: &mut Ui, s: &mut Session) {
    crate::theme::toolbar(ui, |ui| {
        if ui.selectable_label(s.stage_view.placement, "Camera placement").clicked() {
            crate::placement_ui::cancel_drag(s);
            s.stage_view.placement = !s.stage_view.placement;
            s.stage_view.fit_requested = true;
        }
        ui.separator();
        for tool in Tool::ALL {
            if ui.selectable_label(s.stage_view.tool == tool, tool.label()).on_hover_text(tool.hint()).clicked() {
                s.stage_view.tool = tool;
                if tool != Tool::Measure {
                    s.stage_view.measure.clear();
                }
            }
        }
        ui.separator();
        for angle in Angle::ALL {
            if ui.selectable_label(s.stage_view.angle == angle, angle.label()).clicked() && s.stage_view.angle != angle {
                s.stage_view.angle = angle;
                // Each angle needs its own distance to show the whole stage.
                s.stage_view.fit_requested = true;
            }
        }
        if ui.button("Fit").on_hover_text("Show the whole stage and every camera").clicked() {
            s.stage_view.fit_requested = true;
        }
        ui.separator();
        ui.checkbox(&mut s.stage_view.snap, "Snap to model")
            .on_hover_text("A moved camera follows the surface under the pointer, so it can stand on a riser or a balcony");
        if s.stage_view.angle != Angle::Perspective {
            ui.checkbox(&mut s.stage_view.cut, "Cut").on_hover_text(
                "Top: hide what is above the height beside this, such as a ceiling. Front and Side: hide what is nearer than the middle of the view",
            );
            if s.stage_view.angle == Angle::Top {
                let mut height = s.stage_view.cut_height as f64;
                let field = crate::units::distance_box(&mut height, s.project.units).range(-100.0..=1000.0);
                if ui.add_enabled(s.stage_view.cut, field).changed() {
                    s.stage_view.cut_height = height as f32;
                }
            }
        }
        let undo_button = ui
            .add_enabled(!s.stage_view.history.is_empty(), egui::Button::new("Undo move"))
            .on_hover_text("Put the last camera moved in this view back where it was");
        if undo_button.clicked() && may_place(s) {
            undo(s);
        }
    });
}

/// Pointer and keyboard: look around, and use the tool in hand.
fn interact(ui: &Ui, s: &mut Session, resp: &egui::Response, poses: &[Pose]) {
    let rect = resp.rect;
    let probe = Probe { rect, camera: s.stage_view.camera() };
    let (modifiers, press, scroll, pinch, escape) = ui.input(|i| {
        (i.modifiers, i.pointer.press_origin(), i.smooth_scroll_delta.y, i.zoom_delta(), i.key_pressed(egui::Key::Escape))
    });

    if resp.hovered() && (scroll != 0.0 || pinch != 1.0) {
        s.stage_view.zoom((-scroll * 0.0025).exp() / pinch);
    }

    if escape {
        match s.stage_view.drag.take() {
            Some(Drag::Move { index, from, .. } | Drag::Rotate { index, from, .. }) => {
                if let Some(camera) = s.project.cameras.get_mut(index) {
                    camera.mount = from;
                }
            }
            _ => s.stage_view.measure.clear(),
        }
    }

    if resp.drag_started() {
        let at = press.unwrap_or(rect.center());
        let other_button = resp.drag_started_by(PointerButton::Secondary) || resp.drag_started_by(PointerButton::Middle);
        let tool = s.stage_view.tool;
        let hit = camera_under(s, &probe, poses, at, if tool == Tool::Rotate { REACH * 3.0 } else { REACH });
        let drag = if other_button || (modifiers.shift && tool != Tool::Rotate) {
            Drag::Pan
        } else {
            match (tool, hit) {
                (Tool::Move, Some(index)) if may_place(s) => {
                    let from = s.project.cameras[index].mount;
                    let (eye, ray) = probe.ray(at);
                    let on_floor = stage::on_plane(eye, ray, from.z as f32).unwrap_or([from.x as f32, from.y as f32, 0.0]);
                    let id = s.project.cameras[index].id.clone();
                    s.select_camera(&id);
                    Drag::Move { index, from, grab: [from.x as f32 - on_floor[0], from.y as f32 - on_floor[1]] }
                }
                (Tool::Rotate, Some(index)) if may_place(s) => {
                    let from = s.project.cameras[index].mount;
                    let (eye, ray) = probe.ray(at);
                    let start = stage::on_plane(eye, ray, from.z as f32)
                        .map(|p| stage::heading_toward(&from, p))
                        .unwrap_or(from.heading_deg);
                    let id = s.project.cameras[index].id.clone();
                    s.select_camera(&id);
                    Drag::Rotate { index, from, start }
                }
                _ if s.stage_view.nav == Nav::Slide => Drag::Pan,
                _ => Drag::Orbit,
            }
        };
        s.stage_view.drag = Some(drag);
    }

    if resp.dragged() {
        let delta = resp.drag_delta();
        let at = resp.interact_pointer_pos();
        match &s.stage_view.drag {
            // The picture is taken hold of: it goes with the pointer, so the viewpoint goes the other way.
            Some(Drag::Orbit) => s.stage_view.turn(-delta.x * 0.35, delta.y * 0.35),
            Some(Drag::Pan) => {
                let scale = s.stage_view.metres_per_point(rect);
                s.stage_view.slide(-delta.x * scale, delta.y * scale);
            }
            Some(Drag::Move { index, from, grab }) => {
                let (index, from, grab) = (*index, *from, *grab);
                if let Some(at) = at {
                    let (eye, ray) = probe.ray(at);
                    let mut mount = from;
                    if s.stage_view.snap {
                        if let Some(p) = s.world.ground(eye, ray) {
                            (mount.x, mount.y, mount.z) = (p[0] as f64, p[1] as f64, p[2] as f64);
                        }
                    } else if let Some(p) = stage::on_plane(eye, ray, from.z as f32) {
                        (mount.x, mount.y) = ((p[0] + grab[0]) as f64, (p[1] + grab[1]) as f64);
                    }
                    (mount.x, mount.y, mount.z) = (round_to(mount.x, 0.01), round_to(mount.y, 0.01), round_to(mount.z, 0.01));
                    if let Some(camera) = s.project.cameras.get_mut(index) {
                        camera.mount = mount;
                    }
                }
            }
            Some(Drag::Rotate { index, from, start }) => {
                let (index, from, start) = (*index, *from, *start);
                if let Some(at) = at {
                    let (eye, ray) = probe.ray(at);
                    if let Some(p) = stage::on_plane(eye, ray, from.z as f32) {
                        let turned = from.heading_deg + stage::heading_toward(&from, p) - start;
                        let step = if modifiers.shift { 5.0 } else { 0.5 };
                        if let Some(camera) = s.project.cameras.get_mut(index) {
                            camera.mount.heading_deg = wrap_heading(round_to(turned, step));
                        }
                    }
                }
            }
            None => {}
        }
    }

    if resp.drag_stopped() {
        if let Some(Drag::Move { index, from, .. } | Drag::Rotate { index, from, .. }) = s.stage_view.drag.take() {
            commit(s, index, from);
        }
    }

    if resp.clicked() {
        let Some(at) = resp.interact_pointer_pos() else { return };
        let (eye, ray) = probe.ray(at);
        let tool = s.stage_view.tool;
        match tool {
            Tool::Select | Tool::Move | Tool::Rotate => {
                if let Some(index) = camera_under(s, &probe, poses, at, REACH) {
                    let id = s.project.cameras[index].id.clone();
                    s.select_camera(&id);
                    s.axis_lanes.clear();
                }
            }
            Tool::Place | Tool::Aim => {
                let point = s.world.ground(eye, ray);
                match point {
                    Some(p) if may_place(s) => {
                        let index = s.cam_index();
                        let from = s.project.cameras[index].mount;
                        let mount = &mut s.project.cameras[index].mount;
                        if tool == Tool::Place {
                            (mount.x, mount.y, mount.z) =
                                (round_to(p[0] as f64, 0.01), round_to(p[1] as f64, 0.01), round_to(p[2] as f64, 0.01));
                        } else {
                            mount.heading_deg = wrap_heading(round_to(stage::heading_toward(&from, p), 0.1));
                        }
                        commit(s, index, from);
                    }
                    Some(_) => {}
                    None => s.toast("There is nothing under the pointer there, not even the floor", true),
                }
            }
            Tool::Measure => {
                if let Some(p) = s.world.ground(eye, ray) {
                    if s.stage_view.measure.len() >= 2 {
                        s.stage_view.measure.clear();
                    }
                    s.stage_view.measure.push(p);
                }
            }
        }
    }
}

/// One button of the view pad.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PadKey {
    Mode(Nav),
    /// Move the viewpoint right and up by this many presses' worth.
    Arrow(f32, f32),
    /// Come nearer, or go further away.
    Zoom(bool),
    Fit,
}

impl PadKey {
    /// The pad as it is laid out: three rows of three.
    const ALL: [PadKey; 9] = [
        PadKey::Mode(Nav::Turn),
        PadKey::Arrow(0.0, 1.0),
        PadKey::Mode(Nav::Slide),
        PadKey::Arrow(-1.0, 0.0),
        PadKey::Fit,
        PadKey::Arrow(1.0, 0.0),
        PadKey::Zoom(false),
        PadKey::Arrow(0.0, -1.0),
        PadKey::Zoom(true),
    ];

    fn glyph(self) -> &'static str {
        match self {
            PadKey::Mode(Nav::Turn) => icon::ARROWS_CLOCKWISE,
            PadKey::Mode(Nav::Slide) => icon::ARROWS_OUT_CARDINAL,
            PadKey::Arrow(x, y) => match (x < 0.0, x > 0.0, y > 0.0) {
                (true, _, _) => icon::CARET_LEFT,
                (_, true, _) => icon::CARET_RIGHT,
                (_, _, true) => icon::CARET_UP,
                _ => icon::CARET_DOWN,
            },
            PadKey::Zoom(true) => icon::PLUS,
            PadKey::Zoom(false) => icon::MINUS,
            PadKey::Fit => icon::CORNERS_OUT,
        }
    }

    /// What a screen reader, and a test, finds the button by.
    fn name(self) -> &'static str {
        match self {
            PadKey::Mode(Nav::Turn) => "Turn the view",
            PadKey::Mode(Nav::Slide) => "Slide the view",
            PadKey::Arrow(x, y) => match (x < 0.0, x > 0.0, y > 0.0) {
                (true, _, _) => "View left",
                (_, true, _) => "View right",
                (_, _, true) => "View up",
                _ => "View down",
            },
            PadKey::Zoom(true) => "Zoom in",
            PadKey::Zoom(false) => "Zoom out",
            PadKey::Fit => "Show everything",
        }
    }

    fn tip(self, nav: Nav) -> String {
        let way = |x: f32, y: f32, words: [&'static str; 4]| match (x < 0.0, x > 0.0, y > 0.0) {
            (true, _, _) => words[0],
            (_, true, _) => words[1],
            (_, _, true) => words[2],
            _ => words[3],
        };
        match (self, nav) {
            (PadKey::Mode(Nav::Turn), _) => "Dragging the picture, and the arrows here, turn the view about the stage".into(),
            (PadKey::Mode(Nav::Slide), _) => "Dragging the picture, and the arrows here, slide the view across the stage".into(),
            (PadKey::Arrow(x, y), Nav::Turn) => {
                format!("Look from further {}. Hold to keep turning", way(x, y, ["left", "right", "up", "down"]))
            }
            (PadKey::Arrow(x, y), Nav::Slide) => {
                format!("Slide the view {}. Hold to keep going", way(x, y, ["left", "right", "up", "down"]))
            }
            (PadKey::Zoom(true), _) => "Come nearer. Hold to keep going".into(),
            (PadKey::Zoom(false), _) => "Go further away. Hold to keep going".into(),
            (PadKey::Fit, _) => "Show the whole stage and every camera".into(),
        }
    }
}

/// The view pad, at the top right of the picture: every way of moving the view as a button, for a
/// mouse with no wheel and for a ball that is awkward to drag. Acts on what was pressed, and gives
/// the buttons to be drawn once the picture is.
fn view_pad(ui: &mut Ui, s: &mut Session, view: Rect) -> Vec<(PadKey, egui::Response)> {
    const STEP: f32 = CONTROL_H + U / 2.0;
    let side = 3.0 * STEP - U / 2.0;
    if view.width() < side + 4.0 * GAP || view.height() < side + 4.0 * GAP {
        return Vec::new();
    }
    let corner = view.right_top() + vec2(-GAP - side, GAP);
    let (now, pressed_at, dt, any_down) =
        ui.input(|i| (i.time, i.pointer.press_start_time(), i.stable_dt.min(0.1), i.pointer.any_down()));
    let nav = s.stage_view.nav;
    let mut buttons = Vec::with_capacity(PadKey::ALL.len());
    for (n, key) in PadKey::ALL.into_iter().enumerate() {
        let at = corner + vec2((n % 3) as f32 * STEP, (n / 3) as f32 * STEP);
        let rect = Rect::from_min_size(at, vec2(CONTROL_H, CONTROL_H));
        // The button takes a drag as well as a click, so one begun on it does not move the view under it.
        let resp = ui.interact(rect, ui.id().with(("view_pad", n)), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, key.name()));
        let resp = resp.on_hover_text(key.tip(nav));
        // A ball that rolls a little under the finger makes a click a short drag: both are a press.
        let let_go = resp.clicked() || resp.drag_stopped();
        let presses = if resp.is_pointer_button_down_on() && pressed_at.is_some_and(|t| now - t >= PAD_HOLD_S) {
            s.stage_view.pad_held = true;
            ui.ctx().request_repaint();
            PAD_REPEAT_PER_S * dt
        } else if let_go && !s.stage_view.pad_held {
            1.0
        } else {
            0.0
        };
        let v = &mut s.stage_view;
        match key {
            PadKey::Mode(to) if let_go => v.nav = to,
            PadKey::Fit if let_go => v.fit_requested = true,
            PadKey::Arrow(x, y) if presses > 0.0 => match v.nav {
                Nav::Turn => v.turn(x * PAD_TURN_DEG * presses, y * PAD_TURN_DEG * presses),
                Nav::Slide => {
                    let metres = v.metres_per_point(view) * view.width() * PAD_SLIDE_SHARE * presses;
                    v.slide(x * metres, y * metres);
                }
            },
            PadKey::Zoom(nearer) if presses > 0.0 => v.zoom(if nearer { PAD_ZOOM } else { 1.0 / PAD_ZOOM }.powf(presses)),
            _ => {}
        }
        buttons.push((key, resp));
    }
    if !any_down {
        s.stage_view.pad_held = false;
    }
    buttons
}

fn paint_pad(painter: &egui::Painter, nav: Nav, buttons: &[(PadKey, egui::Response)]) {
    let Some(around) = buttons.iter().map(|(_, r)| r.rect).reduce(|a, b| a.union(b)) else { return };
    painter.rect_filled(around.expand(U / 2.0), RADIUS, plate_fill());
    for (key, resp) in buttons {
        let on = matches!(key, PadKey::Mode(mode) if *mode == nav);
        let fill = match (on, resp.is_pointer_button_down_on(), resp.hovered()) {
            (true, _, _) => CONTROL_LINE,
            (_, true, _) => CONTROL_LINE,
            (_, _, true) => CONTROL_HOVER,
            _ => CONTROL,
        };
        painter.rect_filled(resp.rect, RADIUS, fill);
        let ink = if on { ACCENT } else { OFF_WHITE };
        painter.text(resp.rect.center(), Align2::CENTER_CENTER, key.glyph(), icon_font(BODY + 3.0), ink);
    }
}

/// The distance between two measured points, in words.
pub fn measured(a: [f32; 3], b: [f32; 3], units: Units) -> String {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let (straight, floor) = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), d[0].hypot(d[1]));
    let say = |m: f32| units.distance(m as f64, 2);
    if d[2].abs() < 0.005 {
        say(straight)
    } else {
        let way = if d[2] > 0.0 { "up" } else { "down" };
        format!("{}  ({} along the floor, {} {way})", say(straight), say(floor), say(d[2].abs()))
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    toolbar(ui, s);
    s.stage_view.placement_preview = None;
    if s.stage_view.placement {
        let room = ui.available_size();
        let inspector = (room.x * 0.42).clamp(265.0, 340.0).min(room.x * 0.55);
        ui.horizontal_top(|ui| {
            let stage_width = (room.x - inspector - ui.spacing().item_spacing.x).max(60.0);
            ui.allocate_ui_with_layout(vec2(stage_width, room.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                show_scene(ui, s);
            });
            ui.allocate_ui_with_layout(vec2(inspector, room.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                crate::placement_ui::show(ui, s);
            });
        });
    } else {
        show_scene(ui, s);
    }
}

fn show_scene(ui: &mut Ui, s: &mut Session) {
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let rect = resp.rect;
    painter.rect_filled(rect, 0.0, BASE);
    if rect.width() < 60.0 || rect.height() < 40.0 {
        s.stage_view.probe = None;
        return;
    }
    let poses = s.poses();
    let (min, max) = extent(s, &poses);
    let pad = view_pad(ui, s, rect);
    // Fitting waits for the stage to be what the project asks for, so a model still on its way is not missed.
    if s.stage_view.fit_requested && !s.world.is_busy(&s.project.stage) {
        s.stage_view.fit(min, max, rect.width() / rect.height());
        s.stage_view.fit_requested = false;
    }
    interact(ui, s, &resp, &poses);

    let view = s.stage_view.camera();
    let probe = Probe { rect, camera: view };
    s.stage_view.probe = Some(probe);
    let selected = s.cam_index();

    let (mut lines, grid_step) = grid_lines(min, max);
    let mut markers: Vec<Mesh> = Vec::new();
    let mut key = Key::new().camera(&view).add(selected);
    for (i, (camera, pose)) in s.project.cameras.iter().zip(&poses).enumerate() {
        let color = stage::rgb(camera_color(i));
        markers.push(stage::marker(&camera.mount, pose, color));
        let lens_view = stage::view_of(camera, pose);
        let reach = s.world.centre_distance(camera, pose).unwrap_or(stage::FRUSTUM_REACH_M).clamp(0.5, 200.0);
        lines.extend(frustum_lines(&lens_view, reach, color, i == selected));
        let m = &camera.mount;
        key = key.camera(&lens_view).add(m.inverted).floats(&[
            m.x,
            m.y,
            m.z,
            m.heading_deg,
            m.tilt_deg,
            m.base_height_mm,
            reach as f64,
        ]);
    }
    if let [a, b] = s.stage_view.measure[..] {
        lines.push((a, b, [241, 242, 238, 255], 2));
    }
    for (a, b, rgba, width) in &lines {
        key = key.floats(&[a[0], a[1], a[2], b[0], b[1], b[2]].map(f64::from)).add((rgba, width));
    }

    let size = stage::pixel_size(rect, ui.ctx().pixels_per_point(), crate::monitor::WIDEST_PX);
    let texture = s.world.picture(ui.ctx(), "stage", size, key.finish(), true, |frame, scene| {
        let look = Look::default();
        render::draw_scene(frame, &view, scene, &look);
        for marker in &markers {
            render::draw_mesh(frame, &view, marker, &look);
        }
        for (a, b, rgba, width) in &lines {
            render::draw_line(frame, &view, *a, *b, *rgba, *width);
        }
    });
    stage::paint_picture(&painter, rect, texture);

    // Names over the picture, so a camera is found even behind a wall.
    let clipped = painter.with_clip_rect(rect);
    let mut labels = vec![Rect::NOTHING; s.project.cameras.len()];
    for (i, (camera, pose)) in s.project.cameras.iter().zip(&poses).enumerate() {
        let lens = camera.mount.lens_position(pose.elevation).map(|v| v as f32);
        if let Some(at) = probe.screen([lens[0], lens[1], lens[2] + 0.25]).filter(|at| rect.contains(*at)) {
            let on = i == selected;
            let text = format!("{} {}", i + 1, camera.name);
            // Every name in its camera's colour on a dark plate; the camera in hand has a rule round it too.
            labels[i] = crate::monitor::badge(
                &clipped,
                at + vec2(0.0, -4.0),
                Align2::CENTER_BOTTOM,
                &text,
                plate_fill(),
                camera_color(i),
            );
            if on {
                clipped.rect_stroke(labels[i], RADIUS, Stroke::new(RULE, camera_color(i)), StrokeKind::Inside);
            }
        }
    }
    s.stage_view.labels = labels;
    for p in &s.stage_view.measure {
        if let Some(at) = probe.screen(*p) {
            clipped.circle(at, 4.0, OFF_WHITE, Stroke::new(RULE, BASE));
        }
    }

    // What is in hand, where the selected camera stands, and the scale.
    let camera = s.cam();
    let m = camera.mount;
    let units = s.project.units;
    let stands = format!(
        "{}   x {:.2}  y {:.2}  z {}   heading {:.1}°   lens at {}",
        s.project.label(&camera.id),
        m.x / units.m_per_long(),
        m.y / units.m_per_long(),
        units.distance(m.z, 2),
        m.heading_deg,
        units.distance(m.lens_position(poses[selected].elevation)[2], 2)
    );
    let text = |at: Pos2, anchor: Align2, text: String, ink: egui::Color32| plate(&painter, at, anchor, &text, plate_fill(), ink);
    text(rect.left_top() + vec2(GAP, GAP), Align2::LEFT_TOP, stands, camera_color(selected));
    let hint = match (&s.stage_view.measure[..], s.stage_view.tool) {
        ([a, b], Tool::Measure) => format!("Measured {}", measured(*a, *b, units)),
        ([_], Tool::Measure) => "Click the second point".to_string(),
        (_, tool) if s.stage_view.nav == Nav::Slide => format!("{}. Dragging slides; scroll or the pad zooms", tool.hint()),
        (_, tool) => format!("{}. Shift-drag slides; scroll or the pad zooms", tool.hint()),
    };
    let hint = text(rect.left_bottom() + vec2(GAP, -GAP), Align2::LEFT_BOTTOM, hint, OFF_WHITE);
    let ms = s.world.timings.get("stage").copied().unwrap_or(0.0);
    let scale = format!("{} triangles, drawn in {ms:.1} ms   Grid {grid_step:.0} m", s.world.triangles());
    // The figures about the drawing give way to the hint where the view is too narrow for both.
    let needs = painter.layout_no_wrap(scale.clone(), small(), MUTED).size().x + 3.0 * GAP;
    if hint.right() + needs <= rect.right() {
        text(rect.right_bottom() + vec2(-GAP, -GAP), Align2::RIGHT_BOTTOM, scale, MUTED);
    }
    if let Some(wait) = crate::monitor::stage_wait(s) {
        text(rect.center_top() + vec2(0.0, GAP), Align2::CENTER_TOP, wait.to_string(), AMBER);
    }
    paint_pad(&painter, s.stage_view.nav, &pad);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_views_look_the_way_their_names_say_and_show_the_same_width_as_the_free_view() {
        let mut v = View { target: [2.0, 5.0, 1.0], distance: 20.0, ..View::default() };
        let width_at_target = |camera: &Camera| {
            let (a, b) = ([1.0, 5.0, 1.0], [3.0, 5.0, 1.0]);
            let (a, b) = (camera.project(a, 1000, 600).unwrap(), camera.project(b, 1000, 600).unwrap());
            (b[0] - a[0]).abs()
        };
        v.yaw = 0.0;
        v.pitch = 0.0;
        let free = width_at_target(&v.camera());
        v.angle = Angle::Top;
        let top = v.camera();
        // +X is to the right and +Y up the screen; the target is in the middle.
        let centre = top.project(v.target, 1000, 600).unwrap();
        assert!((centre[0] - 500.0).abs() < 0.01 && (centre[1] - 300.0).abs() < 0.01, "{centre:?}");
        assert!(top.project([3.0, 5.0, 1.0], 1000, 600).unwrap()[0] > 500.0);
        assert!(top.project([2.0, 6.0, 1.0], 1000, 600).unwrap()[1] < 300.0);
        assert!((width_at_target(&top) - free).abs() < 0.5, "{} against {free}", width_at_target(&top));
        // A long lens far away is nearly flat: a point 5 m higher lands within a few pixels of the one below it.
        let (low, high) = (top.project([8.0, 5.0, 0.0], 1000, 600).unwrap(), top.project([8.0, 5.0, 5.0], 1000, 600).unwrap());
        assert!((high[0] - low[0]).abs() < 5.0, "{low:?} {high:?}");

        v.angle = Angle::Front;
        let front = v.camera();
        assert!(front.forward[1] > 0.999, "the front view looks toward the stage, along +Y");
        assert!(front.project([3.0, 5.0, 1.0], 1000, 600).unwrap()[0] > 500.0);
        v.angle = Angle::Side;
        let side = v.camera();
        assert!(side.forward[0] < -0.999, "the side view looks along -X");
        assert!(side.project([2.0, 6.0, 1.0], 1000, 600).unwrap()[0] > 500.0, "+Y, the stage end, is to the right");

        // The cut in a top view hides what is above its height and keeps what is below.
        v.angle = Angle::Top;
        v.cut = true;
        v.cut_height = 3.0;
        let cut = v.camera();
        assert!(cut.project([2.0, 5.0, 3.5], 1000, 600).is_none());
        assert!(cut.project([2.0, 5.0, 2.5], 1000, 600).is_some());
    }

    #[test]
    fn fitting_centres_the_view_and_headings_wrap_and_distances_read_in_words() {
        // Whatever the angle and the shape of the picture, every corner is inside it and the view is not further off than needed.
        let (min, max) = ([-20.0, -5.0, 0.0], [20.0, 25.0, 9.0]);
        let corners: Vec<[f32; 3]> = (0..8).map(|n| [0, 1, 2].map(|k| if n >> k & 1 == 0 { min[k] } else { max[k] })).collect();
        for angle in Angle::ALL {
            for (width, height) in [(1000usize, 600usize), (1076, 430), (400, 900)] {
                let mut v = View { angle, cut: true, ..View::default() };
                v.fit(min, max, width as f32 / height as f32);
                assert_eq!(v.target, [0.0, 10.0, 4.5]);
                assert!(v.cut, "fitting leaves the cut as it was");
                v.cut = false;
                let inside = |v: &View, edge: f32| {
                    let camera = v.camera();
                    corners.iter().all(|corner| {
                        camera.project(*corner, width, height).is_some_and(|p| {
                            (edge..=width as f32 - edge).contains(&p[0]) && (edge..=height as f32 - edge).contains(&p[1])
                        })
                    })
                };
                assert!(inside(&v, 0.0), "{angle:?} at {width} by {height} leaves a corner outside");
                v.distance *= 0.8;
                assert!(
                    !inside(&v, 0.04 * height as f32),
                    "{angle:?} at {width} by {height} stands further off than it needs to"
                );
            }
        }
        assert_eq!(wrap_heading(190.0), -170.0);
        assert_eq!(wrap_heading(-190.0), 170.0);
        assert_eq!(wrap_heading(180.0), 180.0);
        assert_eq!(wrap_heading(-180.0), 180.0);
        assert_eq!(wrap_heading(45.5), 45.5);
        let m = Units::Metric;
        assert_eq!(measured([0.0, 0.0, 0.0], [3.0, 4.0, 0.0], m), "5.00 m");
        assert_eq!(measured([0.0, 0.0, 0.0], [3.0, 4.0, 12.0], m), "13.00 m  (5.00 m along the floor, 12.00 m up)");
        assert_eq!(measured([0.0, 0.0, 2.0], [0.0, 1.0, 0.0], m), "2.24 m  (1.00 m along the floor, 2.00 m down)");
        assert_eq!(measured([0.0, 0.0, 0.0], [0.0, 3.048, 0.0], Units::Imperial), "10.00 ft");
    }

    #[test]
    fn the_grid_covers_the_extent_in_whole_metres_and_coarsens_on_a_large_stage() {
        let (lines, step) = grid_lines([-3.2, -1.0, 0.0], [4.1, 12.0, 3.0]);
        assert_eq!(step, 1.0);
        let xs: Vec<f32> = lines.iter().filter(|l| l.0[0] == l.1[0]).map(|l| l.0[0]).collect();
        assert_eq!((xs[0], *xs.last().unwrap()), (-5.0, 10.0));
        assert_eq!(xs.len(), 16);
        assert!(lines.iter().all(|l| l.0[2] == 0.0 && l.1[2] == 0.0));
        let (lines, step) = grid_lines([-100.0, -60.0, 0.0], [100.0, 60.0, 20.0]);
        assert_eq!(step, 5.0);
        assert!(lines.len() < 120, "{}", lines.len());
        let (_, step) = grid_lines([-900.0, -900.0, 0.0], [900.0, 900.0, 20.0]);
        assert_eq!(step, 50.0);
    }

    #[test]
    fn the_view_turns_slides_and_zooms_about_what_it_looks_at() {
        let mut v = View { target: [0.0, 0.0, 0.0], yaw: 0.0, pitch: 0.0, distance: 10.0, ..View::default() };
        let eye = |v: &View| v.camera().eye;

        // Level and looking up the stage, the viewer stands 10 m down it. Turned right, it goes round to stage right.
        assert!(eye(&v)[1] < -9.9);
        v.turn(90.0, 0.0);
        assert!((eye(&v)[0] - 10.0).abs() < 1e-3 && eye(&v)[1].abs() < 1e-3, "{:?}", eye(&v));
        assert_eq!(v.target, [0.0, 0.0, 0.0], "turning leaves what is looked at where it is");
        // Up is over the top, and stops short of straight down; down stops a little under level.
        v.turn(0.0, 45.0);
        assert!((eye(&v)[2] - 10.0 * 45f32.to_radians().sin()).abs() < 1e-3);
        v.turn(0.0, 400.0);
        assert_eq!(v.pitch, -89.0);
        v.turn(0.0, -400.0);
        assert_eq!(v.pitch, 30.0);

        // Turning a flat view makes it the free one, from where the flat one looked.
        let mut top = View { angle: Angle::Top, ..View::default() };
        top.turn(15.0, 0.0);
        assert_eq!(top.angle, Angle::Perspective);
        assert_eq!(top.pitch, -89.0);

        // Sliding takes the viewer and what it looks at the same way, across the picture.
        let mut v = View { target: [0.0, 0.0, 0.0], yaw: 0.0, pitch: 0.0, distance: 10.0, ..View::default() };
        v.slide(2.0, 1.0);
        assert!((v.target[0] - 2.0).abs() < 1e-4 && v.target[1].abs() < 1e-4 && (v.target[2] - 1.0).abs() < 1e-4);
        assert!((eye(&v)[0] - 2.0).abs() < 1e-3 && (eye(&v)[2] - 1.0).abs() < 1e-3);

        // Zoom comes nearer and goes away, between its two ends.
        v.zoom(0.5);
        assert_eq!(v.distance, 5.0);
        v.zoom(1e-6);
        assert_eq!(v.distance, NEAREST_M);
        v.zoom(1e9);
        assert_eq!(v.distance, FURTHEST_M);
    }
}
