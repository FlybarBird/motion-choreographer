//! The stage as the app draws it: the built-in stage or an imported model, the cameras standing in it,
//! and the pictures made of them. The world is in metres, Z up, pan 0 of an unturned camera looking along +Y.

use ptz_core::camera::{CameraConfig, Mount, Stage, Unit, UpAxis};
use ptz_core::pose::{Pose, Project};
use ptz_scene::mesh::{Mesh, Picker, Transform};
use ptz_scene::render::{self, Camera, Frame, Look, Scene};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Behind everything.
pub const SKY: [u8; 3] = [15, 15, 15];
/// Models above this many triangles are made ready on another thread, so the window does not stall.
const BUILD_INLINE_BELOW: usize = 150_000;
/// A frustum is drawn this far when its centre ray meets nothing.
pub const FRUSTUM_REACH_M: f32 = 15.0;
/// Pictures other than the ones being worked in share this much drawing time per window frame.
const BUDGET_MS: f32 = 12.0;
/// A picture that takes longer than this at full size is drawn at half size while it changes.
const SLOW_MS: f32 = 14.0;
/// The largest model file that is read.
pub const LARGEST_FILE: u64 = 200 * 1024 * 1024;

pub const LECTERN: [f32; 2] = [5.3, 12.0];
pub const AWARDS: [f32; 2] = [-3.9, 12.0];
pub const DECK_HEIGHT: f32 = 0.5;
const DECK_FRONT: f32 = 9.0;
const BACK_WALL: f32 = 16.0;

/// Closed procedural geometry shared by the GPU preview, CPU fallback and scene picking.
/// Seams and poles use identical positions so normal generation can weld them reliably.
fn cylinder(mesh: &mut Mesh, from: [f32; 3], to: [f32; 3], radii: [f32; 2], color: [u8; 3]) {
    const SIDES: usize = 24;
    let delta = [0, 1, 2].map(|k| to[k] - from[k]);
    let length = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    if length < 1e-6 {
        return;
    }
    let w = delta.map(|v| v / length);
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let side = cross(w, if w[2].abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] });
    let side_length = side.iter().map(|v| v * v).sum::<f32>().sqrt();
    let u = side.map(|v| v / side_length);
    let v = cross(w, u);
    let ring = |centre: [f32; 3], radius: f32, i: usize| {
        let (sin, cos) = ((i % SIDES) as f32 * std::f32::consts::TAU / SIDES as f32).sin_cos();
        [0, 1, 2].map(|k| centre[k] + radius * (u[k] * cos + v[k] * sin))
    };
    for i in 0..SIDES {
        let (a, b) = (ring(from, radii[0], i), ring(from, radii[0], i + 1));
        let (c, d) = (ring(to, radii[1], i + 1), ring(to, radii[1], i));
        mesh.push_quad([a, b, c, d], color);
        mesh.push_triangle(from, b, a, color);
        mesh.push_triangle(to, d, c, color);
    }
}

fn ellipsoid(mesh: &mut Mesh, centre: [f32; 3], radii: [f32; 3], color: [u8; 3]) {
    const SIDES: usize = 20;
    const RINGS: usize = 12;
    let point = |ring: usize, side: usize| {
        if ring == 0 {
            return [centre[0], centre[1], centre[2] - radii[2]];
        }
        if ring == RINGS {
            return [centre[0], centre[1], centre[2] + radii[2]];
        }
        let (height, width) = (std::f32::consts::PI * (ring as f32 / RINGS as f32 - 0.5)).sin_cos();
        let (y, x) = (std::f32::consts::TAU * (side % SIDES) as f32 / SIDES as f32).sin_cos();
        [centre[0] + radii[0] * width * x, centre[1] + radii[1] * width * y, centre[2] + radii[2] * height]
    };
    for ring in 0..RINGS {
        for side in 0..SIDES {
            let [a, b, c, d] = [point(ring, side), point(ring, side + 1), point(ring + 1, side + 1), point(ring + 1, side)];
            if ring == 0 {
                mesh.push_triangle(a, c, d, color);
            } else if ring == RINGS - 1 {
                mesh.push_triangle(a, b, c, color);
            } else {
                mesh.push_quad([a, b, c, d], color);
            }
        }
    }
}

fn person(mesh: &mut Mesh, x: f32, y: f32, color: [u8; 3]) {
    let at = |dx: f32, dy: f32, z: f32| [x + dx, y + dy, DECK_HEIGHT + z];
    let dark = color.map(|c| (c as f32 * 0.52).round() as u8);
    let skin = [205, 181, 162];
    ellipsoid(mesh, at(0.0, 0.0, 1.16), [0.23, 0.13, 0.30], color);
    ellipsoid(mesh, at(0.0, 0.0, 0.88), [0.18, 0.12, 0.14], dark);
    cylinder(mesh, at(0.0, 0.0, 1.37), at(0.0, 0.0, 1.50), [0.055; 2], skin);
    ellipsoid(mesh, at(0.0, 0.0, 1.61), [0.12, 0.11, 0.15], skin);
    for sign in [-1.0, 1.0] {
        cylinder(mesh, at(sign * 0.10, 0.0, 0.83), at(sign * 0.13, 0.0, 0.10), [0.075, 0.055], dark);
        ellipsoid(mesh, at(sign * 0.13, -0.045, 0.065), [0.075, 0.14, 0.065], [39, 43, 48]);
        cylinder(mesh, at(sign * 0.20, 0.0, 1.34), at(sign * 0.29, -0.015, 0.91), [0.065, 0.045], color);
        ellipsoid(mesh, at(sign * 0.29, -0.015, 0.86), [0.045, 0.04, 0.07], skin);
    }
}

