//! Optional native renderer checks on a real, headless Linux OpenGL context.
//!
//! Run with `cargo test -p ptz-app gpu_tests -- --ignored --nocapture`.
//! Mesa's surfaceless EGL backend works without X11, Wayland, or a display.

use crate::{stage, stage_gpu};
use eframe::glow::{self, HasContext as _};
use ptz_core::camera::{CameraConfig, Mount};
use ptz_core::pose::Pose;
use ptz_scene::mesh::Mesh;
use ptz_scene::render::{Camera, Frame, Look};
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;
use std::time::Instant;

type Handle = *mut c_void;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: i32) -> Handle;
    fn dlsym(library: Handle, name: *const c_char) -> Handle;
    fn dlclose(library: Handle) -> i32;
    fn dlerror() -> *const c_char;
}

/// Load EGL at test runtime, so ordinary headless tests do not gain a system dependency.
struct Egl {
    library: Handle,
    display: Handle,
    context: Handle,
    surface: Handle,
    get_proc: unsafe extern "C" fn(*const c_char) -> Handle,
    make_current: unsafe extern "C" fn(Handle, Handle, Handle, Handle) -> u32,
    destroy_surface: unsafe extern "C" fn(Handle, Handle) -> u32,
    destroy_context: unsafe extern "C" fn(Handle, Handle) -> u32,
    terminate: unsafe extern "C" fn(Handle) -> u32,
}

unsafe fn symbol<T: Copy>(library: Handle, name: &CStr) -> T {
    let address = dlsym(library, name.as_ptr());
    assert!(!address.is_null(), "EGL entry point {} is unavailable", name.to_string_lossy());
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<Handle>());
    std::mem::transmute_copy(&address)
}

impl Egl {
    fn new() -> Self {
        unsafe {
            let library = dlopen(c"libEGL.so.1".as_ptr(), 2);
            if library.is_null() {
                let error = dlerror();
                let why = if error.is_null() { "unknown error".into() } else { CStr::from_ptr(error).to_string_lossy() };
                panic!("The explicit GPU test needs libEGL.so.1: {why}");
            }
            let mut egl = Egl {
                library,
                display: ptr::null_mut(),
                context: ptr::null_mut(),
                surface: ptr::null_mut(),
                get_proc: symbol(library, c"eglGetProcAddress"),
                make_current: symbol(library, c"eglMakeCurrent"),
                destroy_surface: symbol(library, c"eglDestroySurface"),
                destroy_context: symbol(library, c"eglDestroyContext"),
                terminate: symbol(library, c"eglTerminate"),
            };
            let display: unsafe extern "C" fn(u32, Handle, *const i32) -> Handle = symbol(library, c"eglGetPlatformDisplay");
            let initialize: unsafe extern "C" fn(Handle, *mut i32, *mut i32) -> u32 = symbol(library, c"eglInitialize");
            let bind_api: unsafe extern "C" fn(u32) -> u32 = symbol(library, c"eglBindAPI");
            let choose: unsafe extern "C" fn(Handle, *const i32, *mut Handle, i32, *mut i32) -> u32 =
                symbol(library, c"eglChooseConfig");
            let context: unsafe extern "C" fn(Handle, Handle, Handle, *const i32) -> Handle =
                symbol(library, c"eglCreateContext");
            let surface: unsafe extern "C" fn(Handle, Handle, *const i32) -> Handle = symbol(library, c"eglCreatePbufferSurface");
            let get_error: unsafe extern "C" fn() -> u32 = symbol(library, c"eglGetError");
            let check = |ok: bool, operation: &str| assert!(ok, "EGL {operation} failed: 0x{:04x}", get_error());
            // EGL_PLATFORM_SURFACELESS_MESA, then desktop OpenGL rather than GLES.
            egl.display = display(0x31dd, ptr::null_mut(), ptr::null());
            check(!egl.display.is_null(), "display");
            let (mut major, mut minor) = (0, 0);
            check(initialize(egl.display, &mut major, &mut minor) != 0, "initialize");
            check(bind_api(0x30a2) != 0, "bind OpenGL API");
            let attrs = [0x3033, 1, 0x3040, 8, 0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3025, 24, 0x3038];
            let (mut config, mut count) = (ptr::null_mut(), 0);
            check(choose(egl.display, attrs.as_ptr(), &mut config, 1, &mut count) != 0 && count > 0, "choose config");
            let attrs = [0x3098, 3, 0x30fb, 3, 0x30fd, 1, 0x3038];
            egl.context = context(egl.display, config, ptr::null_mut(), attrs.as_ptr());
            check(!egl.context.is_null(), "create OpenGL 3.3 core context");
            let attrs = [0x3057, 16, 0x3056, 16, 0x3038];
            egl.surface = surface(egl.display, config, attrs.as_ptr());
            check(!egl.surface.is_null(), "create pbuffer");
            check((egl.make_current)(egl.display, egl.surface, egl.surface, egl.context) != 0, "make current");
            egl
        }
    }

