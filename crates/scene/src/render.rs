//! Draws a model into a colour buffer and a depth buffer on the CPU.
//!
//! Corners are projected and snapped to 1/256 of a pixel, and triangles are filled with whole-number
//! edge tests, so a pixel's colour does not depend on the machine or on how many threads took part.

use crate::math::{add, dot, normalize, scale, sub};
use crate::mesh::{Bounds, Mesh, DEFAULT_COLOR};
use std::sync::{Mutex, OnceLock, PoisonError};

const SUB_BITS: i32 = 8;
const SUB: i32 = 1 << SUB_BITS;
const HALF: i32 = SUB / 2;
/// Corners that would land further than this many pixels from the middle of the frame are clipped off.
/// Together with `MAX_SIDE` it keeps every snapped coordinate within 2^28, and with that the edge tests within i64.
const GUARD_PX: f32 = 524_288.0;
const SNAP_LIMIT: f32 = (1u32 << 28) as f32;
const MAX_SIDE: usize = 1 << 18;
/// A triangle cut by the five clipping planes has at most 8 corners; the rest is headroom for rounding.
const MAX_CORNERS: usize = 12;
/// Below this many triangles, or pixels, starting threads costs more than it saves.
const SMALL_SCENE: usize = 2_000;
const SMALL_FRAME: usize = 64 * 64;
const ALMOST_ONE: f32 = 0.999_999;
/// Side of the square tiles, in pixels, for which a band remembers how far its furthest pixel is.
const TILE: usize = 8;
const MOST_TILES_CHECKED: usize = 64;
/// Triangles are drawn nearest first, in this many steps of distance, because a triangle behind what is
/// already drawn is cheap to pass over. `NEAREST_STEP` is the step number of things 1/32 m away.
const DEPTH_STEPS: usize = 32;
const NEAREST_STEP: usize = (32.0f32.to_bits() >> 22) as usize;
/// Rows of a triangle at least this many pixels wide have their ends worked out rather than searched for.
const WIDE: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// Unit, orthonormal with `right` and `up`.
    pub forward: [f32; 3],
    /// tan(horizontal field of view / 2)
    pub tan_half_h: f32,
    /// Metres; nothing nearer than this is drawn.
    pub near: f32,
}

impl Camera {
    /// Z is up. Yaw 0 looks toward +Y and positive yaw turns to the camera's left (toward -X); positive
    /// pitch looks up. No roll.
    pub fn look(eye: [f32; 3], yaw_deg: f32, pitch_deg: f32, hfov_deg: f32) -> Camera {
        let (sin_yaw, cos_yaw) = yaw_deg.to_radians().sin_cos();
        let (sin_pitch, cos_pitch) = pitch_deg.to_radians().sin_cos();
        Camera {
            eye,
            right: [cos_yaw, sin_yaw, 0.0],
            up: [sin_yaw * sin_pitch, -cos_yaw * sin_pitch, cos_pitch],
            forward: [-sin_yaw * cos_pitch, cos_yaw * cos_pitch, sin_pitch],
            tan_half_h: (hfov_deg.clamp(1.0, 170.0) * 0.5).to_radians().tan(),
            near: 0.05,
        }
    }

    /// Z is up. Straight up or down has no heading of its own, so the top of the picture then points along -Y or +Y.
    pub fn look_at(eye: [f32; 3], target: [f32; 3], hfov_deg: f32) -> Camera {
        let [x, y, z] = sub(target, eye);
        let level = x.hypot(y);
        let yaw = if level > 0.0 { (-x).atan2(y) } else { 0.0 };
        Camera::look(eye, yaw.to_degrees(), z.atan2(level).to_degrees(), hfov_deg)
    }

    /// Pixel position (x right, y down, in pixels, pixel centres at +0.5) and depth (distance along `forward`).
    /// None when the point is nearer than `near`.
    pub fn project(&self, p: [f32; 3], width: usize, height: usize) -> Option<[f32; 3]> {
        let [x, y, depth] = self.camera_space(p);
        let focal = self.focal_px(width);
        (depth >= self.near).then(|| [width as f32 * 0.5 + focal * x / depth, height as f32 * 0.5 - focal * y / depth, depth])
    }

    /// Unit direction of the ray through pixel coordinates (x, y).
    pub fn ray(&self, x: f32, y: f32, width: usize, height: usize) -> [f32; 3] {
        let focal = self.focal_px(width);
        let (across, upward) = ((x - width as f32 * 0.5) / focal, (height as f32 * 0.5 - y) / focal);
        normalize(add(self.forward, add(scale(self.right, across), scale(self.up, upward))))
    }

    /// Pixels per unit of tangent. Pixels are square, so the same figure serves both directions.
    #[inline]
    fn focal_px(&self, width: usize) -> f32 {
        width as f32 * 0.5 / self.tan_half_h
    }

    /// `p` as (right, up, forward) distances from the eye.
    #[inline]
    fn camera_space(&self, p: [f32; 3]) -> [f32; 3] {
        let from_eye = sub(p, self.eye);
        [dot(from_eye, self.right), dot(from_eye, self.up), dot(from_eye, self.forward)]
    }
}

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub depth: Vec<f32>,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        let mut frame = Frame { width, height, rgba: Vec::new(), depth: Vec::new() };
        frame.allocate();
        frame
    }

    /// Does nothing when the size is unchanged; otherwise the picture starts again black and empty.
    pub fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            (self.width, self.height) = (width, height);
            self.allocate();
        }
    }

    /// Alpha 255, depth = f32::INFINITY.
    pub fn clear(&mut self, color: [u8; 3]) {
        for pixel in self.rgba.as_chunks_mut().0 {
            *pixel = [color[0], color[1], color[2], 255];
        }
        self.depth.fill(f32::INFINITY);
    }

    /// Distance along the camera's forward axis of what was drawn at the pixel; None where only the background is.
    pub fn depth_at(&self, x: usize, y: usize) -> Option<f32> {
        let depth = *self.depth.get(y.checked_mul(self.width)?.checked_add(x)?)?;
        (x < self.width && depth.is_finite()).then_some(depth)
    }

    fn allocate(&mut self) {
        let pixels = self.width * self.height;
        self.rgba.resize(pixels * 4, 0);
        self.depth.resize(pixels, 0.0);
        self.clear([0; 3]);
    }
}

/// A model made ready to draw many times: positions, triangles, and each triangle's colour already shaded
/// for either side it may be seen from.
pub struct Scene {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    colors: Vec<[[u8; 3]; 2]>,
    bounds: Option<Bounds>,
    scratch: Mutex<Scratch>,
}

/// Working memory kept between frames so that drawing does not allocate.
#[derive(Default)]
struct Scratch {
    corners: Vec<Corner>,
    /// For each share of the model's triangles, a list per band of rows and step of distance.
    bins: Vec<Vec<Vec<Triangle>>>,
    /// Per band, the furthest depth drawn so far in each tile.
    far: Vec<Vec<f32>>,
}

impl Scene {
    /// Triangles that point at missing corners or have unusable positions are left out.
    pub fn new(mesh: &Mesh) -> Scene {
        let mut scene = Scene {
            positions: mesh.positions.clone(),
            triangles: Vec::with_capacity(mesh.triangles.len()),
            colors: Vec::with_capacity(mesh.triangles.len()),
            bounds: mesh.bounds(),
            scratch: Mutex::default(),
        };
        for (at, triangle) in mesh.triangles.iter().enumerate() {
            if let Some(corners) = mesh.corners(at) {
                scene.triangles.push(*triangle);
                scene.colors.push(side_colors(corners, mesh.colors.get(at).copied().unwrap_or(DEFAULT_COLOR)));
            }
        }
        scene
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    pub fn bounds(&self) -> Option<Bounds> {
        self.bounds
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub fog_color: [u8; 3],
    /// Metres; no fog nearer than this.
    pub fog_start: f32,
    /// Metres; full `fog_strength` beyond this. fog_end <= fog_start disables fog.
    pub fog_end: f32,
    /// 0..1, how far the colour goes toward `fog_color` at `fog_end`.
    pub fog_strength: f32,
}

impl Default for Look {
    fn default() -> Look {
        Look { fog_color: [0; 3], fog_start: 0.0, fog_end: 0.0, fog_strength: 0.0 }
    }
}

pub fn draw_scene(frame: &mut Frame, camera: &Camera, scene: &Scene, look: &Look) {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    let small = scene.triangles.len() < SMALL_SCENE || frame.width.saturating_mul(frame.height) < SMALL_FRAME;
    let threads =
        if small { 1 } else { *WORKERS.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)) };
    draw_scene_with_threads(frame, camera, scene, look, threads);
}