/// The stage the program draws when no venue has been imported: an auditorium floor, a raised
/// deck with a lectern and two award marks, people standing at them, and a back wall with a screen.
/// Neutral materials and rounded subjects give the preview scale and readable silhouettes.
pub fn builtin() -> Mesh {
    let mut m = Mesh::default();
    let top = |m: &mut Mesh, min: [f32; 2], max: [f32; 2], z: f32, color: [u8; 3]| {
        m.push_quad([[min[0], min[1], z], [max[0], min[1], z], [max[0], max[1], z], [min[0], max[1], z]], color);
    };
    // Auditorium floor in two tones, so distance reads without a texture.
    for i in 0..14 {
        let y = -5.0 + i as f32;
        let color = if i % 2 == 0 { [44, 44, 44] } else { [36, 36, 36] };
        m.push_cuboid([-14.0, y, -0.025], [14.0, y + 1.0, 0.0], color);
    }
    // The deck, its riser, and the wall behind it.
    m.push_cuboid([-10.0, DECK_FRONT, 0.0], [10.0, BACK_WALL, DECK_HEIGHT], [101, 94, 85]);
    m.push_cuboid([-10.0, DECK_FRONT - 0.025, 0.0], [10.0, DECK_FRONT, DECK_HEIGHT - 0.04], [38, 41, 46]);
    m.push_cuboid([-10.0, DECK_FRONT - 0.04, DECK_HEIGHT - 0.04], [10.0, DECK_FRONT, DECK_HEIGHT], [164, 156, 139]);
    for step in 0..3 {
        let rise = (step + 1) as f32 * DECK_HEIGHT / 3.0;
        let front = DECK_FRONT - (3 - step) as f32 * 0.32;
        m.push_cuboid([7.8, front, 0.0], [9.2, DECK_FRONT, rise], [105, 102, 97]);
        m.push_cuboid([7.8, front - 0.01, rise - 0.02], [9.2, front + 0.04, rise], [181, 176, 163]);
    }
    m.push_cuboid([-10.0, BACK_WALL, 0.0], [10.0, BACK_WALL + 0.2, 7.0], [54, 59, 67]);
    m.push_cuboid([-4.12, BACK_WALL - 0.10, 2.48], [4.12, BACK_WALL, 6.32], [24, 27, 33]);
    m.push_cuboid([-4.0, BACK_WALL - 0.12, 2.6], [4.0, BACK_WALL - 0.10, 6.2], [157, 170, 185]);
    for i in -4..=4 {
        let x = i as f32 * 2.5;
        m.push_cuboid([x - 0.03, BACK_WALL - 0.03, DECK_HEIGHT], [x + 0.03, BACK_WALL, 7.0], [84, 84, 84]);
    }
    // Awards: two marks and two people.
    for dx in [-0.55, 0.55] {
        let x = AWARDS[0] + dx;
        top(&mut m, [x - 0.3, AWARDS[1] - 0.3], [x + 0.3, AWARDS[1] + 0.3], DECK_HEIGHT + 0.01, [210, 210, 210]);
        person(&mut m, x, AWARDS[1], [168, 172, 180]);
    }
    // The lectern, with a presenter behind it.
    person(&mut m, LECTERN[0], LECTERN[1] + 0.45, [186, 164, 150]);
    m.push_cuboid(
        [LECTERN[0] - 0.38, LECTERN[1] - 0.04, DECK_HEIGHT],
        [LECTERN[0] + 0.38, LECTERN[1] + 0.3, DECK_HEIGHT + 1.15],
        [112, 112, 112],
    );
    m.push_cuboid(
        [LECTERN[0] - 0.42, LECTERN[1] - 0.08, DECK_HEIGHT + 1.15],
        [LECTERN[0] + 0.42, LECTERN[1] + 0.34, DECK_HEIGHT + 1.2],
        [168, 168, 168],
    );
    m
}

/// The imported model standing where the stage settings put it.
fn placed(model: &Mesh, settings: &ptz_core::camera::StageModel) -> Mesh {
    let mut out = model.clone();
    for p in &mut out.positions {
        *p = settings.place(p.map(f64::from)).map(|v| v as f32);
    }
    out
}

/// Everything that does not move: the model and, when shown, the built-in stage.
fn statics(model: Option<&Mesh>, stage: &Stage) -> Mesh {
    let mut mesh = Mesh::default();
    if let (Some(model), Some(settings)) = (model, &stage.model) {
        mesh = placed(model, settings);
    }
    if stage.show_builtin {
        mesh.append(&builtin());
    }
    mesh
}

struct Built {
    stage: Stage,
    mesh: Arc<Mesh>,
    scene: Arc<Scene>,
    picker: Arc<Picker>,
    bounds: Option<([f32; 3], [f32; 3])>,
}

fn build(model: Option<Arc<Mesh>>, stage: Stage) -> Built {
    let mesh = statics(model.as_deref(), &stage);
    let bounds = mesh.bounds().map(|b| (b.min, b.max));
    Built { scene: Arc::new(Scene::new(&mesh)), picker: Arc::new(Picker::new(&mesh)), mesh: Arc::new(mesh), bounds, stage }
}

/// How a camera's lens looks at the world for a pose.
pub fn view_of(camera: &CameraConfig, pose: &Pose) -> Camera {
    look_from(camera, pose, camera.mount.lens_position(pose.elevation))
}

/// The same view with the lens at a measured column height, in millimetres above the floor.
pub fn view_at_height(camera: &CameraConfig, pose: &Pose, height_mm: f64) -> Camera {
    look_from(camera, pose, camera.mount.lens_at_height(height_mm))
}

/// What the stage and the monitors use: a column with a height map stands at that measured
/// height, and 0 to 100 is a share of it. Anything else keeps the mount's own base height.
pub fn show_view(project: &Project, camera: &CameraConfig, pose: &Pose) -> Camera {
    match project.column_mm(camera, pose.elevation) {
        Some(mm) => view_at_height(camera, pose, mm),
        None => view_of(camera, pose),
    }
}

fn look_from(camera: &CameraConfig, pose: &Pose, eye: [f64; 3]) -> Camera {
    let (yaw, pitch) = camera.mount.view(pose);
    Camera::look(eye.map(|v| v as f32), yaw as f32, pitch as f32, camera.lens.hfov_deg(pose.zoom) as f32)
}

/// Distinct from the housing and trim so both renderers can give the optical glass its own material.
const CAMERA_GLASS: [u8; 3] = [32, 71, 96];

/// A rounded PTZ head with a lens barrel, tilt yoke and pedestal or ceiling suspension.
/// Only the head tilts; the yoke pans and the support stays fixed at the attachment.
#[cfg(test)]
pub fn marker(mount: &Mount, pose: &Pose, color: [u8; 3]) -> Mesh {
    marker_at(mount, pose, color, None)
}

/// `height_mm` is the lens height above the floor. Without it the mount's base height is used.
pub fn marker_at(mount: &Mount, pose: &Pose, color: [u8; 3], height_mm: Option<f64>) -> Mesh {
    let eye = match height_mm {
        Some(mm) => mount.lens_at_height(mm),
        None => mount.lens_position(pose.elevation),
    };
    let [x, y, lens_z] = eye.map(|v| v as f32);
    let attachment = mount.z as f32;
    let direction = if mount.inverted { -1.0 } else { 1.0 };
    let mut m = Mesh::default();
    cylinder(&mut m, [x, y, attachment], [x, y, attachment + direction * 0.06], [0.30, 0.27], [53, 58, 66]);
    let support_end = lens_z - direction * 0.30;
    if (support_end - attachment) * direction > 0.06 {
        cylinder(&mut m, [x, y, attachment + direction * 0.06], [x, y, support_end], [0.055; 2], [121, 128, 139]);
    }

    let (yaw, pitch) = mount.view(pose);
    let basis = |pitch: f32| {
        let view = Camera::look([x, y, lens_z], yaw as f32, pitch, 60.0);
        Transform { m: [0, 1, 2].map(|k| [view.right[k] * direction, view.forward[k], view.up[k] * direction]), t: view.eye }
    };
    let mut yoke = Mesh::default();
    cylinder(&mut yoke, [0.0, 0.0, -0.30], [0.0, 0.0, -0.23], [0.18, 0.20], [39, 43, 50]);
    cylinder(&mut yoke, [0.0, 0.0, -0.235], [0.0, 0.0, -0.215], [0.20; 2], color);
    yoke.push_cuboid([-0.25, -0.10, -0.22], [0.25, 0.10, -0.16], [47, 52, 61]);
    for side in [-1.0, 1.0] {
        let cx = side * 0.22;
        yoke.push_cuboid([cx - 0.03, -0.10, -0.18], [cx + 0.03, 0.10, 0.025], [47, 52, 61]);
        cylinder(&mut yoke, [side * 0.18, 0.0, 0.0], [side * 0.265, 0.0, 0.0], [0.075; 2], color);
    }
    m.append(&yoke.transformed(&basis(0.0)));

    let mut head = Mesh::default();
    ellipsoid(&mut head, [0.0, -0.025, 0.0], [0.185, 0.21, 0.16], [65, 72, 83]);
    cylinder(&mut head, [0.0, 0.10, 0.0], [0.0, 0.30, 0.0], [0.105, 0.115], [31, 35, 42]);
    cylinder(&mut head, [0.0, 0.30, 0.0], [0.0, 0.335, 0.0], [0.115; 2], [130, 141, 154]);
    cylinder(&mut head, [0.0, 0.335, 0.0], [0.0, 0.35, 0.0], [0.098; 2], [17, 23, 31]);
    cylinder(&mut head, [0.0, 0.351, 0.0], [0.0, 0.355, 0.0], [0.082; 2], CAMERA_GLASS);
    cylinder(&mut head, [-0.025, 0.356, 0.03], [-0.025, 0.357, 0.03], [0.017; 2], [114, 161, 177]);
    head.push_cuboid([-0.025, 0.035, 0.145], [0.025, 0.07, 0.165], color);
    m.append(&head.transformed(&basis(pitch as f32)));
    m
}

