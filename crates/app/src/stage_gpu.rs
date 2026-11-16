//! Native preview renderer. OpenGL work stays on eframe's current-context thread.
//! The CPU renderer remains the fallback; mesh files and camera coordinates are unchanged.

use eframe::glow::{self, HasContext};
use ptz_scene::mesh::{Bounds, Mesh, DEFAULT_COLOR};
use ptz_scene::render::{self, Camera, Frame, Look};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub type Line = ([f32; 3], [f32; 3], [u8; 4], u32);
const SHADOW_SIDE: i32 = 2048;
const FLOATS_PER_VERTEX: usize = 9;
const SMOOTH_LIMIT: usize = 200_000;
const LIGHT: [f32; 3] = [-0.26726124, -0.5345225, 0.8017837];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| a[k] - b[k])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length > 1e-12 {
        v.map(|n| n / length)
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn matrix(rows: [[f32; 4]; 4]) -> [f32; 16] {
    std::array::from_fn(|i| rows[i % 4][i / 4])
}

/// Uses the supplied basis directly, including at exactly straight up or down.
fn projection(view: &Camera, aspect: f32, far: f32) -> [f32; 16] {
    let near = view.near.max(0.0001);
    let a = (far + near) / (far - near);
    let b = 2.0 * far * near / (far - near);
    let row = |basis: [f32; 3], scale: f32| [basis[0] * scale, basis[1] * scale, basis[2] * scale, -dot(basis, view.eye) * scale];
    let mut depth = row(view.forward, a);
    depth[3] -= b;
    matrix([row(view.right, 1.0 / view.tan_half_h), row(view.up, aspect / view.tan_half_h), depth, row(view.forward, 1.0)])
}

fn corners(bounds: Bounds) -> [[f32; 3]; 8] {
    std::array::from_fn(|i| [0, 1, 2].map(|k| if i >> k & 1 == 0 { bounds.min[k] } else { bounds.max[k] }))
}

fn combined_bounds(mut bounds: Option<Bounds>, markers: &[Mesh]) -> Bounds {
    for mesh in markers {
        if let Some(other) = mesh.bounds() {
            bounds = Some(match bounds {
                Some(b) => Bounds {
                    min: [0, 1, 2].map(|k| b.min[k].min(other.min[k])),
                    max: [0, 1, 2].map(|k| b.max[k].max(other.max[k])),
                },
                None => other,
            });
        }
    }
    bounds.unwrap_or(Bounds { min: [-1.0; 3], max: [1.0; 3] })
}

fn shadow_projection(bounds: Bounds) -> [f32; 16] {
    let forward = LIGHT.map(|v| -v);
    let right = normalized(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    let points = corners(bounds);
    let row = |basis| {
        let min = points.iter().map(|p| dot(*p, basis)).fold(f32::INFINITY, f32::min);
        let max = points.iter().map(|p| dot(*p, basis)).fold(f32::NEG_INFINITY, f32::max);
        let middle = (min + max) * 0.5;
        let half = ((max - min) * 0.51 + 0.15).max(0.5);
        [basis[0] / half, basis[1] / half, basis[2] / half, -middle / half]
    };
    matrix([row(right), row(up), row(forward), [0.0, 0.0, 0.0, 1.0]])
}

#[derive(Clone)]
struct NormalGroup {
    first: [f32; 3],
    sum: [f32; 3],
}
fn position_key(p: [f32; 3]) -> [i64; 3] {
    p.map(|n| (f64::from(n) * 100_000.0).round() as i64)
}

/// Vertex preparation is linear and bounded. Small procedural models have crease-aware
/// normals; very large imported meshes retain their face normals to bound memory/time.
fn vertices(mesh: &Mesh) -> Vec<f32> {
    let smooth = mesh.triangles.len() <= SMOOTH_LIMIT;
    let mut groups: HashMap<[i64; 3], Vec<NormalGroup>> = HashMap::new();
    let faces: Vec<[f32; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            let p = t.map(|i| mesh.positions[i as usize]);
            normalized(cross(sub(p[1], p[0]), sub(p[2], p[0])))
        })
        .collect();
    if smooth {
        for (triangle, normal) in mesh.triangles.iter().zip(&faces) {
            for index in triangle {
                let at = groups.entry(position_key(mesh.positions[*index as usize])).or_default();
                if let Some(group) = at.iter_mut().find(|g| dot(g.first, *normal) > 0.7072) {
                    for (sum, n) in group.sum.iter_mut().zip(normal) {
                        *sum += n;
                    }
                } else if at.len() < 16 {
                    at.push(NormalGroup { first: *normal, sum: *normal });
                }
            }
        }
    }
    let mut result = Vec::with_capacity(mesh.triangles.len() * 3 * FLOATS_PER_VERTEX);
    for (i, (triangle, face)) in mesh.triangles.iter().zip(&faces).enumerate() {
        let color = mesh.colors.get(i).copied().unwrap_or(DEFAULT_COLOR).map(|v| v as f32 / 255.0);
        for index in triangle {
            let point = mesh.positions[*index as usize];
            let normal = groups
                .get(&position_key(point))
                .and_then(|g| g.iter().find(|g| dot(g.first, *face) > 0.7072))
                .map(|g| normalized(g.sum))
                .unwrap_or(*face);
            result.extend_from_slice(&point);
            result.extend_from_slice(&normal);
            result.extend_from_slice(&color);
        }
    }
    result
}

fn mesh_key(mesh: &Mesh) -> u64 {
    let mut key = std::collections::hash_map::DefaultHasher::new();
    for p in &mesh.positions {
        for value in p {
            value.to_bits().hash(&mut key);
        }
    }
    mesh.triangles.hash(&mut key);
    mesh.colors.hash(&mut key);
    key.finish()
}

fn bytes(values: &[f32]) -> &[u8] {
    // f32 has no padding; this is only an OpenGL upload of initialized values.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

#[derive(Default)]
struct Geometry {
    vao: Option<glow::VertexArray>,
    buffer: Option<glow::Buffer>,
    count: i32,
}
impl Geometry {
    unsafe fn upload(&mut self, gl: &glow::Context, data: &[f32], usage: u32) -> Result<(), String> {
        if self.vao.is_none() {
            self.vao = Some(gl.create_vertex_array()?);
        }
        if self.buffer.is_none() {
            self.buffer = Some(gl.create_buffer()?);
        }
        gl.bind_vertex_array(self.vao);
        gl.bind_buffer(glow::ARRAY_BUFFER, self.buffer);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(data), usage);
        for location in 0..3 {
            gl.enable_vertex_attrib_array(location);
            gl.vertex_attrib_pointer_f32(location, 3, glow::FLOAT, false, (FLOATS_PER_VERTEX * 4) as i32, location as i32 * 12);
        }
        self.count = (data.len() / FLOATS_PER_VERTEX).try_into().map_err(|_| "The scene has too many vertices")?;
        Ok(())
    }
    unsafe fn draw(&self, gl: &glow::Context) {
        if self.count > 0 {
            gl.bind_vertex_array(self.vao);
            gl.draw_arrays(glow::TRIANGLES, 0, self.count);
        }
    }
    unsafe fn destroy(&mut self, gl: &glow::Context) {
        if let Some(v) = self.vao.take() {
            gl.delete_vertex_array(v);
        }
        if let Some(v) = self.buffer.take() {
            gl.delete_buffer(v);
        }
        self.count = 0;
    }
}

#[derive(Default)]
struct Target {
    size: [i32; 2],
    samples: i32,
    resolved: Option<glow::Framebuffer>,
    color: Option<glow::Texture>,
    depth: Option<glow::Renderbuffer>,
    multisample: Option<glow::Framebuffer>,
    ms_color: Option<glow::Renderbuffer>,
    ms_depth: Option<glow::Renderbuffer>,
}
impl Target {
    unsafe fn resize(&mut self, gl: &glow::Context, size: [i32; 2], samples: i32) -> Result<(), String> {
        if self.size == size && self.resolved.is_some() {
            return Ok(());
        }
        self.destroy(gl);
        self.size = size;
        self.samples = samples;
        let [w, h] = size;
        self.resolved = Some(gl.create_framebuffer()?);
        self.color = Some(gl.create_texture()?);
        self.depth = Some(gl.create_renderbuffer()?);
        gl.bind_texture(glow::TEXTURE_2D, self.color);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.bind_renderbuffer(glow::RENDERBUFFER, self.depth);
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);
        gl.bind_framebuffer(glow::FRAMEBUFFER, self.resolved);
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, self.color, 0);
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, self.depth);
        framebuffer_ok(gl)?;
        if samples > 1 {
            self.multisample = Some(gl.create_framebuffer()?);
            self.ms_color = Some(gl.create_renderbuffer()?);
            self.ms_depth = Some(gl.create_renderbuffer()?);
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.multisample);
            for (buffer, format, attachment) in [
                (self.ms_color, glow::RGBA8, glow::COLOR_ATTACHMENT0),
                (self.ms_depth, glow::DEPTH_COMPONENT24, glow::DEPTH_ATTACHMENT),
            ] {
                gl.bind_renderbuffer(glow::RENDERBUFFER, buffer);
                gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, format, w, h);
                gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, attachment, glow::RENDERBUFFER, buffer);
            }
            framebuffer_ok(gl)?;
        }
        Ok(())
    }
    unsafe fn destroy(&mut self, gl: &glow::Context) {
        for f in [&mut self.resolved, &mut self.multisample] {
            if let Some(v) = f.take() {
                gl.delete_framebuffer(v);
            }
        }
        for b in [&mut self.depth, &mut self.ms_color, &mut self.ms_depth] {
            if let Some(v) = b.take() {
                gl.delete_renderbuffer(v);
            }
        }
        if let Some(v) = self.color.take() {
            gl.delete_texture(v);
        }
        self.size = [0, 0];
    }
}