/// `draw_scene` on a given number of threads. The picture is the same for any number; this exists so a test can say so.
#[doc(hidden)]
pub fn draw_scene_with_threads(frame: &mut Frame, camera: &Camera, scene: &Scene, look: &Look, threads: usize) {
    let Some(view) = View::new(camera, frame) else { return };
    let fog = Fog::new(look);
    let threads = threads.clamp(1, 64);
    let mut scratch = scene.scratch.lock().unwrap_or_else(PoisonError::into_inner);
    let Scratch { corners, bins, far } = &mut *scratch;

    // The work is cut into more pieces than there are threads, so a thread that finishes early (its band was
    // empty sky, or another program took its core) can take over pieces from a busy one.
    let pieces = if threads == 1 { 1 } else { threads * 4 };

    corners.resize(scene.positions.len(), Corner::default());
    let share = scene.positions.len().div_ceil(pieces).max(1);
    for_each_parallel(threads, corners.chunks_mut(share).zip(scene.positions.chunks(share)), |(corners, positions)| {
        for (corner, position) in corners.iter_mut().zip(positions) {
            *corner = view.corner(*position);
        }
    });
    let corners = &*corners;

    // Each share of the triangles is sorted into lists by band of rows and by distance. A band is then drawn
    // by one thread, nearest list first and within a distance the shares in order, so each pixel always sees
    // the same triangles in the same order however the work was divided.
    let band_rows = frame.height.div_ceil(pieces).max(8);
    let band_count = frame.height.div_ceil(band_rows);
    let share = scene.triangles.len().div_ceil(pieces).max(1);
    bins.resize_with(scene.triangles.len().div_ceil(share), Vec::new);
    for lists in bins.iter_mut() {
        lists.resize_with(band_count * DEPTH_STEPS, Vec::new);
        lists.iter_mut().for_each(Vec::clear);
    }
    let shares = scene.triangles.chunks(share).zip(scene.colors.chunks(share)).zip(bins.iter_mut());
    for_each_parallel(threads, shares, |((triangles, colors), lists)| {
        let mut sink = |triangle: Triangle, first_row: i32, last_row: i32| {
            let step = depth_step(&triangle);
            for band in first_row as usize / band_rows..=last_row as usize / band_rows {
                lists[band * DEPTH_STEPS + step].push(triangle);
            }
        };
        for (triangle, colors) in triangles.iter().zip(colors) {
            let [a, b, c] = triangle.map(|index| corners[index as usize]);
            if a.w > 0.0 && b.w > 0.0 && c.w > 0.0 {
                emit(&view, [a, b, c], colors, &mut sink);
            } else if a.w >= 0.0 || b.w >= 0.0 || c.w >= 0.0 {
                clip_and_emit(
                    &view,
                    triangle.map(|index| view.camera.camera_space(scene.positions[index as usize])),
                    colors,
                    &mut sink,
                );
            }
        }
    });

    let width = frame.width;
    far.resize_with(band_count, Vec::new);
    for tiles in far.iter_mut() {
        tiles.clear();
        tiles.resize(width.div_ceil(TILE) * band_rows.div_ceil(TILE), f32::INFINITY);
    }
    let bands = frame
        .rgba
        .chunks_mut(band_rows * width * 4)
        .zip(frame.depth.chunks_mut(band_rows * width))
        .zip(far.iter_mut())
        .enumerate();
    for_each_parallel(threads, bands, |(at, ((rgba, depth), far))| {
        let mut band = Band {
            first_row: (at * band_rows) as i32,
            rows: (depth.len() / width) as i32,
            width: width as i32,
            rgba,
            depth,
            far,
        };
        for step in 0..DEPTH_STEPS {
            for triangle in bins.iter().flat_map(|lists| &lists[at * DEPTH_STEPS + step]) {
                fill(triangle, &mut band, fog.as_ref());
            }
        }
    });
}

/// For small things that change every frame (camera markers). Same shading and the same pixels as `draw_scene`.
pub fn draw_mesh(frame: &mut Frame, camera: &Camera, mesh: &Mesh, look: &Look) {
    let Some(view) = View::new(camera, frame) else { return };
    let fog = Fog::new(look);
    let mut band =
        Band { first_row: 0, rows: view.height, width: view.width, rgba: &mut frame.rgba, depth: &mut frame.depth, far: &mut [] };
    for at in 0..mesh.triangles.len() {
        if let Some(corners) = mesh.corners(at) {
            let colors = side_colors(corners, mesh.colors.get(at).copied().unwrap_or(DEFAULT_COLOR));
            let mut sink = |triangle: Triangle, _, _| fill(&triangle, &mut band, fog.as_ref());
            clip_and_emit(&view, corners.map(|p| camera.camera_space(p)), &colors, &mut sink);
        }
    }
}

/// A world-space line, clipped at the near plane, depth-tested against what is already drawn (with a small
/// bias toward the viewer so lines lying on surfaces show), alpha-blended. `width_px` 1 or 2. Does not write depth.
pub fn draw_line(frame: &mut Frame, camera: &Camera, a: [f32; 3], b: [f32; 3], rgba: [u8; 4], width_px: u32) {
    let Some(view) = View::new(camera, frame) else { return };
    let (a, b) = (camera.camera_space(a), camera.camera_space(b));
    if !(a[2] >= view.near || b[2] >= view.near) {
        return;
    }
    let clipped = |end: [f32; 3], other: [f32; 3]| {
        if end[2] < view.near {
            add(other, scale(sub(end, other), (other[2] - view.near) / (other[2] - end[2])))
        } else {
            end
        }
    };
    let (a, b) = (clipped(a, b), clipped(b, a));
    // [x, y, 1/depth] in pixels; 1/depth runs evenly along the line on screen, depth itself does not.
    let project = |c: [f32; 3]| {
        let w = 1.0 / c[2] as f64;
        [
            view.center[0] as f64 + view.focal as f64 * c[0] as f64 * w,
            view.center[1] as f64 - view.focal as f64 * c[1] as f64 * w,
            w,
        ]
    };
    let (a, b) = (project(a), project(b));
    if !a.iter().chain(&b).all(|v| v.is_finite()) {
        return;
    }

    // One sample per pixel along the direction the line mostly runs in ("along"), each at a pixel centre.
    let steep = (b[1] - a[1]).abs() > (b[0] - a[0]).abs();
    let (along, across) = if steep { (1, 0) } else { (0, 1) };
    let (a, b) = if a[along] <= b[along] { (a, b) } else { (b, a) };
    let (along_size, across_size) = if steep { (view.height, view.width) } else { (view.width, view.height) };
    let span = b[along] - a[along];
    let (mut first, mut last) = ((a[along] - 0.5).ceil(), (b[along] - 0.5).floor());
    if first > last {
        // Too short to cross a pixel centre: it still leaves one dot.
        first = ((a[along] + b[along]) * 0.5).floor();
        last = first;
    }
    let (first, last) = (first.max(0.0) as i32, last.min(along_size as f64 - 1.0) as i32);
    let pixel = |along: i32, across: i32| {
        let (x, y) = if steep { (across, along) } else { (along, across) };
        y as usize * frame.width + x as usize
    };
    let alpha = rgba[3] as u32;
    for step in first..=last {
        let t = if span > 0.0 { ((step as f64 + 0.5 - a[along]) / span).clamp(0.0, 1.0) } else { 0.5 };
        let position = a[across] + t * (b[across] - a[across]);
        let depth = (1.0 / (a[2] + t * (b[2] - a[2]))) as f32;
        let nearest = position.floor();
        let side = if position - nearest >= 0.5 { 1.0 } else { -1.0 };
        for offset in [0.0, side].into_iter().take(if width_px >= 2 { 2 } else { 1 }) {
            let row = nearest + offset;
            if !(row >= 0.0 && row < across_size as f64) {
                continue;
            }
            let row = row as i32;
            // The surface was measured at pixel centres and the line passes up to a pixel beside them, where
            // a slanted surface is nearer or further. Its depth at this spot lies between the neighbours'.
            let here = frame.depth[pixel(step, row)];
            let beside = |r: i32, opposite: i32| {
                if (0..across_size).contains(&r) {
                    frame.depth[pixel(step, r)]
                } else if (0..across_size).contains(&opposite) {
                    // Past the edge of the frame the surface is taken to carry on as it runs up to the edge.
                    let w = 2.0 / here - 1.0 / frame.depth[pixel(step, opposite)];
                    if w > 0.0 {
                        1.0 / w
                    } else {
                        f32::INFINITY
                    }
                } else {
                    here
                }
            };
            let behind = here.max(beside(row - 1, row + 1)).max(beside(row + 1, row - 1));
            if depth * 0.998 - 0.002 <= behind {
                let at = pixel(step, row) * 4;
                for (channel, value) in frame.rgba[at..at + 3].iter_mut().zip(rgba) {
                    *channel = ((value as u32 * alpha + *channel as u32 * (255 - alpha) + 127) / 255) as u8;
                }
            }
        }
    }
}

/// A camera together with the frame it draws into.
struct View {
    camera: Camera,
    near: f32,
    focal: f32,
    center: [f32; 2],
    /// Largest |x| / depth and |y| / depth that still projects within `GUARD_PX`.
    guard: f32,
    width: i32,
    height: i32,
}

impl View {
    /// None when there is nothing sensible to draw into or with.
    fn new(camera: &Camera, frame: &Frame) -> Option<View> {
        let (width, height) = (frame.width, frame.height);
        let sized = (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height);
        let focal = camera.focal_px(width);
        if !sized
            || frame.depth.len() != width * height
            || frame.rgba.len() != width * height * 4
            || !(focal > 0.0 && focal.is_finite())
        {
            return None;
        }
        Some(View {
            camera: *camera,
            near: camera.near.max(1e-6),
            focal,
            center: [width as f32 * 0.5, height as f32 * 0.5],
            guard: GUARD_PX / focal,
            width: width as i32,
            height: height as i32,
        })
    }

    /// A model corner on screen, or a marker when triangles using it must be clipped first.
    #[inline]
    fn corner(&self, position: [f32; 3]) -> Corner {
        let c = self.camera.camera_space(position);
        let in_front = c[2] >= self.near;
        let within_guard = c[0].abs() <= self.guard * c[2] && c[1].abs() <= self.guard * c[2];
        match (in_front, within_guard) {
            (false, _) => Corner { x: 0, y: 0, w: -1.0 },
            (true, false) => Corner::default(),
            (true, true) => self.project(c),
        }
    }

    #[inline]
    fn project(&self, c: [f32; 3]) -> Corner {
        let w = 1.0 / c[2];
        Corner { x: snap(self.center[0] + self.focal * c[0] * w), y: snap(self.center[1] - self.focal * c[1] * w), w }
    }
}

/// A corner on screen: position in 1/256 pixel and `w` = 1 / depth, which unlike depth runs evenly
/// across a triangle on screen. In the per-frame list, `w` 0 marks a corner off beyond the guard band
/// and a negative `w` one nearer than the near plane.
#[derive(Clone, Copy, Default)]
struct Corner {
    x: i32,
    y: i32,
    w: f32,
}