pub fn rgb(color: egui::Color32) -> [u8; 3] {
    [color.r(), color.g(), color.b()]
}

/// A hash of anything that decides what a picture looks like, so it is only drawn again when that changes.
pub struct Key(std::collections::hash_map::DefaultHasher);

impl Key {
    pub fn new() -> Key {
        Key(std::collections::hash_map::DefaultHasher::new())
    }

    pub fn add(mut self, value: impl Hash) -> Key {
        value.hash(&mut self.0);
        self
    }

    pub fn floats(mut self, values: &[f64]) -> Key {
        for v in values {
            v.to_bits().hash(&mut self.0);
        }
        self
    }

    pub fn camera(self, c: &Camera) -> Key {
        let all = [c.eye, c.right, c.up, c.forward, [c.tan_half_h, c.near, 0.0]];
        self.floats(&all.concat().iter().map(|v| *v as f64).collect::<Vec<_>>())
    }

    pub fn finish(self) -> u64 {
        self.0.finish()
    }
}

struct Picture {
    frame: Frame,
    /// What the frame shows: the key and the size it was asked for.
    content: Option<u64>,
    texture: Option<egui::TextureHandle>,
    /// How long the last drawing at full size took.
    full_ms: f32,
}

/// A model file read from disk, with the unit and up axis its size and kind suggest.
#[derive(Debug)]
pub struct Read {
    pub name: String,
    pub mesh: Mesh,
    pub unit: Unit,
    pub up: UpAxis,
}

pub fn is_model(path: &Path) -> bool {
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    matches!(extension.as_deref(), Some("obj" | "3ds"))
}