#[derive(Default)]
struct Shadow {
    framebuffer: Option<glow::Framebuffer>,
    depth: Option<glow::Texture>,
    side: i32,
}
impl Shadow {
    unsafe fn create(&mut self, gl: &glow::Context) -> Result<(), String> {
        self.side = SHADOW_SIDE.min(gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE));
        self.framebuffer = Some(gl.create_framebuffer()?);
        self.depth = Some(gl.create_texture()?);
        gl.bind_texture(glow::TEXTURE_2D, self.depth);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::DEPTH_COMPONENT24 as i32,
            self.side,
            self.side,
            0,
            glow::DEPTH_COMPONENT,
            glow::UNSIGNED_INT,
            glow::PixelUnpackData::Slice(None),
        );
        for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::NEAREST as i32);
        }
        for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, glow::CLAMP_TO_EDGE as i32);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, self.framebuffer);
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, self.depth, 0);
        gl.draw_buffer(glow::NONE);
        gl.read_buffer(glow::NONE);
        framebuffer_ok(gl)
    }
    unsafe fn destroy(&mut self, gl: &glow::Context) {
        if let Some(v) = self.framebuffer.take() {
            gl.delete_framebuffer(v);
        }
        if let Some(v) = self.depth.take() {
            gl.delete_texture(v);
        }
    }
}