/// A triangle ready to fill: on screen, corners ordered so the edge tests are positive inside, colour chosen.
#[derive(Clone, Copy)]
struct Triangle {
    corners: [Corner; 3],
    color: [u8; 3],
}

/// Which of the `DEPTH_STEPS` lists a triangle goes in, by its nearest corner: one further for each half
/// doubling of distance, read straight from the exponent and top fraction bit of the largest 1/depth.
#[inline]
fn depth_step(triangle: &Triangle) -> usize {
    let [a, b, c] = triangle.corners;
    let nearness = (a.w.max(b.w).max(c.w).to_bits() >> 22) as usize;
    NEAREST_STEP.saturating_sub(nearness).min(DEPTH_STEPS - 1)
}

/// Pixels to the nearest 1/256, halves upward. Written without `round` or `floor`, which are slow calls on some machines.
#[inline]
fn snap(pixels: f32) -> i32 {
    let v = (pixels * SUB as f32 + 0.5).clamp(-SNAP_LIMIT, SNAP_LIMIT);
    let whole = v as i32;
    whole - i32::from(whole as f32 > v)
}

/// First and last pixel whose centre lies within a span given in 1/256 pixel.
#[inline]
fn pixel_span(low: i32, high: i32) -> (i32, i32) {
    ((low + HALF - 1) >> SUB_BITS, (high - HALF) >> SUB_BITS)
}

/// Twice the signed area of a triangle on screen; positive when a, b, c run clockwise as seen (y points down).
#[inline]
fn area(a: Corner, b: Corner, c: Corner) -> i64 {
    (b.x - a.x) as i64 * (c.y - a.y) as i64 - (b.y - a.y) as i64 * (c.x - a.x) as i64
}

/// The triangle's colour lit from its front (corners counter-clockwise when seen) and from its back.
///
/// The light is fixed in the world: a key light from one side and above, a little more light on faces that
/// point up, and a floor of ambient light, so that shapes read without textures.
fn side_colors([a, b, c]: [[f32; 3]; 3], color: [u8; 3]) -> [[u8; 3]; 2] {
    let key = normalize([0.35, -0.5, 0.8]);
    // In f64, where even the largest f32 edges have a cross product that does not overflow.
    let edge = |to: [f32; 3]| [0, 1, 2].map(|axis| to[axis] as f64 - a[axis] as f64);
    let (u, v) = (edge(b), edge(c));
    let normal = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let front = if length > 0.0 { normal.map(|n| (n / length) as f32) } else { [0.0; 3] };
    [front, scale(front, -1.0)].map(|normal| {
        let shade = 0.38 + 0.42 * dot(normal, key).max(0.0) + 0.20 * (0.5 + 0.5 * normal[2]);
        color.map(|channel| (channel as f32 * shade + 0.5).min(255.0) as u8)
    })
}

/// Clips a triangle given in camera space against the near plane and the guard band, and emits what is left.
fn clip_and_emit(view: &View, corners: [[f32; 3]; 3], colors: &[[u8; 3]; 2], sink: &mut impl FnMut(Triangle, i32, i32)) {
    if !corners.iter().any(|c| c[2] >= view.near) {
        return;
    }
    // Each plane as [a, b, c, d]: a point is kept when a*x + b*y + c*z + d >= 0.
    let planes = [
        [0.0, 0.0, 1.0, -view.near],
        [-1.0, 0.0, view.guard, 0.0],
        [1.0, 0.0, view.guard, 0.0],
        [0.0, -1.0, view.guard, 0.0],
        [0.0, 1.0, view.guard, 0.0],
    ];
    let mut polygon = [[0.0; 3]; MAX_CORNERS];
    polygon[..3].copy_from_slice(&corners);
    let mut count = 3;
    for plane in planes {
        count = clip(&mut polygon, count, plane);
        if count < 3 {
            return;
        }
    }
    let first = view.project(polygon[0]);
    let mut previous = view.project(polygon[1]);
    for corner in &polygon[2..count] {
        let next = view.project(*corner);
        emit(view, [first, previous, next], colors, sink);
        previous = next;
    }
}

/// Keeps the part of a convex polygon on the positive side of `plane`; returns the new corner count.
fn clip(polygon: &mut [[f32; 3]; MAX_CORNERS], count: usize, plane: [f32; 4]) -> usize {
    let source = *polygon;
    let distance = |p: [f32; 3]| plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3];
    let mut kept = 0;
    let mut keep = |p: [f32; 3]| {
        if kept < MAX_CORNERS {
            polygon[kept] = p;
            kept += 1;
        }
    };
    for at in 0..count {
        let (a, b) = (source[at], source[(at + 1) % count]);
        let (da, db) = (distance(a), distance(b));
        if da >= 0.0 {
            keep(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            // Measured from the kept end whichever way round the edge is met, so two triangles that share
            // the edge cut it at exactly the same point and no gap opens between them.
            let (inside, outside, t) = if da >= 0.0 { (a, b, da / (da - db)) } else { (b, a, db / (db - da)) };
            keep(add(inside, scale(sub(outside, inside), t)));
        }
    }
    kept
}

/// Hands a projected triangle to `sink` with the rows it touches, unless it covers no pixel centre.
fn emit(view: &View, mut corners: [Corner; 3], colors: &[[u8; 3]; 2], sink: &mut impl FnMut(Triangle, i32, i32)) {
    let [a, b, c] = corners;
    let (first_column, last_column) = pixel_span(a.x.min(b.x).min(c.x), a.x.max(b.x).max(c.x));
    let (first_row, last_row) = pixel_span(a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y));
    let (first_column, last_column) = (first_column.max(0), last_column.min(view.width - 1));
    let (first_row, last_row) = (first_row.max(0), last_row.min(view.height - 1));
    if first_column > last_column || first_row > last_row {
        return;
    }
    // Seen from its front a triangle runs counter-clockwise, which on screen (y down) is a negative area.
    let color = match area(a, b, c) {
        0 => return,
        seen if seen < 0 => {
            corners.swap(1, 2);
            colors[0]
        }
        _ => colors[1],
    };
    sink(Triangle { corners, color }, first_row, last_row);
}

/// Some whole rows of the frame, owned by one thread while it draws.
struct Band<'a> {
    first_row: i32,
    rows: i32,
    width: i32,
    rgba: &'a mut [u8],
    depth: &'a mut [f32],
    /// For each `TILE` x `TILE` block of the band, a depth no pixel in it lies beyond. Empty when not kept.
    far: &'a mut [f32],
}