/// Read an OBJ or 3DS file, with the material file beside it when it names one.
pub fn read_model(path: &Path) -> Result<Read, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
    let size = std::fs::metadata(path).map_err(|e| format!("it could not be opened: {e}"))?.len();
    if size > LARGEST_FILE {
        return Err(format!(
            "it is {} MB, and the largest model that can be read is {} MB. Export a lighter version",
            size / (1024 * 1024),
            LARGEST_FILE / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("it could not be read: {e}"))?;
    let folder = path.parent();
    // A model names its material file; only one in the model's own folder is opened.
    let sibling = |file: &str| -> Option<Vec<u8>> {
        let file = Path::new(file.trim()).file_name()?;
        std::fs::read(folder?.join(file)).ok()
    };
    let mesh = ptz_scene::import(&name, &bytes, &sibling)?;
    let bounds = mesh.bounds().ok_or("it has no triangles")?;
    let largest = bounds.size().into_iter().fold(0.0f32, f32::max) as f64;
    if !(largest.is_finite() && largest > 0.0) {
        return Err("it has no size".into());
    }
    // OBJ files are usually written Y up, 3DS files Z up.
    let up = if name.to_ascii_lowercase().ends_with(".obj") { UpAxis::Y } else { UpAxis::Z };
    Ok(Read { name, mesh, unit: Unit::guess(largest), up })
}

/// What an import is doing, for the line under the Import button.
#[derive(Clone, Debug, PartialEq)]
pub enum Import {
    Reading(String),
    Uploading(String),
    Done(String),
    Failed { name: String, why: String },
}

pub struct World {
    /// Native studio renderer, available only while the window's GL context is alive.
    gpu: Option<crate::stage_gpu::Renderer>,
    render_error: Option<String>,
    /// The model as the server holds it, in the file's own units and axes, with its revision.
    model: Option<(u64, Arc<Mesh>)>,
    /// The revision that has been asked for and has not arrived.
    asked: Option<u64>,
    /// After a failed fetch, when to ask again.
    retry_at: Option<Instant>,
    built: Built,
    building: Option<(Stage, Receiver<Built>)>,
    /// Goes up whenever the scene changes, so pictures know to be drawn again.
    pub generation: u64,
    pictures: HashMap<String, Picture>,
    pub import: Option<Import>,
    /// Milliseconds the last pictures took to draw, by name, for the read-out and the tests.
    pub timings: HashMap<String, f32>,
    /// Drawing time used in this window frame.
    spent_ms: f32,
    /// A picture that may wait has been drawn in this window frame.
    drew_one: bool,
    /// True when the last call to `picture` drew.
    pub drew: bool,
    /// The Multiview tile whose turn it is to be drawn first.
    pub turn: usize,
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    pub fn new() -> World {
        World {
            gpu: None,
            render_error: None,
            model: None,
            asked: None,
            retry_at: None,
            built: build(None, Stage::default()),
            building: None,
            generation: 1,
            pictures: HashMap::new(),
            import: None,
            timings: HashMap::new(),
            spent_ms: 0.0,
            drew_one: false,
            drew: false,
            turn: 0,
        }
    }

    pub fn picker(&self) -> &Picker {
        &self.built.picker
    }

    /// Called on the window thread with its OpenGL context current.
    pub fn enable_gpu(&mut self, gl: Arc<eframe::glow::Context>) -> Result<(), String> {
        self.disable_gpu();
        self.gpu = Some(crate::stage_gpu::Renderer::new(gl)?);
        self.generation += 1;
        Ok(())
    }

    /// Release GL resources before eframe destroys the window context.
    pub fn disable_gpu(&mut self) {
        if let Some(mut gpu) = self.gpu.take() {
            gpu.destroy();
            self.generation += 1;
        }
    }

    pub fn take_render_error(&mut self) -> Option<String> {
        self.render_error.take()
    }

    /// Render the same scene through native studio shading or the CPU fallback.
    pub fn scene_picture(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        size: [usize; 2],
        key: u64,
        urgent: bool,
        view: &Camera,
        make_markers: impl FnOnce() -> Vec<Mesh>,
        hidden_marker: Option<usize>,
        lines: &[crate::stage_gpu::Line],
        look: &Look,
    ) -> Option<egui::TextureId> {
        let mut gpu = self.gpu.take();
        let mesh = self.built.mesh.clone();
        let generation = self.generation;
        let mut error = None;
        let picture = self.picture(ctx, name, size, key, urgent, |frame, scene| {
            let markers = make_markers();
            if let Some(renderer) = &mut gpu {
                // All views share the same shadow casters, including the camera
                // whose body is hidden from its own simulated picture.
                let visible;
                let shown = if let Some(hidden) = hidden_marker {
                    visible =
                        markers.iter().enumerate().filter(|(i, _)| *i != hidden).map(|(_, m)| m.clone()).collect::<Vec<_>>();
                    &visible
                } else {
                    &markers
                };
                match renderer.render_with_shadow_casters(frame, generation, &mesh, view, shown, &markers, lines, look) {
                    Ok(()) => return,
                    Err(why) => error = Some(why),
                }
                // A failed draw may have partially filled the picture.
                frame.clear(SKY);
            }
            render::draw_scene(frame, view, scene, look);
            for (i, marker) in markers.iter().enumerate() {
                if hidden_marker != Some(i) {
                    render::draw_mesh(frame, view, marker, look);
                }
            }
            for (a, b, color, width) in lines {
                render::draw_line(frame, view, *a, *b, *color, *width);
            }
        });
        self.gpu = gpu;
        if let Some(error) = error {
            self.disable_gpu();
            self.render_error = Some(error);
            ctx.request_repaint();
        }
        picture
    }

    /// Lowest and highest corner of everything that stands still.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.built.bounds
    }

    pub fn triangles(&self) -> usize {
        self.built.scene.triangle_count()
    }

    /// The model revision the app holds.
    pub fn model_revision(&self) -> Option<u64> {
        self.model.as_ref().map(|m| m.0)
    }

    /// The server's model has arrived, or the app has just uploaded it.
    pub fn set_model(&mut self, revision: u64, mesh: Arc<Mesh>) {
        if self.asked == Some(revision) {
            self.asked = None;
        }
        self.model = Some((revision, mesh));
    }

    /// A fetch failed: ask again in a few seconds.
    pub fn fetch_failed(&mut self, revision: u64) {
        if self.asked == Some(revision) {
            self.asked = None;
            self.retry_at = Some(Instant::now() + Duration::from_secs(3));
        }
    }

    /// True while the model named by the project has not arrived yet.
    pub fn waiting_for_model(&self, stage: &Stage) -> bool {
        stage.model.as_ref().is_some_and(|m| self.model_revision() != Some(m.revision))
    }

    pub fn is_building(&self) -> bool {
        self.building.is_some()
    }

    /// True while the scene is not yet what the project asks for, so the window keeps looking.
    pub fn is_busy(&self, stage: &Stage) -> bool {
        self.is_building() || self.waiting_for_model(stage)
    }

    /// Bring the scene in line with the project's stage, once per window frame.
    /// Returns the revision to fetch, when one is needed.
    pub fn sync(&mut self, stage: &Stage) -> Option<u64> {
        self.spent_ms = 0.0;
        self.drew_one = false;
        let mut fetch = None;
        let may_ask = self.retry_at.is_none_or(|at| Instant::now() >= at);
        match &stage.model {
            None => {
                self.model = None;
                self.asked = None;
                self.retry_at = None;
            }
            Some(wanted) if may_ask && self.model_revision() != Some(wanted.revision) && self.asked != Some(wanted.revision) => {
                self.asked = Some(wanted.revision);
                self.retry_at = None;
                fetch = Some(wanted.revision);
            }
            Some(_) => {}
        }
        if let Some((_, done)) = &self.building {
            match done.try_recv() {
                Ok(built) => {
                    self.built = built;
                    self.building = None;
                    self.generation += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.building = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        // Until the model arrives the scene is built from what there is, and built again when it does.
        let model = match (&self.model, &stage.model) {
            (Some((revision, mesh)), Some(wanted)) if *revision == wanted.revision => Some(mesh.clone()),
            _ => None,
        };
        let mut want = stage.clone();
        if model.is_none() {
            want.model = None;
        }
        let pending = self.building.as_ref().map(|b| &b.0);
        if self.built.stage != want && pending != Some(&want) {
            let triangles = model.as_ref().map(|m| m.triangles.len()).unwrap_or(0);
            if triangles < BUILD_INLINE_BELOW {
                self.built = build(model, want);
                self.building = None;
                self.generation += 1;
            } else {
                let (tx, rx) = channel();
                let stage = want.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(build(model, stage));
                });
                self.building = Some((want, rx));
            }
        }
        fetch
    }

    /// Distance from a camera's lens to what the centre of its picture rests on.
    pub fn centre_distance(&self, camera: &CameraConfig, pose: &Pose) -> Option<f32> {
        self.distance_along(&view_of(camera, pose))
    }

    /// How far the centre of a view goes before it meets the stage.
    pub fn distance_along(&self, view: &Camera) -> Option<f32> {
        self.picker().hit(view.eye, view.forward).map(|h| h.distance)
    }

    /// The point of the stage under a ray: the model or built-in stage where it is hit, else the floor.
    pub fn ground(&self, origin: [f32; 3], direction: [f32; 3]) -> Option<[f32; 3]> {
        if let Some(hit) = self.picker().hit(origin, direction) {
            return Some(hit.point);
        }
        on_plane(origin, direction, 0.0)
    }

    /// The picture under `name`, drawn again only when `key` has changed. `draw` receives a cleared frame.
    ///
    /// A picture that is slow is drawn at half size while it changes and at full size once it rests.
    /// One that is not `urgent` waits its turn when this window frame has already spent its drawing time,
    /// and then the picture drawn before is returned: None when there is none yet.
    pub fn picture(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        size: [usize; 2],
        key: u64,
        urgent: bool,
        draw: impl FnOnce(&mut Frame, &Scene),
    ) -> Option<egui::TextureId> {
        self.drew = false;
        let scene = self.built.scene.clone();
        let full = fit(size);
        let content = Key::new().add(key).add(full).add(self.generation).finish();
        let picture = self.pictures.entry(name.to_string()).or_insert_with(|| Picture {
            frame: Frame::new(full[0], full[1]),
            content: None,
            texture: None,
            full_ms: 0.0,
        });
        let same = picture.content == Some(content) && picture.texture.is_some();
        if same && [picture.frame.width, picture.frame.height] == full {
            return picture.texture.as_ref().map(|t| t.id());
        }
        if !urgent && self.drew_one && self.spent_ms >= BUDGET_MS {
            ctx.request_repaint();
            return picture.texture.as_ref().map(|t| t.id());
        }
        let size = if !same && picture.full_ms > SLOW_MS { [(full[0] / 2).max(16), (full[1] / 2).max(9)] } else { full };
        let started = Instant::now();
        picture.frame.resize(size[0], size[1]);
        picture.frame.clear(SKY);
        draw(&mut picture.frame, &scene);
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &picture.frame.rgba);
        match &mut picture.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => picture.texture = Some(ctx.load_texture(format!("stage:{name}"), image, egui::TextureOptions::LINEAR)),
        }
        picture.content = Some(content);
        let took = started.elapsed().as_secs_f32() * 1000.0;
        if size == full {
            picture.full_ms = took;
        } else {
            // Sharpened on the next frame in which it has not changed.
            ctx.request_repaint();
        }
        self.spent_ms += took;
        self.drew_one |= !urgent;
        self.drew = true;
        self.timings.insert(name.to_string(), took);
        picture.texture.as_ref().map(|t| t.id())
    }

    /// True when the picture under `name` is already drawn for `key` at `size`.
    #[cfg(test)]
    pub fn is_fresh(&self, name: &str, size: [usize; 2], key: u64) -> bool {
        let full = fit(size);
        let content = Key::new().add(key).add(full).add(self.generation).finish();
        self.pictures
            .get(name)
            .is_some_and(|p| p.content == Some(content) && p.texture.is_some() && [p.frame.width, p.frame.height] == full)
    }

    /// The pixels last drawn under `name`, for the tests.
    #[cfg(test)]
    pub fn frame(&self, name: &str) -> Option<&Frame> {
        self.pictures.get(name).map(|p| &p.frame)
    }

    /// Forget pictures that are no longer shown, such as those of presets that were deleted.
    pub fn keep_pictures(&mut self, keep: impl Fn(&str) -> bool) {
        self.pictures.retain(|name, _| keep(name));
    }
}

