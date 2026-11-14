//! Prints how long drawing and importing take on this machine, and fails only if they are far too slow.

use ptz_scene::mesh::Mesh;
use ptz_scene::render::{draw_scene, Camera, Frame, Look, Scene};
use ptz_scene::{obj, tds};
use std::time::Instant;

/// A 24 x 30 m room, three walls, and a floor covered by a grid of small boxes: about `triangles` triangles.
fn room(triangles: usize) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.push_cuboid([-12.0, -8.0, -0.1], [12.0, 22.0, 0.0], [52, 56, 68]);
    mesh.push_cuboid([-12.2, 22.0, 0.0], [12.2, 22.2, 7.0], [206, 200, 188]);
    mesh.push_cuboid([-12.2, -8.0, 0.0], [-12.0, 22.0, 7.0], [206, 200, 188]);
    mesh.push_cuboid([12.0, -8.0, 0.0], [12.2, 22.0, 7.0], [206, 200, 188]);
    let boxes = (triangles.saturating_sub(mesh.triangles.len()) / 12).max(1);
    // Columns and rows in the room's own proportions, so the boxes come out square.
    let columns = ((boxes as f32 * 24.0 / 30.0).sqrt().round() as usize).max(1);
    let rows = boxes.div_ceil(columns);
    let (pitch_x, pitch_y) = (24.0 / columns as f32, 30.0 / rows as f32);
    for at in 0..boxes {
        let (column, row) = (at % columns, at / columns);
        let (x, y) = (-12.0 + column as f32 * pitch_x, -8.0 + row as f32 * pitch_y);
        let height = 0.2 + 0.1 * ((column * 7 + row * 13) % 9) as f32;
        let color = [90 + (column * 37 % 120) as u8, 60 + (row * 53 % 120) as u8, 80 + (at * 11 % 90) as u8];
        mesh.push_cuboid([x + 0.2 * pitch_x, y + 0.2 * pitch_y, 0.0], [x + 0.8 * pitch_x, y + 0.8 * pitch_y, height], color);
    }
    mesh
}

fn median_ms(mut run: impl FnMut(), times: usize) -> f64 {
    let mut taken: Vec<f64> = (0..times)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    taken.sort_by(f64::total_cmp);
    taken[times / 2]
}

#[test]
fn render_speed_report() {
    let camera = Camera::look([0.0, -6.5, 1.7], 0.0, -4.0, 60.0);
    let look = Look { fog_color: [150, 170, 190], fog_start: 10.0, fog_end: 60.0, fog_strength: 0.4 };
    let mut frame = Frame::new(960, 540);
    for target in [100_000, 500_000, 1_500_000] {
        let scene = Scene::new(&room(target));
        let draw = |frame: &mut Frame| {
            frame.clear([150, 170, 190]);
            draw_scene(frame, &camera, &scene, &look);
        };
        draw(&mut frame);
        let ms = median_ms(|| draw(&mut frame), 9);
        eprintln!("render_speed 960x540 triangles={} ms={ms:.1}", scene.triangle_count());
        assert!(frame.depth_at(480, 400).is_some(), "the room was not drawn");
        assert!(ms < 400.0, "drawing {} triangles took {ms:.0} ms", scene.triangle_count());
    }

    let mesh = room(500_000);
    let (obj_text, mtl_text) = obj::write(&mesh, "room.mtl");
    let sibling = |name: &str| (name == "room.mtl").then(|| mtl_text.clone().into_bytes());
    let mut read = None;
    let ms = median_ms(|| read = Some(obj::parse(obj_text.as_bytes(), &sibling).unwrap()), 3);
    eprintln!("import_speed obj triangles={} ms={ms:.1}", read.as_ref().map_or(0, |mesh| mesh.triangles.len()));
    assert_eq!(read.as_ref(), Some(&mesh));
    assert!(ms < 5000.0, "reading the OBJ took {ms:.0} ms");

    let tds_bytes = tds::write(&mesh, "Room");
    let ms = median_ms(|| read = Some(tds::parse(&tds_bytes).unwrap()), 3);
    eprintln!("import_speed 3ds triangles={} ms={ms:.1}", read.as_ref().map_or(0, |mesh| mesh.triangles.len()));
    assert_eq!(read.map(|mesh| mesh.triangles.len()), Some(mesh.triangles.len()));
    assert!(ms < 5000.0, "reading the 3DS took {ms:.0} ms");
}