unsafe fn framebuffer_ok(gl: &glow::Context) -> Result<(), String> {
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    if status == glow::FRAMEBUFFER_COMPLETE {
        Ok(())
    } else {
        Err(format!("The preview framebuffer is incomplete ({status:#x})"))
    }
}

unsafe fn program(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    let result = (|| {
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind)?;
            shaders.push(shader);
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(format!("Preview shader: {}", gl.get_shader_info_log(shader)));
            }
            gl.attach_shader(program, shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(format!("Preview program: {}", gl.get_program_info_log(program)));
        }
        Ok(())
    })();
    for shader in shaders {
        gl.delete_shader(shader);
    }
    match result {
        Ok(()) => Ok(program),
        Err(e) => {
            gl.delete_program(program);
            Err(e)
        }
    }
}

/// State changed by this renderer. The caller's UI draw target and pixel transfer
/// buffers are restored even when allocation, shader compilation or drawing fails.
struct State {
    program: Option<glow::Program>,
    vao: Option<glow::VertexArray>,
    buffer: Option<glow::Buffer>,
    draw: Option<glow::Framebuffer>,
    read: Option<glow::Framebuffer>,
    renderbuffer: Option<glow::Renderbuffer>,
    pack_buffer: Option<glow::Buffer>,
    unpack_buffer: Option<glow::Buffer>,
    active_texture: u32,
    texture: Option<glow::Texture>,
    sampler: Option<glow::Sampler>,
    viewport: [i32; 4],
    pixel_store: Vec<(u32, i32)>,
    enables: Vec<(u32, bool)>,
    color_mask: [bool; 4],
    depth_mask: bool,
    depth_func: u32,
    clear_color: [f32; 4],
    clear_depth: f32,
    depth_range: [f32; 2],
    polygon_mode: u32,
}
impl State {
    unsafe fn save(gl: &glow::Context) -> Self {
        let active_texture = gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
        gl.active_texture(glow::TEXTURE0);
        let texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        let sampler = gl.get_parameter_sampler(glow::SAMPLER_BINDING);
        gl.active_texture(active_texture);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let mut clear_color = [0.0; 4];
        gl.get_parameter_f32_slice(glow::COLOR_CLEAR_VALUE, &mut clear_color);
        let mut depth_range = [0.0; 2];
        gl.get_parameter_f32_slice(glow::DEPTH_RANGE, &mut depth_range);
        let mut polygon_modes = [0; 2];
        gl.get_parameter_i32_slice(glow::POLYGON_MODE, &mut polygon_modes);
        State {
            program: gl.get_parameter_program(glow::CURRENT_PROGRAM),
            vao: gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING),
            buffer: gl.get_parameter_buffer(glow::ARRAY_BUFFER_BINDING),
            draw: gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),
            read: gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING),
            renderbuffer: gl.get_parameter_renderbuffer(glow::RENDERBUFFER_BINDING),
            pack_buffer: gl.get_parameter_buffer(glow::PIXEL_PACK_BUFFER_BINDING),
            unpack_buffer: gl.get_parameter_buffer(glow::PIXEL_UNPACK_BUFFER_BINDING),
            active_texture,
            texture,
            sampler,
            viewport,
            pixel_store: [
                glow::PACK_ALIGNMENT,
                glow::PACK_SWAP_BYTES,
                glow::PACK_ROW_LENGTH,
                glow::PACK_SKIP_ROWS,
                glow::PACK_SKIP_PIXELS,
                glow::UNPACK_ALIGNMENT,
                glow::UNPACK_ROW_LENGTH,
                glow::UNPACK_SKIP_ROWS,
                glow::UNPACK_SKIP_PIXELS,
            ]
            .map(|p| (p, gl.get_parameter_i32(p)))
            .to_vec(),
            enables: [
                glow::BLEND,
                glow::CULL_FACE,
                glow::DEPTH_TEST,
                glow::SCISSOR_TEST,
                glow::STENCIL_TEST,
                glow::POLYGON_OFFSET_FILL,
                glow::RASTERIZER_DISCARD,
                glow::FRAMEBUFFER_SRGB,
                glow::MULTISAMPLE,
                glow::DITHER,
                glow::SAMPLE_ALPHA_TO_COVERAGE,
                glow::SAMPLE_COVERAGE,
                glow::SAMPLE_MASK,
                glow::DEPTH_CLAMP,
                glow::COLOR_LOGIC_OP,
            ]
            .map(|p| (p, gl.is_enabled(p)))
            .to_vec(),
            color_mask: gl.get_parameter_bool_array(glow::COLOR_WRITEMASK),
            depth_mask: gl.get_parameter_bool(glow::DEPTH_WRITEMASK),
            depth_func: gl.get_parameter_i32(glow::DEPTH_FUNC) as u32,
            clear_color,
            clear_depth: gl.get_parameter_f32(glow::DEPTH_CLEAR_VALUE),
            depth_range,
            polygon_mode: polygon_modes[0] as u32,
        }
    }
    unsafe fn restore(self, gl: &glow::Context) {
        gl.use_program(self.program);
        gl.bind_vertex_array(self.vao);
        gl.bind_buffer(glow::ARRAY_BUFFER, self.buffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.draw);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.read);
        gl.bind_renderbuffer(glow::RENDERBUFFER, self.renderbuffer);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, self.pack_buffer);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, self.unpack_buffer);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, self.texture);
        gl.bind_sampler(0, self.sampler);
        gl.active_texture(self.active_texture);
        gl.viewport(self.viewport[0], self.viewport[1], self.viewport[2], self.viewport[3]);
        for (p, value) in self.pixel_store {
            gl.pixel_store_i32(p, value);
        }
        for (p, enabled) in self.enables {
            if enabled {
                gl.enable(p);
            } else {
                gl.disable(p);
            }
        }
        gl.color_mask(self.color_mask[0], self.color_mask[1], self.color_mask[2], self.color_mask[3]);
        gl.depth_mask(self.depth_mask);
        gl.depth_func(self.depth_func);
        gl.clear_color(self.clear_color[0], self.clear_color[1], self.clear_color[2], self.clear_color[3]);
        gl.clear_depth(f64::from(self.clear_depth));
        gl.depth_range(f64::from(self.depth_range[0]), f64::from(self.depth_range[1]));
        gl.polygon_mode(glow::FRONT_AND_BACK, self.polygon_mode);
    }
}