impl Band<'_> {
    /// True when everything already drawn in the given box of pixels is at least as near as `nearest`, so
    /// that a triangle no nearer than that cannot show there. Skipping it leaves the picture unchanged.
    fn hides(&mut self, (first_column, last_column): (i32, i32), (first_row, last_row): (i32, i32), nearest: f32) -> bool {
        let width = self.width as usize;
        let (left, right) = (first_column as usize / TILE, last_column as usize / TILE);
        let (top, bottom) = ((first_row - self.first_row) as usize / TILE, (last_row - self.first_row) as usize / TILE);
        if self.far.is_empty() || (right - left + 1) * (bottom - top + 1) > MOST_TILES_CHECKED {
            return false;
        }
        for tile_row in top..=bottom {
            for tile_column in left..=right {
                let far = &mut self.far[tile_row * width.div_ceil(TILE) + tile_column];
                if nearest < *far {
                    // Depths only ever come nearer, so the remembered figure is safe but may be stale.
                    let columns = tile_column * TILE..((tile_column + 1) * TILE).min(width);
                    let rows = tile_row * TILE..((tile_row + 1) * TILE).min(self.rows as usize);
                    let pixels = rows.flat_map(|row| &self.depth[row * width + columns.start..row * width + columns.end]);
                    *far = pixels.fold(f32::NEG_INFINITY, |far, &depth| if depth > far { depth } else { far });
                    if nearest < *far {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Fills the part of a triangle that lies in the band; the nearest surface at each pixel wins.
fn fill(triangle: &Triangle, band: &mut Band, fog: Option<&Fog>) {
    let [a, b, c] = triangle.corners;
    let (first_column, last_column) = pixel_span(a.x.min(b.x).min(c.x), a.x.max(b.x).max(c.x));
    let (top, bottom) = pixel_span(a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y));
    let (first_column, last_column) = (first_column.max(0), last_column.min(band.width - 1));
    let top = top.max(0);
    let (first_row, last_row) = (top.max(band.first_row), bottom.min(band.first_row + band.rows - 1));
    let (w_min, w_max) = (a.w.min(b.w).min(c.w), a.w.max(b.w).max(c.w));
    if first_column > last_column
        || first_row > last_row
        || band.hides((first_column, last_column), (first_row, last_row), 1.0 / w_max)
    {
        return;
    }

    // For each edge: its test value at the centre of the first pixel, and the change per pixel right and down.
    let (px, py) = ((first_column << SUB_BITS) + HALF, (first_row << SUB_BITS) + HALF);
    let edges = [(b, c), (c, a), (a, b)].map(|(p, q)| {
        let (dx, dy) = ((q.x - p.x) as i64, (q.y - p.y) as i64);
        let value = dx * (py - p.y) as i64 - dy * (px - p.x) as i64;
        // A pixel centre exactly on an edge belongs to just one of the two triangles that share the edge:
        // the one that runs along it downward, or leftward when the edge is level.
        let owned = dy > 0 || (dy == 0 && dx < 0);
        (value - i64::from(!owned), -dy * SUB as i64, dx * SUB as i64)
    });
    let (mut values, right, down) = (edges.map(|edge| edge.0), edges.map(|edge| edge.1), edges.map(|edge| edge.2));
    // An edge's test passes through zero `-value / right` columns along the row. The triangle begins there
    // if the test grows to the right and ends there if it shrinks.
    let per_right = right.map(|step| if step == 0 { 0.0 } else { 1.0 / step as f64 });
    let columns = last_column - first_column;

    // 1/depth is a plane over the screen. It is set up in f64 and measured from the first pixel on screen
    // rather than from a corner, which may be far outside the frame.
    let to_pixels = 1.0 / SUB as f64;
    let (ax, ay) = (a.x as f64 * to_pixels, a.y as f64 * to_pixels);
    let (bx, by, cx, cy) =
        (b.x as f64 * to_pixels - ax, b.y as f64 * to_pixels - ay, c.x as f64 * to_pixels - ax, c.y as f64 * to_pixels - ay);
    let (bw, cw) = ((b.w - a.w) as f64, (c.w - a.w) as f64);
    let doubled_area = bx * cy - by * cx;
    let (w_per_column, w_per_row) = ((bw * cy - cw * by) / doubled_area, (cw * bx - bw * cx) / doubled_area);
    let w_origin = a.w as f64 + w_per_column * (first_column as f64 + 0.5 - ax) + w_per_row * (top as f64 + 0.5 - ay);
    let w_step = w_per_column as f32;

    let plain = [triangle.color[0], triangle.color[1], triangle.color[2], 255];
    for row in first_row..=last_row {
        // The first and last pixel of the row inside the triangle are found by testing inward from both
        // ends. The sums cannot overflow (see `GUARD_PX`), and saying so spares the checks.
        let inside = |column: i32| {
            let test = |edge: usize| values[edge].wrapping_add(right[edge].wrapping_mul(column as i64));
            (test(0) | test(1) | test(2)) >= 0
        };
        let (mut start, mut end) = (0, columns);
        if columns >= WIDE {
            // On a wide row most of that walk is saved by working out where the edges cross it. The crossings
            // are right to a tiny fraction of a pixel, so the true first pixel is the estimate or the one
            // after, and the true last pixel at most one beyond its estimate; the tests still settle both.
            let (mut low, mut high) = (0.0f64, columns as f64);
            for edge in 0..3 {
                let crossing = -(values[edge] as f64) * per_right[edge];
                if right[edge] > 0 {
                    low = low.max(crossing);
                } else if right[edge] < 0 {
                    high = high.min(crossing);
                }
            }
            start = low.min(columns as f64 + 1.0) as i32;
            end = (high.max(-1.0) as i32 + 1).min(columns);
        }
        while start <= end && !inside(start) {
            start += 1;
        }
        while end > start && !inside(end) {
            end -= 1;
        }
        if start <= end {
            let row_start = (row - band.first_row) as usize * band.width as usize + first_column as usize;
            let (from, to) = (row_start + start as usize, row_start + end as usize + 1);
            let w_start = (w_origin + w_per_row * (row - top) as f64 + w_per_column * start as f64) as f32;
            let pixels = band.depth[from..to].iter_mut().zip(band.rgba[from * 4..to * 4].as_chunks_mut().0);
            for (at, (depth, pixel)) in pixels.enumerate() {
                // Held between the corners' own values, so rounding can never put a pixel behind the eye.
                let w = (w_start + w_step * at as f32).max(w_min).min(w_max);
                // The multiplication weeds out most hidden pixels without the cost of a division.
                if *depth * w > ALMOST_ONE {
                    let z = 1.0 / w;
                    if z < *depth {
                        *depth = z;
                        *pixel = fog.map_or(plain, |fog| fog.apply(triangle.color, z));
                    }
                }
            }
        }
        values = [0, 1, 2].map(|edge| values[edge].wrapping_add(down[edge]));
    }
}

struct Fog {
    color: [u8; 3],
    start: f32,
    /// Fog per metre and the most fog there can be, both out of 256.
    per_metre: f32,
    most: f32,
}

impl Fog {
    fn new(look: &Look) -> Option<Fog> {
        let strength = look.fog_strength.clamp(0.0, 1.0);
        let on = look.fog_end > look.fog_start && strength > 0.0;
        on.then(|| Fog {
            color: look.fog_color,
            start: look.fog_start,
            per_metre: 256.0 * strength / (look.fog_end - look.fog_start),
            most: 256.0 * strength,
        })
    }

    #[inline]
    fn apply(&self, color: [u8; 3], depth: f32) -> [u8; 4] {
        let fog = ((depth - self.start) * self.per_metre).max(0.0).min(self.most) as u32;
        let mix = |channel: usize| ((color[channel] as u32 * (256 - fog) + self.color[channel] as u32 * fog + 128) >> 8) as u8;
        [mix(0), mix(1), mix(2), 255]
    }
}

/// Runs `work` on every item, on up to `threads` threads including this one.
fn for_each_parallel<T: Send>(threads: usize, items: impl Iterator<Item = T> + Send, work: impl Fn(T) + Sync) {
    if threads <= 1 {
        items.for_each(work);
        return;
    }
    let queue = Mutex::new(items);
    let run = || loop {
        let item = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
        match item {
            Some(item) => work(item),
            None => break,
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..threads {
            scope.spawn(run);
        }
        run();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::cross;
    use crate::testing::Rng;

    const SKY: [u8; 3] = [1, 2, 3];
    const NO_FOG: Look = Look { fog_color: [0; 3], fog_start: 0.0, fog_end: 0.0, fog_strength: 0.0 };

    fn sky(width: usize, height: usize) -> Frame {
        let mut frame = Frame::new(width, height);
        frame.clear(SKY);
        frame
    }

    fn pixels(frame: &Frame) -> impl Iterator<Item = [u8; 4]> + '_ {
        frame.rgba.as_chunks().0.iter().copied()
    }

    fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
        let at = (y * frame.width + x) * 4;
        [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2], frame.rgba[at + 3]]
    }

    fn is_untouched(frame: &Frame) -> bool {
        pixels(frame).all(|p| p == [SKY[0], SKY[1], SKY[2], 255]) && frame.depth.iter().all(|d| *d == f32::INFINITY)
    }

    fn depth_bits(frame: &Frame) -> Vec<u32> {
        frame.depth.iter().map(|d| d.to_bits()).collect()
    }

    fn close(a: [f32; 3], b: [f32; 3], within: f32) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= within)
    }

    /// A wall across the view at distance `y`, facing the origin, wound so that side is its front.
    fn wall(y: f32, half: f32, color: [u8; 3]) -> Mesh {
        let mut mesh = Mesh::default();
        mesh.push_quad([[-half, y, -half], [half, y, -half], [half, y, half], [-half, y, half]], color);
        mesh
    }

    fn floor(half: f32, color: [u8; 3]) -> Mesh {
        let mut mesh = Mesh::default();
        mesh.push_quad([[-half, -half, 0.0], [half, -half, 0.0], [half, half, 0.0], [-half, half, 0.0]], color);
        mesh
    }

    /// Triangles of every size and direction around the origin, some through the eye and some far too big.
    fn soup(count: usize, seed: u64) -> Mesh {
        let mut rng = Rng(seed);
        let mut mesh = Mesh::default();
        for at in 0..count {
            let reach = [0.3, 0.3, 1.0, 4.0, 30.0][at % 5];
            let center = [rng.range(-8.0, 8.0), rng.range(-8.0, 8.0), rng.range(-3.0, 3.0)];
            let mut corner = || [0, 1, 2].map(|axis| center[axis] + rng.range(-reach, reach));
            let (a, b, c) = (corner(), corner(), corner());
            mesh.push_triangle(a, b, c, [(at % 251) as u8, (at * 7 % 253) as u8, (at * 13 % 241) as u8]);
        }
        mesh
    }

    fn fnv1a(bytes: impl Iterator<Item = u8>) -> u64 {
        bytes.fold(0xCBF2_9CE4_8422_2325, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01B3))
    }

    #[test]
    fn the_camera_follows_its_conventions() {
        let ahead = Camera::look([1.0, 2.0, 3.0], 0.0, 0.0, 90.0);
        assert_eq!((ahead.eye, ahead.near), ([1.0, 2.0, 3.0], 0.05));
        assert!(
            close(ahead.forward, [0.0, 1.0, 0.0], 1e-6)
                && close(ahead.right, [1.0, 0.0, 0.0], 1e-6)
                && close(ahead.up, [0.0, 0.0, 1.0], 1e-6)
        );
        assert!((ahead.tan_half_h - 1.0).abs() < 1e-6);
        assert!((Camera::look([0.0; 3], 0.0, 0.0, 60.0).tan_half_h - 0.577_350_3).abs() < 1e-6);

        let turned_left = Camera::look([0.0; 3], 90.0, 0.0, 60.0);
        assert!(close(turned_left.forward, [-1.0, 0.0, 0.0], 1e-6) && close(turned_left.right, [0.0, 1.0, 0.0], 1e-6));
        let looking_up = Camera::look([0.0; 3], 0.0, 30.0, 60.0);
        assert!(close(looking_up.forward, [0.0, 0.866_025_4, 0.5], 1e-6) && close(looking_up.up, [0.0, -0.5, 0.866_025_4], 1e-6));

        let mut rng = Rng(11);
        for _ in 0..200 {
            let camera = Camera::look([0.0; 3], rng.range(-400.0, 400.0), rng.range(-90.0, 90.0), rng.range(-10.0, 200.0));
            let (r, u, f) = (camera.right, camera.up, camera.forward);
            assert!(r[2] == 0.0, "no roll: the horizon stays level");
            assert!(u[2] >= -1e-6, "the picture is never upside down");
            for (a, b, expected) in [(r, r, 1.0), (u, u, 1.0), (f, f, 1.0), (r, u, 0.0), (u, f, 0.0), (f, r, 0.0)] {
                assert!((dot(a, b) - expected).abs() < 1e-5);
            }
            // Right-handed: right x forward = up, so the picture is not mirrored.
            assert!(close(cross(r, f), u, 1e-5));
            assert!(camera.tan_half_h > 0.008 && camera.tan_half_h < 11.5);
        }
    }

    #[test]
    fn look_at_points_the_camera_at_its_target() {
        let mut rng = Rng(5);
        for _ in 0..200 {
            let eye = [rng.range(-9.0, 9.0), rng.range(-9.0, 9.0), rng.range(-9.0, 9.0)];
            let target = [rng.range(-9.0, 9.0), rng.range(-9.0, 9.0), rng.range(-9.0, 9.0)];
            let camera = Camera::look_at(eye, target, 50.0);
            assert!(close(camera.forward, normalize(sub(target, eye)), 1e-4));
            assert_eq!(camera.right[2], 0.0);
            let [x, y, _] = camera.project(target, 640, 360).unwrap();
            assert!((x - 320.0).abs() < 0.05 && (y - 180.0).abs() < 0.05);
        }
        let down = Camera::look_at([0.0, 0.0, 9.0], [0.0; 3], 50.0);
        assert!(
            close(down.forward, [0.0, 0.0, -1.0], 1e-6)
                && close(down.right, [1.0, 0.0, 0.0], 1e-6)
                && close(down.up, [0.0, 1.0, 0.0], 1e-6)
        );
        let nowhere = Camera::look_at([1.0; 3], [1.0; 3], 50.0);
        assert!(close(nowhere.forward, [0.0, 1.0, 0.0], 1e-6));
    }

    #[test]
    fn project_and_ray_undo_each_other() {
        let mut rng = Rng(77);
        for _ in 0..500 {
            let eye = [rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), rng.range(0.0, 3.0)];
            let camera = Camera::look(eye, rng.range(-180.0, 180.0), rng.range(-80.0, 80.0), rng.range(20.0, 120.0));
            let (x, y) = (rng.range(-50.0, 690.0), rng.range(-50.0, 410.0));
            let ray = camera.ray(x, y, 640, 360);
            assert!((dot(ray, ray) - 1.0).abs() < 1e-5);
            let distance = rng.range(0.5, 40.0);
            let [px, py, depth] = camera.project(add(eye, scale(ray, distance)), 640, 360).unwrap();
            assert!((px - x).abs() < 0.02 && (py - y).abs() < 0.02, "({x}, {y}) came back as ({px}, {py})");
            assert!((depth - distance * dot(ray, camera.forward)).abs() < 1e-3);
        }
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        assert!(close(camera.ray(320.0, 180.0, 640, 360), [0.0, 1.0, 0.0], 1e-6));
        assert!(camera.ray(639.0, 180.0, 640, 360)[0] > 0.0, "the right of the picture is the camera's right");
        assert!(camera.ray(320.0, 0.0, 640, 360)[2] > 0.0, "the top of the picture is up");
        assert_eq!(camera.project([0.0, -1.0, 0.0], 640, 360), None);
        assert_eq!(camera.project([0.0, 0.04, 0.0], 640, 360), None);
        assert!(camera.project([0.0, 0.06, 0.0], 640, 360).is_some());
    }

    #[test]
    fn the_edge_of_the_field_of_view_is_the_edge_of_the_picture() {
        for (hfov, yaw) in [(60.0f32, 0.0f32), (90.0, 40.0), (30.0, -110.0), (120.0, 5.0)] {
            let camera = Camera::look([2.0, 1.0, 1.5], yaw, 0.0, hfov);
            // Half the field of view to the right of where the camera looks, which is a turn toward negative yaw.
            let direction = Camera::look([0.0; 3], yaw - hfov * 0.5, 0.0, hfov).forward;
            let [x, y, _] = camera.project(add(camera.eye, scale(direction, 7.0)), 960, 540).unwrap();
            assert!((x - 960.0).abs() < 0.01 && (y - 270.0).abs() < 0.01, "{hfov} degrees: ({x}, {y})");
            let direction = Camera::look([0.0; 3], yaw + hfov * 0.5, 0.0, hfov).forward;
            let [x, _, _] = camera.project(add(camera.eye, scale(direction, 7.0)), 960, 540).unwrap();
            assert!(x.abs() < 0.01, "{hfov} degrees: {x}");
        }
        // Pixels are square: the same angle up as across is the same number of pixels.
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 90.0);
        assert!(close(camera.project([1.0, 2.0, 1.0], 960, 540).unwrap(), [720.0, 30.0, 2.0], 1e-3));
    }

    #[test]
    fn frames_clear_and_resize() {
        let mut frame = Frame::new(7, 5);
        assert_eq!((frame.rgba.len(), frame.depth.len()), (140, 35));
        assert_eq!(pixel(&frame, 6, 4), [0, 0, 0, 255]);
        assert_eq!(frame.depth_at(3, 3), None);
        frame.clear([9, 8, 7]);
        assert!(pixels(&frame).all(|p| p == [9, 8, 7, 255]) && frame.depth.iter().all(|d| *d == f32::INFINITY));
        frame.resize(7, 5);
        assert_eq!(pixel(&frame, 0, 0), [9, 8, 7, 255], "the same size keeps the picture");
        frame.resize(4, 9);
        assert_eq!((frame.width, frame.height, frame.rgba.len(), frame.depth.len()), (4, 9, 144, 36));
        assert_eq!(pixel(&frame, 3, 8), [0, 0, 0, 255]);
        frame.resize(0, 0);
        assert!(frame.rgba.is_empty() && frame.depth.is_empty());
    }

    #[test]
    fn the_nearer_wall_hides_the_further_in_either_order() {
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let (near, far) = (wall(4.0, 1.0, [250, 0, 0]), wall(9.0, 50.0, [0, 0, 250]));
        let mut pictures = Vec::new();
        for order in [[&near, &far], [&far, &near]] {
            let mut both = Mesh::default();
            order.iter().for_each(|mesh| both.append(mesh));
            let mut together = sky(160, 90);
            draw_scene(&mut together, &camera, &Scene::new(&both), &NO_FOG);
            let mut apart = sky(160, 90);
            order.iter().for_each(|mesh| draw_mesh(&mut apart, &camera, mesh, &NO_FOG));
            assert_eq!(together.rgba, apart.rgba);
            assert_eq!(depth_bits(&together), depth_bits(&apart));
            pictures.push(together);
        }
        assert_eq!(pictures[0].rgba, pictures[1].rgba);
        assert_eq!(depth_bits(&pictures[0]), depth_bits(&pictures[1]));
        let picture = &pictures[0];
        let (middle, corner) = (pixel(picture, 80, 45), pixel(picture, 3, 3));
        assert!(middle[0] > 100 && middle[2] == 0, "the near red wall shows in the middle: {middle:?}");
        assert!(corner[2] > 100 && corner[0] == 0, "the far blue wall shows around it: {corner:?}");
        assert!((picture.depth_at(80, 45).unwrap() - 4.0).abs() < 1e-4);
        assert!((picture.depth_at(3, 3).unwrap() - 9.0).abs() < 1e-4);
        // The near wall is 2 m wide at 4 m: 80 -/+ 1 / 4 x focal length (160 / 2 / tan 30) is from 45.4 to 114.6.
        let red: Vec<usize> = (0..160).filter(|&x| pixel(picture, x, 45)[0] > 100).collect();
        assert_eq!(red, (45..=114).collect::<Vec<_>>());
    }

    #[test]
    fn depth_is_distance_along_the_view_and_none_on_the_background() {
        let camera = Camera::look([0.0, -2.0, 0.0], 0.0, 0.0, 60.0);
        let mut frame = sky(160, 90);
        draw_mesh(&mut frame, &camera, &wall(5.0, 3.0, [200; 3]), &NO_FOG);
        assert!((frame.depth_at(80, 45).unwrap() - 7.0).abs() < 1e-4);
        // Depth is measured along the view, not along the ray, so a flat wall is equally deep all over.
        assert!((frame.depth_at(40, 20).unwrap() - 7.0).abs() < 1e-4);
        assert_eq!(frame.depth_at(1, 45), None);
        assert_eq!(pixel(&frame, 1, 45), [SKY[0], SKY[1], SKY[2], 255]);
        for (x, y) in [(160, 45), (80, 90), (usize::MAX, 0), (0, usize::MAX), (usize::MAX, usize::MAX)] {
            assert_eq!(frame.depth_at(x, y), None);
        }

        // A slanted floor: the depth at each pixel is where the ray through the pixel's centre meets it.
        let camera = Camera::look([0.3, -1.0, 1.6], 20.0, -35.0, 70.0);
        let mut frame = sky(160, 90);
        draw_scene(&mut frame, &camera, &Scene::new(&floor(40.0, [200; 3])), &NO_FOG);
        for (x, y) in [(0, 0), (80, 45), (159, 89), (17, 71), (140, 8)] {
            let ray = camera.ray(x as f32 + 0.5, y as f32 + 0.5, 160, 90);
            let expected = -camera.eye[2] / ray[2] * dot(ray, camera.forward);
            let depth = frame.depth_at(x, y).unwrap();
            assert!((depth - expected).abs() < expected * 2e-3, "({x}, {y}): {depth} for {expected}");
        }
    }

    #[test]
    fn a_floor_that_passes_under_the_camera_fills_the_bottom_and_none_of_the_sky() {
        let camera = Camera::look([0.0, 0.0, 1.6], 0.0, 0.0, 60.0);
        for half in [50.0, 3000.0, 1e6] {
            let mesh = floor(half, [200; 3]);
            let mut frame = sky(160, 90);
            draw_scene(&mut frame, &camera, &Scene::new(&mesh), &NO_FOG);
            let mut immediate = sky(160, 90);
            draw_mesh(&mut immediate, &camera, &mesh, &NO_FOG);
            assert_eq!(frame.rgba, immediate.rgba);
            assert_eq!(depth_bits(&frame), depth_bits(&immediate));
            // The horizon is the middle of the picture; the floor's far edge is a little below it.
            for y in 0..45 {
                assert!((0..160).all(|x| frame.depth_at(x, y).is_none()), "floor of {half} m drawn in the sky at row {y}");
            }
            for y in 50..90 {
                assert!((0..160).all(|x| pixel(&frame, x, y) == [183, 183, 183, 255]), "floor of {half} m missing at row {y}");
            }
            // 1.6 m below the eye, seen 44.5 pixels below the horizon with a focal length of 138.56 pixels.
            let depth = frame.depth_at(80, 89).unwrap();
            assert!((depth - 1.6 * 138.564 / 44.5).abs() < 0.01, "floor of {half} m: {depth}");
        }
    }

    #[test]
    fn what_cannot_be_seen_writes_nothing() {
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let mut unseen = Mesh::default();
        let grey = [128; 3];
        // Behind the camera, off to each side, nearer than the near plane, and beyond the far corner.
        unseen.append(&wall(-3.0, 5.0, grey));
        unseen.push_triangle([30.0, 5.0, 0.0], [40.0, 5.0, 0.0], [35.0, 5.0, 5.0], grey);
        unseen.push_triangle([-30.0, 5.0, 0.0], [-40.0, 5.0, 0.0], [-35.0, 5.0, 5.0], grey);
        unseen.push_triangle([0.0, 5.0, 20.0], [1.0, 5.0, 20.0], [0.0, 5.0, 25.0], grey);
        unseen.push_triangle([0.0, 5.0, -20.0], [1.0, 5.0, -20.0], [0.0, 5.0, -25.0], grey);
        unseen.append(&wall(0.04, 5.0, grey));
        // No area: three corners in a line, two corners the same, all the same, and edge-on to the camera.
        unseen.push_triangle([-1.0, 5.0, -1.0], [0.0, 5.0, 0.0], [1.0, 5.0, 1.0], grey);
        unseen.push_triangle([-1.0, 5.0, 0.0], [-1.0, 5.0, 0.0], [1.0, 5.0, 1.0], grey);
        unseen.push_triangle([0.0, 5.0, 0.0], [0.0, 5.0, 0.0], [0.0, 5.0, 0.0], grey);
        unseen.push_triangle([0.0, 2.0, -1.0], [0.0, 9.0, -1.0], [0.0, 5.0, 1.0], grey);
        // Too small to reach any pixel's centre, and thin enough to pass between them.
        unseen.push_triangle([0.001, 5.0, 0.001], [0.002, 5.0, 0.001], [0.001, 5.0, 0.002], grey);
        unseen.push_triangle([-3.0, 5.0, 0.004], [3.0, 5.0, 0.004], [3.0, 5.0, 0.005], grey);
        // Not usable at all.
        unseen.push_triangle([f32::NAN, 5.0, 0.0], [1.0, 5.0, 0.0], [0.0, 5.0, 1.0], grey);
        unseen.push_triangle([f32::INFINITY, 5.0, 0.0], [1.0, 5.0, 0.0], [0.0, 5.0, 1.0], grey);
        unseen.triangles.push([0, 1, 4000]);
        unseen.colors.push(grey);

        let scene = Scene::new(&unseen);
        assert_eq!(scene.triangle_count(), unseen.triangles.len() - 3, "the unusable ones are left out");
        let mut frame = sky(160, 90);
        draw_scene(&mut frame, &camera, &scene, &NO_FOG);
        assert!(is_untouched(&frame));
        draw_mesh(&mut frame, &camera, &unseen, &NO_FOG);
        assert!(is_untouched(&frame));
        for threads in [2, 5] {
            draw_scene_with_threads(&mut frame, &camera, &scene, &NO_FOG, threads);
            assert!(is_untouched(&frame));
        }

        let empty = Scene::new(&Mesh::default());
        assert_eq!((empty.triangle_count(), empty.bounds()), (0, None));
        draw_scene(&mut frame, &camera, &empty, &NO_FOG);
        assert!(is_untouched(&frame));
        let bounds = Scene::new(&wall(5.0, 2.0, grey)).bounds().unwrap();
        assert_eq!((bounds.min, bounds.max), ([-2.0, 5.0, -2.0], [2.0, 5.0, 2.0]));
    }

    #[test]
    fn odd_frames_cameras_and_coordinates_do_no_harm() {
        let mesh = soup(300, 3);
        let scene = Scene::new(&mesh);
        let good = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        for (width, height) in [(0, 0), (0, 5), (5, 0), (1, 1), (1, 300), (300, 1), (3, 2)] {
            let mut frame = Frame::new(width, height);
            for threads in [1, 3] {
                draw_scene_with_threads(&mut frame, &good, &scene, &NO_FOG, threads);
            }
            draw_mesh(&mut frame, &good, &mesh, &NO_FOG);
            draw_line(&mut frame, &good, [-1.0, 3.0, -1.0], [1.0, 3.0, 1.0], [255; 4], 2);
        }
        // A frame whose fields were set by hand and no longer agree is left alone.
        let mut broken = Frame::new(8, 8);
        broken.width = 80;
        draw_scene(&mut broken, &good, &scene, &NO_FOG);
        draw_mesh(&mut broken, &good, &mesh, &NO_FOG);
        draw_line(&mut broken, &good, [-1.0, 3.0, -1.0], [1.0, 3.0, 1.0], [255; 4], 1);
        assert!(pixels(&broken).all(|p| p == [0, 0, 0, 255]));

        let mut rng = Rng(99);
        let odd = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            1e-30,
            1e30,
            -1e30,
            f32::MAX,
            f32::MIN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            0.05,
            1e-6,
        ];
        let mut frame = sky(64, 36);
        for round in 0..400 {
            let mut pick = || if rng.below(3) == 0 { odd[rng.below(odd.len())] } else { rng.range(-5.0, 5.0) };
            let mut camera = Camera::look([pick(), pick(), pick()], pick() * 50.0, pick() * 20.0, pick() * 30.0);
            if round % 4 == 0 {
                camera = Camera {
                    eye: [pick(), pick(), pick()],
                    right: [pick(), pick(), pick()],
                    up: [pick(), pick(), pick()],
                    forward: [pick(), pick(), pick()],
                    tan_half_h: pick(),
                    near: pick(),
                };
            }
            let mut wild = Mesh::default();
            for _ in 0..6 {
                wild.push_triangle([pick(), pick(), pick()], [pick(), pick(), pick()], [pick(), pick(), pick()], [200, 100, 50]);
            }
            let look = Look { fog_color: [9; 3], fog_start: pick(), fog_end: pick(), fog_strength: pick() };
            draw_scene_with_threads(&mut frame, &camera, &Scene::new(&wild), &look, 1 + round % 3);
            draw_mesh(&mut frame, &camera, &wild, &look);
            draw_line(
                &mut frame,
                &camera,
                [pick(), pick(), pick()],
                [pick(), pick(), pick()],
                [255, 0, 0, 200],
                1 + (round % 3) as u32,
            );
            assert!(pixels(&frame).all(|p| p[3] == 255));
            assert!(frame.depth.iter().all(|d| *d > 0.0), "depths stay in front of the eye");
        }

        // Corners a very long way off still leave the near part of the triangle in the right place.
        let camera = Camera::look([0.0, 0.0, 1.0], 0.0, 0.0, 60.0);
        let mut far_reaching = Mesh::default();
        far_reaching.push_triangle([-1.0, 2.0, 0.0], [1.0, 2.0, 0.0], [0.0, 3e30, 0.0], [200; 3]);
        let mut frame = sky(160, 90);
        draw_scene(&mut frame, &camera, &Scene::new(&far_reaching), &NO_FOG);
        assert_eq!(pixel(&frame, 80, 88), [183, 183, 183, 255]);
        assert!(frame.depth_at(80, 44).is_none() && frame.depth_at(10, 80).is_none());
    }

    #[test]
    fn faces_are_lit_on_whichever_side_is_seen() {
        let grey = [200; 3];
        let above = Camera::look_at([0.2, 0.2, 5.0], [0.2, 0.2, 0.0], 60.0);
        let below = Camera::look_at([0.2, 0.2, -5.0], [0.2, 0.2, 0.0], 60.0);
        let (a, b, c) = ([-2.0, -2.0, 0.0], [2.0, -2.0, 0.0], [0.0, 2.0, 0.0]);
        for corners in [[a, b, c], [a, c, b]] {
            let mut mesh = Mesh::default();
            mesh.push_triangle(corners[0], corners[1], corners[2], grey);
            for (camera, expected) in [(&above, 183), (&below, 76)] {
                let mut frame = sky(64, 36);
                draw_scene(&mut frame, camera, &Scene::new(&mesh), &NO_FOG);
                assert_eq!(pixel(&frame, 32, 18), [expected, expected, expected, 255]);
                let mut frame = sky(64, 36);
                draw_mesh(&mut frame, camera, &mesh, &NO_FOG);
                assert_eq!(pixel(&frame, 32, 18), [expected, expected, expected, 255]);
            }
        }
        // 0.38 + 0.42 x (n . key) + 0.20 x (0.5 + 0.5 x n.z), with the key light along (0.35, -0.5, 0.8) / 1.00623.
        let seen_from_origin = |mesh: &Mesh, yaw: f32| {
            let mut frame = sky(64, 36);
            draw_scene(&mut frame, &Camera::look([0.0; 3], yaw, 0.0, 60.0), &Scene::new(mesh), &NO_FOG);
            pixel(&frame, 32, 18)[0]
        };
        assert_eq!(seen_from_origin(&wall(5.0, 3.0, grey), 0.0), 138, "facing -Y: 0.38 + 0.42 x 0.4969 + 0.10");
        assert_eq!(seen_from_origin(&wall(-5.0, 3.0, grey), 180.0), 96, "facing +Y, away from the light: 0.38 + 0.10");
        let mut side = Mesh::default();
        side.push_quad([[5.0, -3.0, -3.0], [5.0, 3.0, -3.0], [5.0, 3.0, 3.0], [5.0, -3.0, 3.0]], grey);
        assert_eq!(seen_from_origin(&side, -90.0), 96, "facing -X, away from the light");
        let mut other_side = Mesh::default();
        other_side.push_quad([[-5.0, -3.0, -3.0], [-5.0, 3.0, -3.0], [-5.0, 3.0, 3.0], [-5.0, -3.0, 3.0]], grey);
        assert_eq!(seen_from_origin(&other_side, 90.0), 125, "facing +X: 0.38 + 0.42 x 0.3478 + 0.10");
        let mut bright = Mesh::default();
        bright.push_triangle(a, b, c, [255, 0, 100]);
        bright.colors.clear();
        let mut frame = sky(64, 36);
        draw_mesh(&mut frame, &above, &bright, &NO_FOG);
        assert_eq!(pixel(&frame, 32, 18), [155, 155, 155, 255], "a triangle without a colour is drawn in the default grey");
    }

    #[test]
    fn fog_thickens_with_distance() {
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let look = Look { fog_color: [255, 255, 0], fog_start: 5.0, fog_end: 15.0, fog_strength: 0.5 };
        let seen = |distance: f32, look: &Look| {
            let mut frame = sky(64, 36);
            draw_scene(&mut frame, &camera, &Scene::new(&wall(distance, distance, [200; 3])), look);
            let mut immediate = sky(64, 36);
            draw_mesh(&mut immediate, &camera, &wall(distance, distance, [200; 3]), look);
            assert_eq!(frame.rgba, immediate.rgba);
            pixel(&frame, 32, 18)
        };
        assert_eq!(seen(4.0, &look), [138, 138, 138, 255], "no fog before it starts");
        // Half way from start to end at half strength is a quarter fog: 138 + (255 - 138) / 4, and 138 x 3 / 4.
        assert_eq!(seen(10.02, &look), [167, 167, 104, 255]);
        assert_eq!(seen(15.5, &look), [197, 197, 69, 255]);
        assert_eq!(seen(400.0, &look), [197, 197, 69, 255], "never more than its strength");
        assert_eq!(seen(400.0, &Look { fog_strength: 1.0, ..look }), [255, 255, 0, 255]);
        assert_eq!(seen(400.0, &Look { fog_strength: 7.0, ..look }), [255, 255, 0, 255]);
        for off in [
            Look::default(),
            Look { fog_end: 5.0, ..look },
            Look { fog_end: 2.0, ..look },
            Look { fog_strength: 0.0, ..look },
            Look { fog_strength: f32::NAN, ..look },
        ] {
            assert_eq!(seen(400.0, &off), [138, 138, 138, 255]);
        }
        let default = Look::default();
        assert!(default.fog_end <= default.fog_start, "fog is off unless asked for");
    }

    /// A flat sheet of `cells` x `cells` squares cut into triangles, its inner corners nudged off the grid.
    fn sheet(cells: usize, half: f32, place: impl Fn(f32, f32) -> [f32; 3]) -> Mesh {
        let mut rng = Rng(42);
        let step = 2.0 * half / cells as f32;
        let mut mesh = Mesh::default();
        for row in 0..=cells {
            for column in 0..=cells {
                let inner = row > 0 && row < cells && column > 0 && column < cells;
                let nudge = if inner { [rng.range(-0.2, 0.2) * step, rng.range(-0.2, 0.2) * step] } else { [0.0; 2] };
                mesh.positions.push(place(-half + column as f32 * step + nudge[0], -half + row as f32 * step + nudge[1]));
            }
        }
        for row in 0..cells {
            for column in 0..cells {
                let corner = (row * (cells + 1) + column) as u32;
                let (right, above) = (corner + 1, corner + cells as u32 + 1);
                // The diagonal alternates, and some triangles are wound the other way round.
                let pair = if (row + column) % 2 == 0 {
                    [[corner, right, above + 1], [corner, above + 1, above]]
                } else {
                    [[corner, right, above], [right, above + 1, above]]
                };
                for (at, triangle) in pair.into_iter().enumerate() {
                    mesh.triangles.push(if (row * 3 + column + at) % 4 == 0 {
                        [triangle[0], triangle[2], triangle[1]]
                    } else {
                        triangle
                    });
                    mesh.colors.push([200; 3]);
                }
            }
        }
        mesh
    }

    /// How many of the mesh's triangles, each drawn on its own, cover each pixel.
    fn coverage(camera: &Camera, mesh: &Mesh, width: usize, height: usize) -> Vec<u32> {
        let mut covered = vec![0; width * height];
        let mut frame = sky(width, height);
        for at in 0..mesh.triangles.len() {
            let one =
                Mesh { positions: mesh.positions.clone(), triangles: vec![mesh.triangles[at]], colors: vec![mesh.colors[at]] };
            draw_mesh(&mut frame, camera, &one, &NO_FOG);
            for (count, depth) in covered.iter_mut().zip(&mut frame.depth) {
                *count += u32::from(depth.is_finite());
                *depth = f32::INFINITY;
            }
        }
        covered
    }

    #[test]
    fn triangles_that_share_an_edge_leave_no_gap_and_no_overlap() {
        // A wall bigger than the view, turned so that no edge runs along the pixel grid.
        let tilted_wall = sheet(14, 9.0, |u, v| [u * 0.8 + v * 0.1, 6.0 + u * 0.5 - v * 0.3, v * 0.9 - u * 0.2]);
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let covered = coverage(&camera, &tilted_wall, 160, 90);
        assert!(
            covered.iter().all(|&count| count == 1),
            "{} pixels missed, {} drawn twice",
            covered.iter().filter(|c| **c == 0).count(),
            covered.iter().filter(|c| **c > 1).count()
        );

        // A floor that runs under and behind the camera, so many shared edges are cut by the near plane,
        // and one so large that they are cut at the sides too.
        let camera = Camera::look([0.3, 0.2, 1.6], 25.0, -30.0, 60.0);
        for half in [40.0, 4e5] {
            let ground = sheet(16, half, |u, v| [u, v, 0.0]);
            let covered = coverage(&camera, &ground, 160, 90);
            assert!(
                covered.iter().all(|&count| count == 1),
                "floor of {half} m: {} pixels missed, {} drawn twice",
                covered.iter().filter(|c| **c == 0).count(),
                covered.iter().filter(|c| **c > 1).count()
            );
            let mut frame = sky(160, 90);
            draw_scene(&mut frame, &camera, &Scene::new(&ground), &NO_FOG);
            assert!(pixels(&frame).all(|p| p == [183, 183, 183, 255]), "background shows through the floor of {half} m");
        }

        // Corners exactly on pixel centres, edges exactly through them: each pixel still has one owner.
        let camera = Camera {
            eye: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            tan_half_h: 1.0,
            near: 0.05,
        };
        let on_the_grid = sheet(1, 0.5, |u, v| [u + 0.0125, 1.0, v + 0.0125]);
        let covered = coverage(&camera, &on_the_grid, 80, 80);
        assert_eq!(covered.iter().filter(|c| **c == 1).count(), 40 * 40);
        assert_eq!(covered.iter().filter(|c| **c > 1).count(), 0);
    }

    #[test]
    fn the_picture_is_the_same_on_any_number_of_threads() {
        let mut mesh = soup(6000, 2024);
        mesh.append(&floor(60.0, [90, 120, 90]).transformed(&crate::mesh::Transform::translation([0.0, 0.0, -1.5])));
        let scene = Scene::new(&mesh);
        assert!(scene.triangle_count() >= SMALL_SCENE);
        let look = Look { fog_color: [150, 170, 190], fog_start: 2.0, fog_end: 30.0, fog_strength: 0.6 };
        let cameras = [
            Camera::look([0.0; 3], 30.0, -10.0, 70.0),
            Camera::look([3.0, -14.0, 2.0], -20.0, 5.0, 45.0),
            Camera::look_at([0.0, 0.0, 25.0], [0.0; 3], 100.0),
        ];
        for camera in &cameras {
            let mut single = sky(323, 187);
            draw_scene_with_threads(&mut single, camera, &scene, &look, 1);
            assert!(single.depth.iter().filter(|d| d.is_finite()).count() > 323 * 187 / 2);
            for threads in [2, 3, 4, 7, 8, 64, 1000] {
                let mut parallel = sky(323, 187);
                draw_scene_with_threads(&mut parallel, camera, &scene, &look, threads);
                assert!(parallel.rgba == single.rgba, "{threads} threads gave another picture");
                assert!(depth_bits(&parallel) == depth_bits(&single), "{threads} threads gave other depths");
            }
            let mut automatic = sky(323, 187);
            draw_scene(&mut automatic, camera, &scene, &look);
            assert!(automatic.rgba == single.rgba && depth_bits(&automatic) == depth_bits(&single));
            // Drawing again into a used frame, as every frame after the first does, changes nothing.
            draw_scene(&mut automatic, camera, &scene, &look);
            assert!(automatic.rgba == single.rgba && depth_bits(&automatic) == depth_bits(&single));
            let mut immediate = sky(323, 187);
            draw_mesh(&mut immediate, camera, &mesh, &look);
            assert!(
                immediate.rgba == single.rgba && depth_bits(&immediate) == depth_bits(&single),
                "draw_mesh and draw_scene differ"
            );
        }
    }

    #[test]
    fn lines_are_hidden_behind_walls_and_shown_in_front() {
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let mut walled = sky(160, 90);
        draw_mesh(&mut walled, &camera, &wall(5.0, 1.0, [200; 3]), &NO_FOG);
        let red = [255, 0, 0, 255];

        let mut frame = Frame { width: 160, height: 90, rgba: walled.rgba.clone(), depth: walled.depth.clone() };
        draw_line(&mut frame, &camera, [-0.9, 6.0, -0.9], [0.9, 6.0, 0.7], red, 2);
        draw_line(&mut frame, &camera, [0.5, 5.2, -0.8], [0.5, 40.0, 0.8], red, 1);
        assert_eq!(frame.rgba, walled.rgba, "a line behind the wall shows through it");

        // A line across the whole view behind the wall shows on both sides of it and nowhere on it.
        draw_line(&mut frame, &camera, [-6.0, 8.0, 0.0], [6.0, 8.0, 0.0], red, 1);
        let reds: Vec<usize> = (0..160).filter(|&x| (0..90).any(|y| pixel(&frame, x, y) == red)).collect();
        // The wall's edges are at 80 -/+ 138.56 / 5 = 52.3 and 107.7; the line's ends at 80 -/+ 103.9.
        assert_eq!(reds, (0..52).chain(108..160).collect::<Vec<_>>());

        let mut frame = Frame { width: 160, height: 90, rgba: walled.rgba.clone(), depth: walled.depth.clone() };
        draw_line(&mut frame, &camera, [-0.9, 4.0, -0.5], [0.9, 4.0, 0.5], red, 1);
        // From x = 80 - 0.9 / 4 x 138.56 = 48.8 to 111.2: one pixel in each column whose centre is passed.
        for x in 0..160 {
            let count = (0..90).filter(|&y| pixel(&frame, x, y) == red).count();
            assert_eq!(count, usize::from((49..=110).contains(&x)), "column {x}");
        }
        assert_eq!(depth_bits(&frame), depth_bits(&walled), "lines leave the depths alone");

        // Twice as wide is two pixels in each column, and the same line drawn steeply two in each row.
        let mut wide = sky(160, 90);
        draw_line(&mut wide, &camera, [-0.9, 4.0, -0.5], [0.9, 4.0, 0.5], red, 2);
        assert!((49..=110).all(|x| (0..90).filter(|&y| pixel(&wide, x, y) == red).count() == 2));
        let mut steep = sky(160, 90);
        draw_line(&mut steep, &camera, [-0.3, 4.0, -1.0], [0.2, 4.0, 1.0], red, 2);
        for y in 0..90 {
            // From y = 45 + 138.56 / 4 = 79.6 up to 10.4: the rows whose centres lie between.
            let count = (0..160).filter(|&x| pixel(&steep, x, y) == red).count();
            assert_eq!(count, if (10..=79).contains(&y) { 2 } else { 0 }, "row {y}");
        }

        // Half-transparent lines mix with what is there; fully transparent ones leave it alone.
        let mut frame = sky(160, 90);
        frame.clear([100, 200, 50]);
        draw_line(&mut frame, &camera, [-0.9, 4.0, 0.0], [0.9, 4.0, 0.0], [200, 100, 250, 128], 1);
        draw_line(&mut frame, &camera, [-0.9, 4.0, 0.5], [0.9, 4.0, 0.5], [200, 100, 250, 0], 1);
        // The line runs exactly between two rows of pixels; it is drawn in one of them.
        let mixed = (0..90).filter(|&y| pixel(&frame, 80, y) != [100, 200, 50, 255]).collect::<Vec<_>>();
        assert_eq!(mixed.len(), 1);
        assert!(mixed[0] == 44 || mixed[0] == 45);
        assert_eq!(pixel(&frame, 80, mixed[0]), [150, 150, 150, 255]);
    }

    #[test]
    fn lines_are_cut_at_the_near_plane_and_short_ones_leave_a_dot() {
        let camera = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
        let white = [255; 4];
        let count = |frame: &Frame| pixels(frame).filter(|p| *p == white).count();

        // From behind the camera to the middle of the view: only the part in front is drawn.
        let mut frame = sky(160, 90);
        draw_line(&mut frame, &camera, [0.4, -3.0, -0.2], [0.0, 5.0, 0.0], white, 1);
        let mut reversed = sky(160, 90);
        draw_line(&mut reversed, &camera, [0.0, 5.0, 0.0], [0.4, -3.0, -0.2], white, 1);
        assert_eq!(frame.rgba, reversed.rgba);
        assert!(count(&frame) >= 79, "{}", count(&frame));
        assert_eq!(pixel(&frame, 80, 45), white);
        assert!(
            (0..160 * 90).all(|at| pixel(&frame, at % 160, at / 160) != white || (at % 160 >= 80 && at / 160 >= 45)),
            "the line leaves toward the lower right"
        );

        let mut frame = sky(160, 90);
        draw_line(&mut frame, &camera, [0.4, -3.0, -0.2], [0.0, -0.1, 0.0], white, 2);
        draw_line(&mut frame, &camera, [0.4, 0.01, -0.2], [0.0, 0.04, 0.0], white, 2);
        draw_line(&mut frame, &camera, [50.0, 3.0, 1.0], [60.0, 3.0, 2.0], white, 2);
        assert!(is_untouched(&frame), "lines wholly behind, too near or off to the side draw nothing");

        let mut frame = sky(160, 90);
        draw_line(&mut frame, &camera, [0.001, 5.0, 0.001], [0.002, 5.0, 0.0015], white, 1);
        draw_line(&mut frame, &camera, [1.0, 5.0, 1.0], [1.0, 5.0, 1.0], white, 1);
        assert_eq!(count(&frame), 2);
        assert_eq!(pixel(&frame, 80, 44), white);

        // A line that leaves the frame a long way is cut to it, whichever way it is given.
        let mut frame = sky(160, 90);
        draw_line(&mut frame, &camera, [-1e6, 5.0, -1e6], [1e6, 5.0, 1e6], white, 1);
        assert_eq!(count(&frame), 90);
        assert_eq!((pixel(&frame, 35, 89), pixel(&frame, 80, 44), pixel(&frame, 124, 0)), (white, white, white));
    }

    #[test]
    fn a_line_lying_on_a_floor_seen_at_a_low_angle_is_unbroken() {
        let ground = floor(100.0, [200; 3]);
        let red = [255, 0, 0, 255];
        for (pitch, height) in [(-3.0, 0.4), (-8.0, 1.6), (-25.0, 1.6), (-60.0, 3.0)] {
            let camera = Camera::look([0.0, 0.0, height], 12.0, pitch, 60.0);
            let mut drawn = sky(320, 180);
            draw_scene(&mut drawn, &camera, &Scene::new(&ground), &NO_FOG);
            let lines = [
                ([-1.0, 1.0, 0.0], [-9.0, 60.0, 0.0]),
                ([-30.0, 25.0, 0.0], [30.0, 31.0, 0.0]),
                ([2.0, 0.5, 0.0], [-4.0, 9.0, 0.0]),
                ([-50.0, 80.0, 0.0], [5.0, -2.0, 0.0]),
                ([0.5, -5.0, 0.0], [0.5, 90.0, 0.0]),
            ];
            let mut seen = 0;
            for (a, b) in lines {
                let mut frame = Frame { width: 320, height: 180, rgba: drawn.rgba.clone(), depth: drawn.depth.clone() };
                draw_line(&mut frame, &camera, a, b, red, 1);
                let mut bare = sky(320, 180);
                draw_line(&mut bare, &camera, a, b, red, 1);
                let shown = pixels(&frame).filter(|p| *p == red).count();
                let whole = pixels(&bare).filter(|p| *p == red).count();
                assert_eq!(shown, whole, "pitch {pitch}: the floor hid part of the line from {a:?} to {b:?}");
                seen += whole;
            }
            assert!(seen > 300, "pitch {pitch}: the test lines should cross the picture, but cover only {seen} pixels");
            // The same lines just under the floor are hidden, apart from where they are within the bias of it.
            let mut frame = Frame { width: 320, height: 180, rgba: drawn.rgba.clone(), depth: drawn.depth.clone() };
            draw_line(&mut frame, &camera, [-3.0, 2.0, -0.5], [3.0, 6.0, -0.5], red, 2);
            assert_eq!(frame.rgba, drawn.rgba);
        }
    }

    #[test]
    fn a_fixed_scene_always_gives_the_same_picture() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([-6.0, -8.0, -0.1], [6.0, 12.0, 0.0], [52, 56, 68]);
        mesh.push_cuboid([-6.2, 12.0, 0.0], [6.2, 12.2, 5.0], [206, 200, 188]);
        mesh.push_cuboid([-4.0, 8.0, 0.0], [4.0, 11.0, 0.6], [60, 42, 30]);
        for at in 0..40 {
            let (x, y) = (-4.5 + (at % 8) as f32 * 1.25, -1.0 + (at / 8) as f32 * 1.5);
            mesh.push_cuboid([x, y, 0.0], [x + 0.5, y + 0.5, 0.45 + 0.05 * (at % 3) as f32], [150, 30 + at as u8 * 4, 40]);
        }
        mesh.push_triangle([-3.0, 9.0, 0.6], [-2.0, 9.5, 2.6], [-1.0, 9.0, 0.6], [240, 240, 235]);
        // Orthonormal axes with exact decimal fractions, so no sine or cosine goes into the picture.
        let camera = Camera {
            eye: [3.5, -6.5, 2.5],
            right: [0.8, 0.6, 0.0],
            up: [-0.12, 0.16, 0.98],
            forward: [-0.588, 0.784, -0.2],
            tan_half_h: 0.5,
            near: 0.05,
        };
        let look = Look { fog_color: [150, 170, 190], fog_start: 4.0, fog_end: 24.0, fog_strength: 0.5 };
        let mut frame = Frame::new(200, 120);
        frame.clear([150, 170, 190]);
        draw_scene(&mut frame, &camera, &Scene::new(&mesh), &look);
        let mut marker = Mesh::default();
        marker.push_cuboid([-0.2, 2.0, 1.0], [0.2, 2.4, 1.4], [250, 200, 20]);
        draw_mesh(&mut frame, &camera, &marker, &look);
        draw_line(&mut frame, &camera, [-6.0, -8.0, 0.0], [6.0, 12.0, 0.0], [255, 255, 0, 160], 2);
        draw_line(&mut frame, &camera, [0.0, 2.2, 1.2], [0.0, 30.0, 1.2], [0, 255, 255, 255], 1);

        let drawn = frame.depth.iter().filter(|d| d.is_finite()).count();
        assert!(drawn > 200 * 120 / 2, "only {drawn} pixels drawn");
        let checksum = fnv1a(frame.rgba.iter().copied().chain(frame.depth.iter().flat_map(|d| d.to_bits().to_le_bytes())));
        assert_eq!(checksum, 0x54DE_C869_662D_FA47, "the renderer's output changed: {checksum:#018X}");
    }
}