    fn gl(&self) -> Arc<glow::Context> {
        Arc::new(unsafe {
            glow::Context::from_loader_function(|name| {
                let name = CString::new(name).unwrap();
                (self.get_proc)(name.as_ptr()).cast_const()
            })
        })
    }
}

impl Drop for Egl {
    fn drop(&mut self) {
        unsafe {
            if !self.display.is_null() {
                (self.make_current)(self.display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                if !self.surface.is_null() {
                    (self.destroy_surface)(self.display, self.surface);
                }
                if !self.context.is_null() {
                    (self.destroy_context)(self.display, self.context);
                }
                (self.terminate)(self.display);
            }
            dlclose(self.library);
        }
    }
}

fn save(frame: &Frame, name: &str) {
    if let Some(folder) = std::env::var_os("PTZ_GPU_SHOTS") {
        let folder = std::path::PathBuf::from(folder);
        std::fs::create_dir_all(&folder).unwrap();
        image::save_buffer(
            folder.join(format!("{name}.png")),
            &frame.rgba,
            frame.width as u32,
            frame.height as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

fn usable(frame: &Frame, expect_geometry: bool) {
    assert_eq!(frame.rgba.len(), frame.width * frame.height * 4);
    assert_eq!(frame.depth.len(), frame.width * frame.height);
    assert!(frame.depth.iter().all(|d| *d >= 0.0 && !d.is_nan()), "depth must be positive or infinite");
    assert!(frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255), "the view must stay opaque");
    if expect_geometry {
        assert!(frame.depth.iter().any(|d| d.is_finite()), "the GPU drew no geometry");
        assert!(frame.rgba.chunks_exact(4).any(|p| p[0].max(p[1]).max(p[2]) > 32), "the GPU drew no visible surfaces");
    }
}

fn target(view: &Camera, across: f32, up: f32) -> [f32; 3] {
    [0, 1, 2].map(|k| view.eye[k] + 4.0 * view.forward[k] + across * view.right[k] + up * view.up[k])
}

fn pole_targets(view: &Camera) -> Mesh {
    let mut mesh = Mesh::default();
    for (x, y, color) in [(-0.7, 0.5, [220, 35, 20]), (0.7, 0.5, [30, 220, 50]), (-0.7, -0.5, [35, 60, 230])] {
        mesh.push_quad(
            [
                target(view, x - 0.25, y - 0.25),
                target(view, x + 0.25, y - 0.25),
                target(view, x + 0.25, y + 0.25),
                target(view, x - 0.25, y + 0.25),
            ],
            color,
        );
    }
    mesh
}

#[test]
#[ignore = "requires explicit execution with a working surfaceless EGL/OpenGL driver"]
fn native_gpu_draws_views_depth_vertical_mounts_and_restores_gl_state() {
    let egl = Egl::new();
    let gl = egl.gl();
    unsafe {
        eprintln!("Native GPU check: {} / {}", gl.get_parameter_string(glow::VERSION), gl.get_parameter_string(glow::RENDERER));
    }
    let mut renderer = stage_gpu::Renderer::new(gl.clone()).expect("native renderer should initialize on OpenGL 3.3+");
    let look = Look::default();
    let mesh = stage::builtin();
    let mut frame = Frame::new(960, 540);
    let view = Camera::look([0.0, 0.0, 1.2], 0.0, 0.0, 60.0);
    renderer.render(&mut frame, 1, &mesh, &view, &[], &[], &look).unwrap();
    usable(&frame, true);
    save(&frame, "native-stage-wide");
    let lectern = Camera::look_at([0.0, 0.0, 1.2], [5.3, 12.45, 1.8], 24.0);
    renderer.render(&mut frame, 1, &mesh, &lectern, &[], &[], &look).unwrap();
    usable(&frame, true);
    save(&frame, "native-stage-lectern");

    let mut rig_floor = Mesh::default();
    rig_floor.push_cuboid([-4.0, -4.0, -0.05], [4.0, 4.0, 0.0], [52, 56, 62]);
    let mounts = [
        Mount { x: -0.55, ..Mount::default() },
        Mount { x: 0.7, y: 0.25, z: 3.3, inverted: true, heading_deg: -20.0, ..Mount::default() },
    ];
    let markers = mounts.map(|mount| stage::marker(&mount, &Pose { tilt: 24.0, ..Pose::home() }, [114, 184, 158]));
    let rig_view = Camera::look_at([3.0, 3.0, 2.7], [0.0, 0.0, 1.65], 50.0);
    renderer.render(&mut frame, 2, &rig_floor, &rig_view, &markers, &[], &look).unwrap();
    usable(&frame, true);
    save(&frame, "native-camera-models");

    // Exact vertical views retain the camera's own heading, including inverted mounts.
    // An asymmetric arrangement catches a mirrored image, lost roll, or a singular look-at matrix.
    for (at, (pitch, inverted)) in [(90.0, false), (-90.0, false), (90.0, true), (-90.0, true)].into_iter().enumerate() {
        let camera = CameraConfig {
            mount: Mount {
                z: if inverted { 2.4 } else { 0.0 },
                inverted,
                heading_deg: 37.0,
                tilt_deg: pitch,
                ..Mount::default()
            },
            ..CameraConfig::default()
        };
        let view = stage::view_of(&camera, &Pose::home());
        let targets = pole_targets(&view);
        frame.resize(320, 180);
        renderer.render(&mut frame, 10 + at as u64, &targets, &view, &[], &[], &look).unwrap();
        usable(&frame, true);
        for (x, y, channel) in [(-0.7, 0.5, 0), (0.7, 0.5, 1), (-0.7, -0.5, 2)] {
            let projected = view.project(target(&view, x, y), frame.width, frame.height).unwrap();
            let (px, py) = (projected[0] as usize, projected[1] as usize);
            let index = py * frame.width + px;
            let pixel = &frame.rgba[index * 4..index * 4 + 3];
            assert!(
                pixel[channel] > pixel[(channel + 1) % 3] && pixel[channel] > pixel[(channel + 2) % 3],
                "pitch {pitch}, inverted {inverted}: target colour is misplaced: {pixel:?}"
            );
            assert!(
                (frame.depth[index] - 4.0).abs() < 0.015,
                "pitch {pitch}, inverted {inverted}: view depth {} does not match the CPU projection",
                frame.depth[index]
            );
        }
        save(&frame, &format!("native-pole-{at}"));
    }

    // A changed imported mesh is uploaded when its generation changes, and resize keeps valid depth.
    let mut imported = Mesh::default();
    imported.push_cuboid([-1.0, 4.0, -1.0], [1.0, 6.0, 1.0], [180, 50, 30]);
    let view = Camera::look([0.0; 3], 0.0, 0.0, 60.0);
    renderer.render(&mut frame, 30, &imported, &view, &[], &[], &look).unwrap();
    assert!((frame.depth_at(160, 90).unwrap() - 4.0).abs() < 0.015);
    let original = frame.rgba.clone();
    for p in &mut imported.positions {
        p[1] += 2.0;
    }
    imported.colors.fill([30, 50, 180]);
    renderer.render(&mut frame, 31, &imported, &view, &[], &[], &look).unwrap();
    assert!((frame.depth_at(160, 90).unwrap() - 6.0).abs() < 0.015);
    assert_ne!(frame.rgba, original);
    frame.resize(192, 108);
    renderer.render(&mut frame, 31, &imported, &view, &[], &[], &look).unwrap();
    usable(&frame, true);
    assert!((frame.depth_at(96, 54).unwrap() - 6.0).abs() < 0.015);

    // State that egui or another OpenGL callback may have set must survive the renderer.
    frame.resize(193, 109);
    unsafe {
        let framebuffer = gl.create_framebuffer().unwrap();
        let vao = gl.create_vertex_array().unwrap();
        let buffer = gl.create_buffer().unwrap();
        let texture = gl.create_texture().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.viewport(3, 5, 91, 77);
        gl.scissor(2, 7, 61, 57);
        gl.enable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.enable(glow::SAMPLE_MASK);
        gl.enable(glow::DEPTH_CLAMP);
        gl.enable(glow::COLOR_LOGIC_OP);
        gl.disable(glow::DEPTH_TEST);
        gl.depth_mask(false);
        gl.color_mask(true, false, true, false);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 8);
        gl.pixel_store_i32(glow::PACK_SWAP_BYTES, 1);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        let bindings = [
            glow::DRAW_FRAMEBUFFER_BINDING,
            glow::READ_FRAMEBUFFER_BINDING,
            glow::VERTEX_ARRAY_BINDING,
            glow::ARRAY_BUFFER_BINDING,
            glow::ACTIVE_TEXTURE,
            glow::TEXTURE_BINDING_2D,
            glow::PACK_ALIGNMENT,
            glow::PACK_SWAP_BYTES,
            glow::UNPACK_ALIGNMENT,
        ];
        let before = bindings.map(|p| gl.get_parameter_i32(p));
        renderer.render(&mut frame, 31, &imported, &view, &[], &[], &look).unwrap();
        assert_eq!(bindings.map(|p| gl.get_parameter_i32(p)), before, "OpenGL bindings or pixel alignment leaked");
        let mut rectangle = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut rectangle);
        assert_eq!(rectangle, [3, 5, 91, 77]);
        gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut rectangle);
        assert_eq!(rectangle, [2, 7, 61, 57]);
        assert!(gl.is_enabled(glow::SCISSOR_TEST) && gl.is_enabled(glow::BLEND));
        assert!(gl.is_enabled(glow::SAMPLE_MASK) && gl.is_enabled(glow::DEPTH_CLAMP) && gl.is_enabled(glow::COLOR_LOGIC_OP));
        assert!(!gl.is_enabled(glow::DEPTH_TEST));
        assert_eq!(gl.get_parameter_i32(glow::DEPTH_WRITEMASK), 0);
        gl.get_parameter_i32_slice(glow::COLOR_WRITEMASK, &mut rectangle);
        assert_eq!(rectangle, [1, 0, 1, 0]);
        assert_eq!(gl.get_error(), glow::NO_ERROR);
        assert!((frame.depth_at(96, 54).unwrap() - 6.0).abs() < 0.015, "host pixel packing must not corrupt depth readback");
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.delete_framebuffer(framebuffer);
        gl.delete_vertex_array(vao);
        gl.delete_buffer(buffer);
        gl.delete_texture(texture);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::SAMPLE_MASK);
        gl.disable(glow::DEPTH_CLAMP);
        gl.disable(glow::COLOR_LOGIC_OP);
        gl.pixel_store_i32(glow::PACK_SWAP_BYTES, 0);
        gl.depth_mask(true);
        gl.color_mask(true, true, true, true);
    }

    // A raised object must cast a localized shadow on unchanged receiver pixels.
    // Matching depths excludes the object's own silhouette from this comparison.
    let mut receiver = Mesh::default();
    receiver.push_quad([[-4.0, -4.0, 0.0], [4.0, -4.0, 0.0], [4.0, 4.0, 0.0], [-4.0, 4.0, 0.0]], [180, 174, 160]);
    let mut blocker = Mesh::default();
    blocker.push_cuboid([-0.25, -0.25, 0.0], [0.25, 0.25, 1.6], [75, 85, 100]);
    let shadow_view = Camera::look_at([3.0, -5.0, 4.0], [0.0, 0.0, 0.0], 60.0);
    frame.resize(320, 180);
    renderer.render(&mut frame, 39, &receiver, &shadow_view, &[], &[], &look).unwrap();
    let (unshadowed, receiver_depth) = (frame.rgba.clone(), frame.depth.clone());
    renderer.render(&mut frame, 39, &receiver, &shadow_view, std::slice::from_ref(&blocker), &[], &look).unwrap();
    let dimmed = (0..frame.depth.len())
        .filter(|&pixel| {
            let same_surface = receiver_depth[pixel].is_finite() && (receiver_depth[pixel] - frame.depth[pixel]).abs() < 0.001;
            let before: i32 = unshadowed[pixel * 4..pixel * 4 + 3].iter().map(|&v| i32::from(v)).sum();
            let after: i32 = frame.rgba[pixel * 4..pixel * 4 + 3].iter().map(|&v| i32::from(v)).sum();
            same_surface && before - after > 30
        })
        .count();
    let visible = receiver_depth.iter().filter(|d| d.is_finite()).count();
    assert!(dimmed > 8 && dimmed < visible / 3, "expected a localized cast shadow, got {dimmed} of {visible} receiver pixels");
    save(&frame, "native-shadow-receiver");

    // A hidden camera remains a shadow caster without appearing in its own picture.
    renderer.render_with_shadow_casters(&mut frame, 39, &receiver, &shadow_view, &[], &[blocker], &[], &look).unwrap();
    assert!(frame.depth.iter().zip(&receiver_depth).all(|(a, b)| a == b || (a - b).abs() < 0.001));
    let hidden_caster_shadow = (0..frame.depth.len())
        .filter(|&pixel| {
            let before: i32 = unshadowed[pixel * 4..pixel * 4 + 3].iter().map(|&v| i32::from(v)).sum();
            let after: i32 = frame.rgba[pixel * 4..pixel * 4 + 3].iter().map(|&v| i32::from(v)).sum();
            frame.depth[pixel].is_finite() && before - after > 30
        })
        .count();
    assert!(hidden_caster_shadow > 8, "a hidden camera must still cast its shadow");

    // Exercise a twelve-camera update, including cameras pointing exactly up/down.
    // Each tile excludes its own camera, as the actual multiview does, while all
    // cameras cast shadows. The common caster set should need only one shadow pass.
    // This is a measured local smoke test, not a performance promise for a hardware GPU.
    frame.resize(320, 180);
    let cameras: Vec<_> = (0..12)
        .map(|index| {
            let camera = CameraConfig {
                mount: Mount {
                    x: index as f64 - 5.5,
                    y: -2.0,
                    z: if index % 2 == 0 { 1.2 } else { 3.6 },
                    inverted: index % 2 != 0,
                    heading_deg: index as f64 * 4.0 - 22.0,
                    ..Mount::default()
                },
                ..CameraConfig::default()
            };
            let pose = Pose {
                tilt: match index % 3 {
                    0 => 90.0,
                    1 => -90.0,
                    _ => -8.0,
                },
                ..Pose::home()
            };
            (stage::view_of(&camera, &pose), stage::marker(&camera.mount, &pose, [114, 184, 158]))
        })
        .collect();
    let tile_markers: Vec<Vec<Mesh>> = (0..12)
        .map(|index| cameras.iter().enumerate().filter(|(other, _)| *other != index).map(|(_, (_, m))| m.clone()).collect())
        .collect();
    let mut shadow_casters: Vec<_> = cameras.iter().map(|(_, marker)| marker.clone()).collect();
    let shadow_draws = renderer.shadow_draws();
    let started = Instant::now();
    for (index, (view, _)) in cameras.iter().enumerate() {
        renderer
            .render_with_shadow_casters(&mut frame, 40, &mesh, view, &tile_markers[index], &shadow_casters, &[], &look)
            .unwrap();
        usable(&frame, index % 3 != 0);
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(renderer.shadow_draws(), shadow_draws + 1, "all twelve views should share one shadow pass");
    eprintln!(
        "Twelve 320×180 native views (including vertical, own marker excluded, shared shadow casters): {elapsed_ms:.1} ms total, {:.1} ms/view on this driver",
        elapsed_ms / 12.0
    );
    for position in &mut shadow_casters[0].positions {
        position[0] += 0.25;
    }
    renderer
        .render_with_shadow_casters(&mut frame, 40, &mesh, &cameras[2].0, &tile_markers[2], &shadow_casters, &[], &look)
        .unwrap();
    assert_eq!(renderer.shadow_draws(), shadow_draws + 2, "moving a caster must invalidate the shared shadow map");
    renderer
        .render_with_shadow_casters(&mut frame, 40, &mesh, &cameras[3].0, &tile_markers[3], &shadow_casters, &[], &look)
        .unwrap();
    assert_eq!(renderer.shadow_draws(), shadow_draws + 2, "switching visible camera meshes must retain the shared shadow map");
    renderer.destroy();

    // The actual app must select this backend and upload its pictures through
    // the same cache/egui texture path used by camera placement in the window.
    let (mut project, presets) = crate::ui_tests::three_cameras();
    project.cameras[1].mount.z = 3.3;
    project.cameras[1].mount.inverted = true;
    project.cameras[1].mount.tilt_deg = -4.0;
    let song = ptz_core::song::Song::new("gpu-placement", "Native camera placement", "01:00:00:00", 1500);
    let mut rig = crate::ui_tests::rig_sized(crate::ui_tests::DEFAULT_WINDOW, project, presets, song);
    {
        let shell = rig.harness.state_mut();
        shell.session.world.enable_gpu(gl.clone()).unwrap();
        shell.session.select_camera("cam2");
        shell.session.stage_view.placement = true;
        shell.show_tab(crate::app::Tab::Stage);
    }
    rig.harness.run_steps(4);
    let world = &mut rig.harness.state_mut().session.world;
    assert!(world.take_render_error().is_none(), "the real app fell back after enabling native GPU rendering");
    usable(world.frame("stage").expect("native stage view"), true);
    usable(world.frame("placement").expect("native placement view"), true);
    if let Some(folder) = std::env::var_os("PTZ_GPU_SHOTS") {
        let path = std::path::PathBuf::from(folder).join("native-placement.png");
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::ui_tests::DEFAULT_WINDOW);
        rig.picture().save(bounds, &path);
    }
    rig.harness.state_mut().session.world.disable_gpu();
    assert_eq!(unsafe { gl.get_error() }, glow::NO_ERROR);
}