pub struct Renderer {
    gl: Arc<glow::Context>,
    shaded: Option<glow::Program>,
    depth_program: Option<glow::Program>,
    scene: Geometry,
    dynamic: Geometry,
    shadow_geometry: Geometry,
    target: Target,
    shadow: Shadow,
    generation: Option<u64>,
    scene_bounds: Option<Bounds>,
    shadow_key: Option<u64>,
    samples: i32,
    marker_cache: HashMap<u64, Vec<f32>>,
    pixels: Vec<u8>,
    depth_pixels: Vec<u8>,
    #[cfg(test)]
    shadow_draws: usize,
}

impl Renderer {
    /// The GL context must be current for this call and every render/destroy call.
    pub fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        let version = gl.version();
        if version.is_embedded || (version.major, version.minor) < (3, 3) {
            return Err("The studio preview needs desktop OpenGL 3.3; using the CPU renderer".into());
        }
        unsafe {
            let saved = State::save(&gl);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_sampler(0, None);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            let mut renderer = Renderer {
                gl: gl.clone(),
                shaded: None,
                depth_program: None,
                scene: Geometry::default(),
                dynamic: Geometry::default(),
                shadow_geometry: Geometry::default(),
                target: Target::default(),
                shadow: Shadow::default(),
                generation: None,
                scene_bounds: None,
                shadow_key: None,
                samples: gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(1, 4),
                marker_cache: HashMap::new(),
                pixels: Vec::new(),
                depth_pixels: Vec::new(),
                #[cfg(test)]
                shadow_draws: 0,
            };
            let result = (|| {
                renderer.shaded = Some(program(&gl, VERTEX, FRAGMENT)?);
                renderer.depth_program = Some(program(&gl, DEPTH_VERTEX, DEPTH_FRAGMENT)?);
                renderer.shadow.create(&gl)?;
                Ok(())
            })();
            if result.is_err() {
                renderer.destroy();
            }
            saved.restore(&gl);
            result.map(|()| renderer)
        }
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        generation: u64,
        static_mesh: &Mesh,
        view: &Camera,
        markers: &[Mesh],
        lines: &[Line],
        look: &Look,
    ) -> Result<(), String> {
        self.render_common(frame, generation, static_mesh, view, markers, markers, lines, look)
    }

    /// Every monitor can omit its own camera from the picture while sharing the
    /// same complete set of shadow casters, so twelve views need one shadow pass.
    pub fn render_with_shadow_casters(
        &mut self,
        frame: &mut Frame,
        generation: u64,
        static_mesh: &Mesh,
        view: &Camera,
        markers: &[Mesh],
        shadow_casters: &[Mesh],
        lines: &[Line],
        look: &Look,
    ) -> Result<(), String> {
        if std::ptr::eq(markers, shadow_casters) {
            self.render(frame, generation, static_mesh, view, markers, lines, look)
        } else {
            self.render_common(frame, generation, static_mesh, view, markers, shadow_casters, lines, look)
        }
    }

    fn render_common(
        &mut self,
        frame: &mut Frame,
        generation: u64,
        static_mesh: &Mesh,
        view: &Camera,
        markers: &[Mesh],
        shadow_casters: &[Mesh],
        lines: &[Line],
        look: &Look,
    ) -> Result<(), String> {
        if self.shaded.is_none() {
            return Err("The preview renderer has been closed".into());
        }
        if frame.width == 0 || frame.height == 0 {
            return Err("The preview has no size".into());
        }
        let gl = self.gl.clone();
        let result = unsafe {
            let saved = State::save(&gl);
            let result = self.render_inner(frame, generation, static_mesh, view, markers, shadow_casters, look);
            saved.restore(&gl);
            result
        };
        if result.is_ok() {
            for (a, b, color, width) in lines {
                render::draw_line(frame, view, *a, *b, *color, *width);
            }
        }
        result
    }

    unsafe fn render_inner(
        &mut self,
        frame: &mut Frame,
        generation: u64,
        static_mesh: &Mesh,
        view: &Camera,
        markers: &[Mesh],
        shadow_casters: &[Mesh],
        look: &Look,
    ) -> Result<(), String> {
        let gl = &self.gl;
        let max = gl.get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE) as usize;
        if frame.width > max || frame.height > max {
            return Err("The preview exceeds this GPU's framebuffer size".into());
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_sampler(0, None);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        for parameter in [
            glow::PACK_SWAP_BYTES,
            glow::PACK_ROW_LENGTH,
            glow::PACK_SKIP_ROWS,
            glow::PACK_SKIP_PIXELS,
            glow::UNPACK_ROW_LENGTH,
            glow::UNPACK_SKIP_ROWS,
            glow::UNPACK_SKIP_PIXELS,
        ] {
            gl.pixel_store_i32(parameter, 0);
        }
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        for capability in [
            glow::BLEND,
            glow::CULL_FACE,
            glow::SCISSOR_TEST,
            glow::STENCIL_TEST,
            glow::POLYGON_OFFSET_FILL,
            glow::RASTERIZER_DISCARD,
            glow::FRAMEBUFFER_SRGB,
            glow::DITHER,
            glow::SAMPLE_ALPHA_TO_COVERAGE,
            glow::SAMPLE_COVERAGE,
            glow::SAMPLE_MASK,
            glow::DEPTH_CLAMP,
            glow::COLOR_LOGIC_OP,
        ] {
            gl.disable(capability);
        }
        gl.enable(glow::DEPTH_TEST);
        gl.enable(glow::MULTISAMPLE);
        gl.color_mask(true, true, true, true);
        gl.depth_mask(true);
        gl.depth_func(glow::LEQUAL);
        gl.depth_range(0.0, 1.0);
        gl.clear_depth(1.0);
        gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
        let size = [frame.width as i32, frame.height as i32];
        self.target.resize(gl, size, self.samples)?;
        if self.generation != Some(generation) {
            self.scene.upload(gl, &vertices(static_mesh), glow::STATIC_DRAW)?;
            self.scene_bounds = static_mesh.bounds();
            self.generation = Some(generation);
            self.shadow_key = None;
        }
        let mut moving = Vec::new();
        if self.marker_cache.len() > 64 {
            self.marker_cache.clear();
        }
        for marker in markers {
            let key = mesh_key(marker);
            moving.extend_from_slice(self.marker_cache.entry(key).or_insert_with(|| vertices(marker)));
        }
        self.dynamic.upload(gl, &moving, glow::STREAM_DRAW)?;
        let bounds = combined_bounds(self.scene_bounds, shadow_casters);
        let light = shadow_projection(bounds);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        generation.hash(&mut hash);
        let shadow_keys: Vec<u64> = shadow_casters.iter().map(mesh_key).collect();
        shadow_keys.hash(&mut hash);
        let shadow_key = hash.finish();
        if self.shadow_key != Some(shadow_key) {
            let mut casters = Vec::new();
            for (mesh, key) in shadow_casters.iter().zip(shadow_keys) {
                casters.extend_from_slice(self.marker_cache.entry(key).or_insert_with(|| vertices(mesh)));
            }
            self.shadow_geometry.upload(gl, &casters, glow::STREAM_DRAW)?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.shadow.framebuffer);
            gl.viewport(0, 0, self.shadow.side, self.shadow.side);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            let program = self.depth_program.unwrap();
            gl.use_program(Some(program));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(program, "light_matrix").as_ref(), false, &light);
            self.scene.draw(gl);
            self.shadow_geometry.draw(gl);
            self.shadow_key = Some(shadow_key);
            #[cfg(test)]
            {
                self.shadow_draws += 1;
            }
        }
        let far = corners(bounds).iter().map(|p| dot(sub(*p, view.eye), view.forward)).fold(view.near + 10.0, f32::max) + 10.0;
        let project = projection(view, frame.width as f32 / frame.height as f32, far);
        gl.bind_framebuffer(glow::FRAMEBUFFER, self.target.multisample.or(self.target.resolved));
        gl.viewport(0, 0, size[0], size[1]);
        gl.clear_color(15.0 / 255.0, 15.0 / 255.0, 15.0 / 255.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        let program = self.shaded.unwrap();
        gl.use_program(Some(program));
        gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(program, "camera_matrix").as_ref(), false, &project);
        gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(program, "light_matrix").as_ref(), false, &light);
        gl.uniform_3_f32_slice(gl.get_uniform_location(program, "eye").as_ref(), &view.eye);
        gl.uniform_3_f32_slice(gl.get_uniform_location(program, "fog_color").as_ref(), &look.fog_color.map(|c| c as f32 / 255.0));
        gl.uniform_3_f32(gl.get_uniform_location(program, "fog_range").as_ref(), look.fog_start, look.fog_end, look.fog_strength);
        gl.uniform_1_i32(gl.get_uniform_location(program, "shadow_map").as_ref(), 0);
        gl.bind_texture(glow::TEXTURE_2D, self.shadow.depth);
        self.scene.draw(gl);
        self.dynamic.draw(gl);
        if self.target.multisample.is_some() {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.target.multisample);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.target.resolved);
            gl.blit_framebuffer(
                0,
                0,
                size[0],
                size[1],
                0,
                0,
                size[0],
                size[1],
                glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.target.resolved);
        gl.read_buffer(glow::COLOR_ATTACHMENT0);
        self.pixels.resize(frame.width * frame.height * 4, 0);
        self.depth_pixels.resize(frame.width * frame.height * 4, 0);
        gl.read_pixels(
            0,
            0,
            size[0],
            size[1],
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut self.pixels)),
        );
        gl.read_pixels(
            0,
            0,
            size[0],
            size[1],
            glow::DEPTH_COMPONENT,
            glow::FLOAT,
            glow::PixelPackData::Slice(Some(&mut self.depth_pixels)),
        );
        let error = gl.get_error();
        if error != glow::NO_ERROR {
            return Err(format!("The GPU preview failed ({error:#x}); using the CPU renderer"));
        }
        let near = view.near.max(0.0001);
        for row in 0..frame.height {
            let source = (frame.height - row - 1) * frame.width;
            let target = row * frame.width;
            frame.rgba[target * 4..(target + frame.width) * 4]
                .copy_from_slice(&self.pixels[source * 4..(source + frame.width) * 4]);
            for column in 0..frame.width {
                let at = (source + column) * 4;
                let depth = f32::from_ne_bytes(self.depth_pixels[at..at + 4].try_into().unwrap());
                frame.depth[target + column] = if depth >= 1.0 {
                    f32::INFINITY
                } else {
                    2.0 * near * far / (far + near - (2.0 * depth - 1.0) * (far - near))
                };
            }
        }
        Ok(())
    }

    /// Call while the context is current, before eframe destroys it. No GL calls
    /// are made by Drop because the desktop may already have removed the context.
    pub fn destroy(&mut self) {
        unsafe {
            let gl = &self.gl;
            self.scene.destroy(gl);
            self.dynamic.destroy(gl);
            self.shadow_geometry.destroy(gl);
            self.target.destroy(gl);
            self.shadow.destroy(gl);
            if let Some(program) = self.shaded.take() {
                gl.delete_program(program);
            }
            if let Some(program) = self.depth_program.take() {
                gl.delete_program(program);
            }
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub fn shadow_draws(&self) -> usize {
        self.shadow_draws
    }
}

const VERTEX: &str = r#"#version 330 core
layout(location=0) in vec3 position;
layout(location=1) in vec3 normal;
layout(location=2) in vec3 color;
uniform mat4 camera_matrix;
out vec3 world;
out vec3 surface_normal;
out vec3 base_color;
void main() { world=position; surface_normal=normal; base_color=color; gl_Position=camera_matrix*vec4(position,1.0); }
"#;
const DEPTH_VERTEX: &str = r#"#version 330 core
layout(location=0) in vec3 position;
uniform mat4 light_matrix;
void main() { gl_Position=light_matrix*vec4(position,1.0); }
"#;
const DEPTH_FRAGMENT: &str = "#version 330 core\nvoid main() {}\n";
const FRAGMENT: &str = r#"#version 330 core
in vec3 world;
in vec3 surface_normal;
in vec3 base_color;
uniform vec3 eye;
uniform mat4 light_matrix;
uniform sampler2D shadow_map;
uniform vec3 fog_color;
uniform vec3 fog_range;
out vec4 out_color;
const float PI=3.14159265;
const vec3 KEY=vec3(-0.26726124,-0.5345225,0.8017837);
bool palette(vec3 value) { return all(lessThan(abs(base_color*255.0-value),vec3(0.5))); }
float visibility(vec3 n) {
    vec3 p=(light_matrix*vec4(world+n*0.018,1.0)).xyz*0.5+0.5;
    if(any(lessThan(p,vec3(0)))||any(greaterThan(p,vec3(1)))) return 1.0;
    float bias=max(0.00012,0.00065*(1.0-max(dot(n,KEY),0.0)));
    vec2 step=1.0/vec2(textureSize(shadow_map,0));
    float sum=0.0;
    for(int x=-1;x<=1;x++) for(int y=-1;y<=1;y++) sum+=p.z-bias<=texture(shadow_map,p.xy+vec2(x,y)*step).r?1.0:0.0;
    return sum/9.0;
}
vec3 brdf(vec3 n,vec3 v,vec3 l,vec3 base,float rough,float metal) {
    vec3 h=normalize(v+l);
    float nl=max(dot(n,l),0.0),nv=max(dot(n,v),0.001),nh=max(dot(n,h),0.0),vh=max(dot(v,h),0.0);
    float a=rough*rough,a2=a*a,den=nh*nh*(a2-1.0)+1.0;
    float d=a2/(PI*den*den+0.00001),k=(rough+1.0)*(rough+1.0)/8.0;
    float g=nv/(nv*(1.0-k)+k)*nl/(nl*(1.0-k)+k);
    vec3 f0=mix(vec3(0.04),base,metal),f=f0+(1.0-f0)*pow(1.0-vh,5.0);
    return ((1.0-f)*(1.0-metal)*base/PI+d*g*f/max(4.0*nv*nl,0.0001))*nl;
}
vec3 aces(vec3 x) { return clamp((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14),0.0,1.0); }
void main() {
    vec3 n=normalize(surface_normal),v=normalize(eye-world);
    if(dot(n,v)<0.0) n=-n;
    vec3 base=pow(base_color,vec3(2.2));
    float rough=0.74,metal=0.05;
    if(palette(vec3(65,72,83))) { rough=0.4;metal=0.2; }
    if(palette(vec3(121,128,139))||palette(vec3(130,141,154))) { rough=0.28;metal=0.7; }
    if(palette(vec3(32,71,96))) { rough=0.12;metal=0.65; }
    vec3 ambient=mix(vec3(0.16,0.14,0.12),vec3(0.68,0.78,0.90),n.z*0.5+0.5);
    vec3 light=base*ambient*0.65;
    light+=brdf(n,v,KEY,base,rough,metal)*vec3(1.0,0.89,0.72)*3.0*visibility(n);
    light+=brdf(n,v,normalize(vec3(8,-4,5)),base,rough,metal)*vec3(0.66,0.82,0.95)*0.65;
    if(palette(vec3(157,170,185))) light+=vec3(0.034,0.057,0.078);
    vec3 shown=pow(aces(light*1.05),vec3(1.0/2.2));
    if(fog_range.y>fog_range.x&&fog_range.z>0.0) {
        float fog=clamp((length(eye-world)-fog_range.x)/(fog_range.y-fog_range.x),0.0,1.0)*clamp(fog_range.z,0.0,1.0);
        shown=mix(shown,fog_color,fog);
    }
    out_color=vec4(shown,1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    fn clip(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
        [0, 1, 2, 3].map(|row| m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row])
    }
    #[test]
    fn projection_matches_the_cpu_including_both_vertical_poles() {
        for pitch in [-90.0, -45.0, 0.0, 45.0, 90.0] {
            for yaw in [-170.0, 0.0, 73.0, 180.0] {
                let view = Camera::look([2.0, -3.0, 4.0], yaw, pitch, 60.0);
                let m = projection(&view, 16.0 / 9.0, 100.0);
                for (x, y, z) in [(0.0, 0.0, 10.0), (1.0, 2.0, 8.0), (-2.0, -1.0, 5.0)] {
                    let p = [0, 1, 2].map(|k| view.eye[k] + view.right[k] * x + view.up[k] * y + view.forward[k] * z);
                    let actual = clip(&m, p);
                    let cpu = view.project(p, 960, 540).unwrap();
                    assert!((480.0 * (actual[0] / actual[3] + 1.0) - cpu[0]).abs() < 0.001);
                    assert!((270.0 * (1.0 - actual[1] / actual[3]) - cpu[1]).abs() < 0.001);
                    assert!((actual[3] - z).abs() < 0.00001);
                }
            }
        }
    }
    #[test]
    fn smoothing_keeps_box_edges_hard_and_rounds_a_shallow_seam() {
        let mut cube = Mesh::default();
        cube.push_cuboid([-1.0; 3], [1.0; 3], [120; 3]);
        for vertex in vertices(&cube).as_chunks::<FLOATS_PER_VERTEX>().0 {
            let n = &vertex[3..6];
            assert_eq!(n.iter().filter(|v| v.abs() > 0.99).count(), 1);
            assert_eq!(n.iter().filter(|v| v.abs() < 0.01).count(), 2);
        }
        let mut bent = Mesh::default();
        bent.push_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [120; 3]);
        bent.push_triangle([0.0, 0.0, 0.0], [0.0, -1.0, -0.2], [1.0, 0.0, 0.0], [120; 3]);
        let data = vertices(&bent);
        let shared: Vec<_> = data.as_chunks::<FLOATS_PER_VERTEX>().0.iter().filter(|v| v[..3] == [0.0; 3]).copied().collect();
        assert_eq!(&shared[0][3..6], &shared[1][3..6]);
        assert!(shared[0][4].abs() > 0.02 && shared[0][5] > 0.98);
    }
    #[test]
    fn shadow_projection_fits_every_corner_of_small_and_large_venues() {
        for scale in [0.1, 1.0, 10000.0] {
            let bounds = Bounds { min: [-4.0 * scale, -3.0 * scale, 0.0], max: [5.0 * scale, 8.0 * scale, 3.0 * scale] };
            let m = shadow_projection(bounds);
            for p in corners(bounds) {
                assert!(clip(&m, p)[..3].iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            }
        }
    }
}
