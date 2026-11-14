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
pub const SKY: [u8; 3] = [9, 10, 12];
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

fn person(mesh: &mut Mesh, x: f32, y: f32, color: [u8; 3]) {
    let floor = DECK_HEIGHT;
    mesh.push_cuboid([x - 0.22, y - 0.12, floor], [x + 0.22, y + 0.12, floor + 1.45], color);
    let head = color.map(|c| c.saturating_add(40));
    mesh.push_cuboid([x - 0.1, y - 0.1, floor + 1.5], [x + 0.1, y + 0.1, floor + 1.76], head);
}

/// The simple stage the program draws when no venue has been imported: an auditorium floor, a raised
/// deck with a lectern and two award marks, people standing at them, and a back wall with a screen.
pub fn builtin() -> Mesh {
    let mut m = Mesh::default();
    let top = |m: &mut Mesh, min: [f32; 2], max: [f32; 2], z: f32, color: [u8; 3]| {
        m.push_quad([[min[0], min[1], z], [max[0], min[1], z], [max[0], max[1], z], [min[0], max[1], z]], color);
    };
    // Auditorium floor in two tones, so distance reads without a texture.
    for i in 0..14 {
        let y = -5.0 + i as f32;
        let color = if i % 2 == 0 { [34, 37, 58] } else { [28, 31, 50] };
        top(&mut m, [-14.0, y], [14.0, y + 1.0], 0.0, color);
    }
    // The deck, its riser, and the wall behind it.
    m.push_cuboid([-10.0, DECK_FRONT, 0.0], [10.0, BACK_WALL, DECK_HEIGHT], [78, 70, 80]);
    m.push_cuboid([-10.0, BACK_WALL, 0.0], [10.0, BACK_WALL + 0.2, 7.0], [40, 50, 104]);
    m.push_cuboid([-4.0, BACK_WALL - 0.06, 2.6], [4.0, BACK_WALL, 6.2], [52, 96, 150]);
    for i in -4..=4 {
        let x = i as f32 * 2.5;
        m.push_cuboid([x - 0.03, BACK_WALL - 0.03, DECK_HEIGHT], [x + 0.03, BACK_WALL, 7.0], [58, 70, 130]);
    }
    // Awards: two marks and two people.
    for dx in [-0.55, 0.55] {
        let x = AWARDS[0] + dx;
        top(&mut m, [x - 0.3, AWARDS[1] - 0.3], [x + 0.3, AWARDS[1] + 0.3], DECK_HEIGHT + 0.01, [70, 200, 40]);
        person(&mut m, x, AWARDS[1], [150, 156, 184]);
    }
    // The lectern, with a presenter behind it.
    person(&mut m, LECTERN[0], LECTERN[1] + 0.45, [186, 164, 150]);
    m.push_cuboid(
        [LECTERN[0] - 0.38, LECTERN[1] - 0.04, DECK_HEIGHT],
        [LECTERN[0] + 0.38, LECTERN[1] + 0.3, DECK_HEIGHT + 1.15],
        [48, 92, 150],
    );
    m.push_cuboid(
        [LECTERN[0] - 0.42, LECTERN[1] - 0.08, DECK_HEIGHT + 1.15],
        [LECTERN[0] + 0.42, LECTERN[1] + 0.34, DECK_HEIGHT + 1.2],
        [120, 160, 210],
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
    scene: Arc<Scene>,
    picker: Arc<Picker>,
    bounds: Option<([f32; 3], [f32; 3])>,
}

fn build(model: Option<Arc<Mesh>>, stage: Stage) -> Built {
    let mesh = statics(model.as_deref(), &stage);
    let bounds = mesh.bounds().map(|b| (b.min, b.max));
    Built { scene: Arc::new(Scene::new(&mesh)), picker: Arc::new(Picker::new(&mesh)), bounds, stage }
}

/// How a camera's lens looks at the world for a pose.
pub fn view_of(camera: &CameraConfig, pose: &Pose) -> Camera {
    let eye = camera.mount.lens_position(pose.elevation).map(|v| v as f32);
    let (yaw, pitch) = camera.mount.view(pose);
    Camera::look(eye, yaw as f32, pitch as f32, camera.lens.hfov_deg(pose.zoom) as f32)
}

/// A pedestal with a head at lens height, turned to where the camera looks.
pub fn marker(mount: &Mount, pose: &Pose, color: [u8; 3]) -> Mesh {
    let [x, y, lens_z] = mount.lens_position(pose.elevation).map(|v| v as f32);
    let floor = mount.z as f32;
    let mut m = Mesh::default();
    m.push_cuboid([x - 0.3, y - 0.3, floor], [x + 0.3, y + 0.3, floor + 0.08], [70, 72, 84]);
    m.push_cuboid([x - 0.07, y - 0.07, floor + 0.08], [x + 0.07, y + 0.07, (lens_z - 0.12).max(floor + 0.1)], [120, 122, 134]);
    let mut head = Mesh::default();
    head.push_cuboid([-0.13, -0.19, -0.11], [0.13, 0.15, 0.11], color);
    head.push_cuboid([-0.06, 0.15, -0.06], [0.06, 0.3, 0.06], [230, 232, 236]);
    let (yaw, _) = mount.view(pose);
    m.append(&head.transformed(&Transform::rotation_z(yaw as f32).then(&Transform::translation([x, y, lens_z]))));
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
        let view = view_of(camera, pose);
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
    let view = view_of(camera, poses.get(index)?);
    let key = camera_key(&s.project, poses, index, &view);
    let project = &s.project;
    s.world.picture(ctx, name, size, key, urgent, |frame, scene| draw_camera_view(frame, scene, project, poses, index, &view))
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

/// The key of a camera's picture: its own view, and where the others stand.
pub fn camera_key(project: &Project, poses: &[Pose], index: usize, view: &Camera) -> u64 {
    let mut key = Key::new().camera(view).add(index);
    for (i, camera) in project.cameras.iter().enumerate() {
        if i != index {
            let pose = poses.get(i).copied().unwrap_or_else(Pose::home);
            let m = &camera.mount;
            key = key.floats(&[m.x, m.y, m.z, m.heading_deg, m.base_height_mm, pose.pan, pose.elevation]);
        }
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
        let mount = Mount { x: 3.0, y: -2.0, z: 0.5, heading_deg: 90.0, base_height_mm: 1200.0 };
        let pose = Pose { elevation: 300.0, ..Pose::home() };
        let m = marker(&mount, &pose, [1, 2, 3]);
        let b = m.bounds().unwrap();
        assert!((b.min[2] - 0.5).abs() < 1e-6, "{b:?}");
        assert!((b.max[2] - (0.5 + 1.5 + 0.11)).abs() < 1e-5, "{b:?}");
        // Heading 90 looks along -X: the lens barrel is the part furthest that way.
        assert!((b.min[0] - (3.0 - 0.3)).abs() < 1e-5 && (b.max[0] - (3.0 + 0.3)).abs() < 1e-5, "{b:?}");
        let barrel_tip = m.positions.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        assert!((barrel_tip - 2.7).abs() < 1e-5);
        assert_eq!(heading_toward(&mount, [3.0, 10.0, 0.0]), 0.0);
        assert!((heading_toward(&mount, [-7.0, -2.0, 0.0]) - 90.0).abs() < 1e-9);
        assert!((heading_toward(&mount, [13.0, -2.0, 0.0]) + 90.0).abs() < 1e-9);
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
        // A camera does not see itself.
        let own = view_of(&project.cameras[1], &poses[1]);
        let own_key = camera_key(&project, &poses, 1, &own);
        world.picture(&ctx, "second", [320, 180], own_key, true, |frame, scene| {
            draw_camera_view(frame, scene, &project, &poses, 1, &own)
        });
        assert!(world.frame("second").unwrap().depth_at(160, 90).unwrap() > 9.0);
    }
}