fn fit(size: [usize; 2]) -> [usize; 2] {
    [size[0].clamp(16, 1920), size[1].clamp(9, 1080)]
}

/// The size in pixels to draw a picture shown in `rect`, no wider than `cap`.
pub fn pixel_size(rect: egui::Rect, pixels_per_point: f32, cap: f32) -> [usize; 2] {
    let scale = pixels_per_point.min(cap / rect.width().max(1.0)).max(0.05);
    [(rect.width() * scale).round().max(1.0) as usize, (rect.height() * scale).round().max(1.0) as usize]
}

/// Where a ray meets the horizontal plane at height `z`, when it does in front of its origin.
pub fn on_plane(origin: [f32; 3], direction: [f32; 3], z: f32) -> Option<[f32; 3]> {
    if direction[2].abs() < 1e-6 {
        return None;
    }
    let t = (z - origin[2]) / direction[2];
    (t > 0.0 && t.is_finite()).then(|| [origin[0] + direction[0] * t, origin[1] + direction[1] * t, z])
}

/// The heading that makes pan 0 face a point from where the camera stands.
pub fn heading_toward(mount: &Mount, point: [f32; 3]) -> f64 {
    let (dx, dy) = (point[0] as f64 - mount.x, point[1] as f64 - mount.y);
    if dx.hypot(dy) < 1e-6 {
        return mount.heading_deg;
    }
    (-dx).atan2(dy).to_degrees()
}

fn fog() -> Look {
    Look { fog_color: SKY, fog_start: 18.0, fog_end: 160.0, fog_strength: 0.6 }
}

/// What camera `index` sees at `poses[index]`, with the other cameras standing at theirs, as the picture kept under `name`.
pub fn camera_picture(
    ctx: &egui::Context,
    s: &mut crate::session::Session,
    name: &str,
    index: usize,
    poses: &[Pose],
    size: [usize; 2],
    urgent: bool,
) -> Option<egui::TextureId> {
    let camera = s.project.cameras.get(index)?;
    let view = show_view(&s.project, camera, poses.get(index)?);
    let key = camera_key(&s.project, poses, index, &view);
    let project = &s.project;
    s.world.scene_picture(
        ctx,
        name,
        size,
        key,
        urgent,
        &view,
        || {
            project
                .cameras
                .iter()
                .enumerate()
                .map(|(i, camera)| {
                    let pose = poses.get(i).copied().unwrap_or_else(Pose::home);
                    marker_at(&camera.mount, &pose, rgb(crate::theme::camera_color(i)), project.column_mm(camera, pose.elevation))
                })
                .collect()
        },
        Some(index),
        &[],
        &fog(),
    )
}

/// Paint a picture made by `World::picture`, or the empty sky while it has not been drawn yet.
pub fn paint_picture(painter: &egui::Painter, rect: egui::Rect, texture: Option<egui::TextureId>) {
    match texture {
        Some(texture) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(texture, rect, uv, egui::Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(SKY[0], SKY[1], SKY[2]));
        }
    }
}

/// What one camera sees: the stage, and the other cameras standing in it.
#[cfg(test)]
pub fn draw_camera_view(frame: &mut Frame, scene: &Scene, project: &Project, poses: &[Pose], index: usize, view: &Camera) {
    let look = fog();
    render::draw_scene(frame, view, scene, &look);
    for (i, camera) in project.cameras.iter().enumerate() {
        if i != index {
            let pose = poses.get(i).copied().unwrap_or_else(Pose::home);
            render::draw_mesh(frame, view, &marker(&camera.mount, &pose, rgb(crate::theme::camera_color(i))), &look);
        }
    }
}

/// The key of a camera's picture: its view and every camera body that can cast a shadow.
pub fn camera_key(project: &Project, poses: &[Pose], index: usize, view: &Camera) -> u64 {
    let mut key = Key::new().camera(view).add(index);
    for (i, camera) in project.cameras.iter().enumerate() {
        let pose = poses.get(i).copied().unwrap_or_else(Pose::home);
        let m = &camera.mount;
        key = key.add(m.inverted).floats(&[
            m.x,
            m.y,
            m.z,
            m.heading_deg,
            m.tilt_deg,
            m.base_height_mm,
            pose.pan,
            pose.tilt,
            pose.elevation,
        ]);
    }
    key.finish()
}

/// The corners of a camera's picture `reach` metres out along its view: top left, top right, bottom right, bottom left.
pub fn frustum_corners(view: &Camera, reach: f32) -> [[f32; 3]; 4] {
    let (half_w, half_h) = (view.tan_half_h * reach, view.tan_half_h * reach * 9.0 / 16.0);
    let at = |across: f32, upward: f32| {
        [0, 1, 2].map(|k| view.eye[k] + view.forward[k] * reach + view.right[k] * across + view.up[k] * upward)
    };
    [at(-half_w, half_h), at(half_w, half_h), at(half_w, -half_h), at(-half_w, -half_h)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptz_core::camera::{StageModel, Unit, UpAxis};
    use ptz_core::lens::Lens;

    fn first() -> CameraConfig {
        Project::default().cameras[0].clone()
    }

    #[test]
    fn the_built_in_stage_is_where_the_seeded_presets_look() {
        let camera = first();
        // Panning to the lectern centres it; with no pan it is right of centre; positive pan looks left.
        let mut pose = Pose::home();
        pose.pan = -(LECTERN[0] / LECTERN[1]).atan().to_degrees() as f64;
        let view = view_of(&camera, &pose);
        let p = view.project([LECTERN[0], LECTERN[1], 1.5], 960, 540).unwrap();
        assert!((p[0] - 480.0).abs() < 1.0, "{p:?}");
        let home = view_of(&camera, &Pose::home());
        assert!(home.project([LECTERN[0], LECTERN[1], 1.5], 960, 540).unwrap()[0] > 600.0);
        pose.pan = -(AWARDS[0] / AWARDS[1]).atan().to_degrees() as f64;
        assert!(pose.pan > 0.0);
        let view = view_of(&camera, &pose);
        assert!((view.project([AWARDS[0], AWARDS[1], 1.5], 960, 540).unwrap()[0] - 480.0).abs() < 1.0);
        // The centre of the home picture rests on something a believable distance away.
        let world = World::new();
        let d = world.centre_distance(&camera, &Pose::home()).unwrap();
        assert!((15.9..16.1).contains(&d), "the back wall is 16 m away: {d}");
    }

    #[test]
    fn tilt_zoom_elevation_and_the_lens_move_the_picture_the_right_way() {
        let camera = first();
        let at = |camera: &CameraConfig, pose: &Pose| view_of(camera, pose).project([1.0, 12.0, 2.0], 960, 540).unwrap();
        let base = Pose::home();
        let p0 = at(&camera, &base);
        let up = Pose { tilt: base.tilt + 5.0, ..base };
        assert!(at(&camera, &up)[1] > p0[1], "tilting up moves the subject down the frame");
        let tight = Pose { zoom: base.zoom + 0.3, ..base };
        assert!(at(&camera, &tight)[0] - 480.0 > p0[0] - 480.0, "zooming in spreads points from the centre");
        let high = Pose { elevation: base.elevation + 400.0, ..base };
        assert!(at(&camera, &high)[1] > p0[1], "raising the camera moves the subject down the frame");

        // A point on the edge of the frame the lens maths gives lands on the edge of the picture.
        for (lens, zoom) in [(Lens::default(), 0.0), (Lens::default(), 0.5), (Lens::presets()[3].clone(), 1.0)] {
            let camera = CameraConfig { lens, ..first() };
            let pose = Pose { zoom, elevation: 0.0, ..Pose::home() };
            let (w, h) = camera.lens.frame_at(zoom, 10.0);
            let eye = camera.mount.lens_position(0.0);
            let corner = [eye[0] + w / 2.0, eye[1] + 10.0, eye[2] + h / 2.0].map(|v| v as f32);
            let p = view_of(&camera, &pose).project(corner, 960, 540).unwrap();
            assert!((p[0] - 960.0).abs() < 0.5 && p[1].abs() < 0.5, "{} at zoom {zoom}: {p:?}", camera.lens.name);
        }

        // A camera moved back sees the same thing smaller by the ratio of the distances.
        let size = |camera: &CameraConfig| {
            let view = view_of(camera, &Pose::home());
            let (a, b) = (view.project([-1.0, 12.0, 1.2], 960, 540).unwrap(), view.project([1.0, 12.0, 1.2], 960, 540).unwrap());
            b[0] - a[0]
        };
        let mut back = first();
        back.mount.y = -5.0;
        let ratio = size(&first()) / size(&back);
        assert!((ratio - 17.0 / 12.0).abs() < 1e-3, "{ratio}");
        // Turned a quarter left, what was ahead is now off to the right.
        let mut turned = first();
        turned.mount.heading_deg = 90.0;
        assert!(view_of(&turned, &Pose::home()).project([0.0, 12.0, 1.2], 960, 540).is_none_or(|p| p[0] > 960.0));
    }

    #[test]
    fn the_scene_follows_the_stage_settings_and_asks_for_a_model_once() {
        let mut world = World::new();
        let built_in = world.triangles();
        assert!(built_in > 100);
        let start = world.generation;
        assert_eq!(world.sync(&Stage::default()), None);
        assert_eq!(world.generation, start, "nothing changed, nothing is rebuilt");

        // A hall of 40 by 25 by 9 metres, written in millimetres with Y up.
        let mut hall = Mesh::default();
        hall.push_cuboid([0.0, 0.0, -25_000.0], [40_000.0, 9_000.0, 0.0], [90, 90, 100]);
        let settings = StageModel {
            name: "hall.obj".into(),
            revision: 3,
            triangles: 12,
            min: [0.0, 0.0, -25_000.0],
            max: [40_000.0, 9_000.0, 0.0],
            unit: Unit::Mm,
            up: UpAxis::Y,
            ..StageModel::default()
        };
        let mut stage = Stage { show_builtin: false, model: Some(settings) };
        assert_eq!(world.sync(&stage), Some(3), "the model is asked for");
        assert_eq!(world.sync(&stage), None, "and only once");
        assert!(world.waiting_for_model(&stage));
        assert_eq!(world.triangles(), 0, "without the built-in stage there is nothing to draw until it arrives");

        world.set_model(3, Arc::new(hall));
        world.sync(&stage);
        assert!(!world.waiting_for_model(&stage));
        assert_eq!(world.triangles(), 12);
        let (min, max) = world.bounds().unwrap();
        assert_eq!((min, max), ([0.0, 0.0, 0.0], [40.0, 25.0, 9.0]));

        // Moving it is a setting: the scene is rebuilt, and nothing is fetched.
        let model = stage.model.as_mut().unwrap();
        model.centre();
        model.offset[2] = 1.0;
        assert_eq!(world.sync(&stage), None);
        assert_eq!(world.bounds().unwrap(), ([-20.0, -12.5, 1.0], [20.0, 12.5, 10.0]));
        stage.show_builtin = true;
        world.sync(&stage);
        assert_eq!(world.triangles(), 12 + built_in);

        // A ray down from above lands on the hall's roof; outside the hall it lands on the floor.
        assert_eq!(world.ground([0.0, 0.0, 50.0], [0.0, 0.0, -1.0]).unwrap()[2], 10.0);
        assert_eq!(world.ground([100.0, 0.0, 50.0], [0.0, 0.0, -1.0]).unwrap(), [100.0, 0.0, 0.0]);
        assert_eq!(world.ground([100.0, 0.0, 50.0], [0.0, 0.0, 1.0]), None);

        // Removing the model drops it; a new upload has a new revision and is asked for again.
        stage.model = None;
        world.sync(&stage);
        assert_eq!((world.triangles(), world.model_revision()), (built_in, None));
    }

    #[test]
    fn a_marker_stands_on_its_floor_point_with_its_head_at_lens_height_facing_where_the_camera_looks() {
        let mount = Mount { x: 3.0, y: -2.0, z: 0.5, heading_deg: 90.0, base_height_mm: 1200.0, ..Mount::default() };
        let pose = Pose { elevation: 300.0, ..Pose::home() };
        let m = marker(&mount, &pose, [1, 2, 3]);
        let b = m.bounds().unwrap();
        assert!((b.min[2] - 0.5).abs() < 1e-6, "{b:?}");
        assert!((b.max[2] - (0.5 + 1.5 + 0.165)).abs() < 1e-5, "{b:?}");
        // Heading 90 looks along -X: the lens barrel is the part furthest that way.
        assert!(b.min[0] < 3.0 - 0.35 && (b.max[0] - (3.0 + 0.3)).abs() < 1e-5, "{b:?}");
        assert_eq!(heading_toward(&mount, [3.0, 10.0, 0.0]), 0.0);
        assert!((heading_toward(&mount, [-7.0, -2.0, 0.0]) - 90.0).abs() < 1e-9);
        assert!((heading_toward(&mount, [13.0, -2.0, 0.0]) + 90.0).abs() < 1e-9);
    }

    #[test]
    fn an_inverted_marker_hangs_from_its_attachment_and_its_barrel_tracks_the_view() {
        let mut camera = first();
        camera.mount = Mount { x: 3.0, y: -2.0, z: 6.0, inverted: true, ..Mount::default() };
        let pose = Pose { elevation: 300.0, ..Pose::home() };
        let m = marker(&camera.mount, &pose, [1, 2, 3]);
        let bounds = m.bounds().unwrap();
        assert!((bounds.max[2] - 6.0).abs() < 1e-6, "the suspension ends at the ceiling: {bounds:?}");
        assert!((bounds.min[2] - 4.335).abs() < 1e-5, "the head hangs at lens height: {bounds:?}");
        assert!(m.validate().is_ok());

        // The optical glass must stay centred on the view ray when either mounting
        // angles or pose change, including a hanging head looking exactly up or down.
        for inverted in [false, true] {
            camera.mount.inverted = inverted;
            for (heading, mount_tilt) in [(0.0, 0.0), (35.0, -20.0), (-110.0, 35.0)] {
                camera.mount.heading_deg = heading;
                camera.mount.tilt_deg = mount_tilt;
                for (pan, pitch) in [(0.0, -90.0), (-80.0, 0.0), (45.0, 90.0), (20.0, 25.0)] {
                    let pose = Pose { pan, tilt: pitch - mount_tilt, ..pose };
                    let view = view_of(&camera, &pose);
                    let m = marker(&camera.mount, &pose, [1, 2, 3]);
                    let glass = points_with_color(&m, CAMERA_GLASS);
                    assert!(!glass.is_empty());
                    let centre = [0, 1, 2].map(|k| {
                        (glass.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min)
                            + glass.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max))
                            * 0.5
                    });
                    for (k, &coordinate) in centre.iter().enumerate() {
                        assert!((coordinate - view.eye[k] - 0.353 * view.forward[k]).abs() < 1e-5);
                    }
                    let projected = view.project(centre, 960, 540).unwrap();
                    assert!((projected[0] - 480.0).abs() < 0.02 && (projected[1] - 270.0).abs() < 0.02, "{projected:?}");
                    assert!(m.validate().is_ok());
                }
            }
        }
    }

    fn points_with_color(mesh: &Mesh, color: [u8; 3]) -> Vec<[f32; 3]> {
        mesh.triangles
            .iter()
            .zip(&mesh.colors)
            .filter(|(_, c)| **c == color)
            .flat_map(|(triangle, _)| triangle.map(|index| mesh.positions[index as usize]))
            .collect()
    }

    #[test]
    fn the_yoke_and_support_stay_put_when_the_head_tilts_but_the_yoke_follows_pan() {
        for inverted in [false, true] {
            let mount = Mount { z: if inverted { 5.0 } else { 0.0 }, inverted, heading_deg: 27.0, ..Mount::default() };
            let base = marker(&mount, &Pose::home(), [1, 2, 3]);
            for tilt in [-90.0, -35.0, 25.0, 90.0] {
                let tilted = marker(&mount, &Pose { tilt, ..Pose::home() }, [1, 2, 3]);
                for fixed_color in [[53, 58, 66], [121, 128, 139], [47, 52, 61]] {
                    assert_eq!(points_with_color(&base, fixed_color), points_with_color(&tilted, fixed_color));
                }
                assert_ne!(points_with_color(&base, CAMERA_GLASS), points_with_color(&tilted, CAMERA_GLASS));
            }
            let panned = marker(&mount, &Pose { pan: 55.0, ..Pose::home() }, [1, 2, 3]);
            assert_ne!(points_with_color(&base, [47, 52, 61]), points_with_color(&panned, [47, 52, 61]));
            assert_eq!(points_with_color(&base, [121, 128, 139]), points_with_color(&panned, [121, 128, 139]));
        }
    }

    #[test]
    fn rounded_primitives_are_closed_non_degenerate_and_wound_outward() {
        let mut round = Mesh::default();
        let centre = [1.0, 2.0, 3.0];
        ellipsoid(&mut round, centre, [0.3, 0.18, 0.5], [1, 2, 3]);
        let mut tube = Mesh::default();
        let (from, to) = ([-1.0, 0.3, 2.0], [0.5, 1.1, 4.0]);
        cylinder(&mut tube, from, to, [0.2, 0.3], [1, 2, 3]);
        for (mesh, inside) in [(round, centre), (tube, [0, 1, 2].map(|k| (from[k] + to[k]) * 0.5))] {
            assert!(mesh.validate().is_ok());
            let mut edges = std::collections::HashMap::<([u32; 3], [u32; 3]), (usize, i32)>::new();
            for triangle in &mesh.triangles {
                let [a, b, c] = triangle.map(|i| mesh.positions[i as usize]);
                let ab = [0, 1, 2].map(|k| b[k] - a[k]);
                let ac = [0, 1, 2].map(|k| c[k] - a[k]);
                let n = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
                assert!(n.iter().map(|v| v * v).sum::<f32>() > 1e-12, "degenerate triangle");
                assert!((0..3).map(|k| n[k] * (a[k] - inside[k])).sum::<f32>() > 0.0, "inward-facing triangle");
                let point_key = |p: [f32; 3]| p.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
                for (a, b) in [(a, b), (b, c), (c, a)] {
                    let (a, b) = (point_key(a), point_key(b));
                    let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                    let entry = edges.entry(edge).or_default();
                    entry.0 += 1;
                    entry.1 += sign;
                }
            }
            assert!(edges.values().all(|&(count, winding)| count == 2 && winding == 0), "open seam or mismatched winding");
        }
    }

    #[test]
    fn detailed_models_stay_within_the_preview_budget_and_keep_subjects_on_their_marks() {
        let stage = builtin();
        assert!(stage.validate().is_ok());
        assert!(stage.triangles.len() < 15_000, "{} stage triangles", stage.triangles.len());
        let camera = marker(&Mount::default(), &Pose::home(), [1, 2, 3]);
        assert!(camera.triangles.len() < 3_000, "{} camera triangles", camera.triangles.len());
        let mut subject = Mesh::default();
        person(&mut subject, AWARDS[0], AWARDS[1], [168, 172, 180]);
        let bounds = subject.bounds().unwrap();
        assert!((bounds.min[2] - DECK_HEIGHT).abs() < 1e-5, "feet meet the stage");
        assert!((bounds.max[2] - DECK_HEIGHT - 1.76).abs() < 1e-5, "subject height stays useful for framing");
        assert!(((bounds.min[0] + bounds.max[0]) * 0.5 - AWARDS[0]).abs() < 1e-5, "subject stays centred on the mark");
    }

    #[test]
    fn an_inverted_camera_projects_its_tilted_view_upright() {
        let camera = CameraConfig { mount: Mount { z: 8.0, inverted: true, tilt_deg: -15.0, ..Mount::default() }, ..first() };
        let pose = Pose { tilt: -15.0, elevation: 600.0, ..Pose::home() };
        let view = view_of(&camera, &pose);
        assert!((view.eye[2] - 6.2).abs() < 1e-6);
        // At a combined -30 degrees the optical axis falls 5 m over a 10 m slant distance.
        let target = [0.0, 75.0f32.sqrt(), 1.2];
        let centre = view.project(target, 960, 540).unwrap();
        assert!((centre[0] - 480.0).abs() < 0.01 && (centre[1] - 270.0).abs() < 0.01, "{centre:?}");
        let above = view.project([target[0], target[1], target[2] + 1.0], 960, 540).unwrap();
        let right = view.project([target[0] + 1.0, target[1], target[2]], 960, 540).unwrap();
        assert!(above[1] < centre[1] && right[0] > centre[0], "image flip keeps up and right on screen");
        let extended = Pose { elevation: 1000.0, ..pose };
        assert!(view_of(&camera, &extended).project(target, 960, 540).unwrap()[1] < centre[1]);
    }

    #[test]
    fn a_model_file_is_read_with_its_material_file_and_a_unit_and_up_axis_are_suggested() {
        let dir = std::env::temp_dir().join(format!("ptz-app-read-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("venue")).unwrap();
        let at = |name: &str| dir.join("venue").join(name);
        let mut hall = Mesh::default();
        hall.push_cuboid([0.0, 0.0, -25_000.0], [40_000.0, 9_000.0, 0.0], [120, 120, 130]);
        hall.push_cuboid([15_000.0, 0.0, -24_000.0], [25_000.0, 1_000.0, -18_000.0], [90, 60, 40]);

        // An OBJ in millimetres: Y up is assumed, the unit comes from its size, the colours from the file beside it.
        let (obj, mtl) = ptz_scene::obj::write(&hall, "hall.mtl");
        std::fs::write(at("Hall A.OBJ"), &obj).unwrap();
        std::fs::write(at("hall.mtl"), &mtl).unwrap();
        let read = read_model(&at("Hall A.OBJ")).unwrap();
        assert_eq!((read.name.as_str(), read.unit, read.up), ("Hall A.OBJ", Unit::Mm, UpAxis::Y));
        assert_eq!(read.mesh, hall);

        // The material file is looked for beside the model only, whatever folder the model names.
        std::fs::write(at("moved.obj"), obj.replace("mtllib hall.mtl", "mtllib C:\\exports\\old\\hall.mtl")).unwrap();
        assert!(obj.contains("mtllib hall.mtl"));
        assert_eq!(read_model(&at("moved.obj")).unwrap().mesh, hall);
        // Without it the model still opens, in a neutral grey.
        std::fs::write(at("bare.obj"), obj.replace("mtllib hall.mtl", "mtllib gone.mtl")).unwrap();
        let bare = read_model(&at("bare.obj")).unwrap().mesh;
        assert_eq!(bare.triangles.len(), hall.triangles.len());
        assert!(bare.colors.iter().all(|c| *c == ptz_scene::mesh::DEFAULT_COLOR));

        // A 3DS in metres: Z up is assumed.
        let mut room = Mesh::default();
        room.push_cuboid([-10.0, 0.0, 0.0], [10.0, 30.0, 6.0], [200, 180, 160]);
        std::fs::write(at("room.3ds"), ptz_scene::tds::write(&room, "Room")).unwrap();
        let read = read_model(&at("room.3ds")).unwrap();
        assert_eq!((read.unit, read.up, read.mesh.triangles.len()), (Unit::M, UpAxis::Z, 12));
        assert_eq!(read.mesh.bounds().map(|b| (b.min, b.max)), Some(([-10.0, 0.0, 0.0], [10.0, 30.0, 6.0])));

        // What cannot be read says why.
        assert!(is_model(&at("room.3ds")) && is_model(&at("Hall A.OBJ")) && !is_model(&at("hall.mtl")));
        std::fs::write(at("plan.dwg"), b"AC1027").unwrap();
        let why = read_model(&at("plan.dwg")).unwrap_err();
        assert!(why.contains(".obj") && why.contains(".3ds"), "{why}");
        assert!(read_model(&at("nowhere.obj")).unwrap_err().starts_with("it could not be opened"));
        std::fs::write(at("empty.obj"), b"# nothing\n").unwrap();
        assert!(read_model(&at("empty.obj")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slow_picture_is_drawn_at_half_size_while_it_changes_and_others_wait_their_turn() {
        let ctx = egui::Context::default();
        let mut world = World::new();
        let slow = |frame: &mut Frame, _: &Scene| {
            if frame.width == 320 {
                std::thread::sleep(Duration::from_millis(SLOW_MS as u64 + 6));
            }
        };
        let size = |world: &World, name: &str| world.frame(name).map(|f| [f.width, f.height]);
        // The first drawing is at full size, and shows it to be slow.
        world.picture(&ctx, "big", [320, 180], 1, true, slow);
        assert_eq!(size(&world, "big"), Some([320, 180]));
        // While it changes it is drawn at half size; once it rests it is drawn again at full size.
        world.sync(&Stage::default());
        world.picture(&ctx, "big", [320, 180], 2, true, slow);
        assert_eq!(size(&world, "big"), Some([160, 90]));
        assert!(!world.is_fresh("big", [320, 180], 2));
        world.sync(&Stage::default());
        world.picture(&ctx, "big", [320, 180], 2, true, slow);
        assert_eq!(size(&world, "big"), Some([320, 180]));
        assert!(world.is_fresh("big", [320, 180], 2));

        // That used up this frame's time. One picture that can wait is still drawn; the next is not, and has nothing to show yet.
        assert!(world.picture(&ctx, "tile a", [64, 36], 1, false, |_, _| {}).is_some());
        assert!(world.drew);
        assert!(world.picture(&ctx, "tile b", [64, 36], 1, false, |_, _| {}).is_none());
        assert!(!world.drew);
        // On the next frame it gets its turn, and a picture that has not changed costs nothing.
        world.sync(&Stage::default());
        assert!(world.picture(&ctx, "tile b", [64, 36], 1, false, |_, _| {}).is_some());
        world.picture(&ctx, "tile a", [64, 36], 1, false, |_, _| panic!("drawn again without a change"));
        // A picture that must be current is drawn whatever has been spent.
        world.picture(&ctx, "big", [320, 180], 3, true, slow);
        assert!(world.picture(&ctx, "tile a", [64, 36], 2, false, |_, _| {}).is_some());
        world.picture(&ctx, "program", [64, 36], 9, true, |_, _| {});
        assert!(world.drew);
    }

    #[test]
    fn a_picture_is_drawn_once_per_key_and_other_cameras_show_in_it() {
        let ctx = egui::Context::default();
        let mut world = World::new();
        let mut project = Project::default();
        let mut second = CameraConfig::new("cam2", "Second");
        second.mount.y = 6.0;
        project.cameras.push(second);
        let poses = vec![Pose::home(); 2];
        let view = view_of(&project.cameras[0], &poses[0]);
        let key = camera_key(&project, &poses, 0, &view);
        let mut drawn = 0;
        for _ in 0..3 {
            world.picture(&ctx, "program", [320, 180], key, true, |frame, scene| {
                drawn += 1;
                draw_camera_view(frame, scene, &project, &poses, 0, &view);
            });
        }
        assert_eq!(drawn, 1);
        assert!(world.is_fresh("program", [320, 180], key));
        // The second camera stands 6 m ahead on the centre line: its head is what the middle of the picture shows.
        let frame = world.frame("program").unwrap();
        let depth = frame.depth_at(160, 90).unwrap();
        assert!((5.6..6.0).contains(&depth), "{depth}");

        // Raising the second camera changes the first camera's picture, so its key changes.
        let mut raised = poses.clone();
        raised[1].elevation = 400.0;
        assert_ne!(camera_key(&project, &raised, 0, &view), key);
        let mut tilted = poses.clone();
        tilted[1].tilt = 30.0;
        assert_ne!(camera_key(&project, &tilted, 0, &view), key, "another camera's tilted head changes the picture");
        let mut mounted = project.clone();
        mounted.cameras[1].mount.tilt_deg = -15.0;
        assert_ne!(camera_key(&mounted, &poses, 0, &view), key, "mount tilt also changes the head");
        mounted.cameras[1].mount = project.cameras[1].mount;
        mounted.cameras[1].mount.inverted = true;
        assert_ne!(camera_key(&mounted, &poses, 0, &view), key, "an inverted suspension needs a fresh picture");
        mounted = project.clone();
        mounted.cameras[0].mount.inverted = true;
        assert_ne!(camera_key(&mounted, &poses, 0, &view), key, "the hidden camera still casts a shadow");
        // A camera does not see itself.
        let own = view_of(&project.cameras[1], &poses[1]);
        let own_key = camera_key(&project, &poses, 1, &own);
        world.picture(&ctx, "second", [320, 180], own_key, true, |frame, scene| {
            draw_camera_view(frame, scene, &project, &poses, 1, &own)
        });
        assert!(world.frame("second").unwrap().depth_at(160, 90).unwrap() > 9.0);
    }
}
