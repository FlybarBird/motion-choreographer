//! Writes a sample event room as `venue.obj` + `venue.mtl` (Y up, as OBJ files usually are) and
//! `venue.3ds` (Z up), both in metres:
//!
//! ```text
//! cargo run -p ptz-scene --example make_venue -- <output dir>
//! ```
//!
//! The room is laid out Z up with the audience camera near the origin looking toward +Y at the stage.

use ptz_scene::mesh::{Mesh, Transform};
use ptz_scene::{obj, tds};
use std::path::Path;

const CARPET: [u8; 3] = [52, 56, 68];
const WALL: [u8; 3] = [206, 200, 188];
const STAGE_WOOD: [u8; 3] = [150, 112, 74];
const LECTERN_WOOD: [u8; 3] = [92, 62, 40];
const TABLE_CLOTH: [u8; 3] = [44, 58, 96];
const SCREEN: [u8; 3] = [236, 236, 230];
const TRUSS: [u8; 3] = [124, 126, 130];
const CHAIR: [u8; 3] = [112, 36, 48];
const RISER: [u8; 3] = [70, 70, 76];
const BALCONY: [u8; 3] = [158, 156, 150];

fn venue() -> Mesh {
    let mut mesh = Mesh::default();
    let wall_height = 7.0;
    let wall_thickness = 0.2;

    mesh.push_cuboid([-12.0, -8.0, -0.1], [12.0, 22.0, 0.0], CARPET);
    // Back wall and two side walls; no ceiling and no front wall, so a view from outside sees in.
    mesh.push_cuboid([-12.0 - wall_thickness, 22.0, 0.0], [12.0 + wall_thickness, 22.0 + wall_thickness, wall_height], WALL);
    mesh.push_cuboid([-12.0 - wall_thickness, -8.0, 0.0], [-12.0, 22.0, wall_height], WALL);
    mesh.push_cuboid([12.0, -8.0, 0.0], [12.0 + wall_thickness, 22.0, wall_height], WALL);

    let stage_top = 0.9;
    mesh.push_cuboid([-7.0, 13.0, 0.0], [7.0, 20.0, stage_top], STAGE_WOOD);
    mesh.push_cuboid([4.2, 14.25, stage_top], [4.8, 14.75, stage_top + 1.15], LECTERN_WOOD);
    mesh.push_cuboid([-4.9, 14.65, stage_top], [-3.1, 15.35, stage_top + 0.75], TABLE_CLOTH);
    mesh.push_cuboid([-3.0, 21.9, 2.6], [3.0, 22.0, 6.0], SCREEN);
    mesh.push_cuboid([-8.0, 11.85, 5.85], [8.0, 12.15, 6.15], TRUSS);

    // Two blocks of chairs facing the stage, either side of a 2 m centre aisle.
    let (rows, seats) = (8, 10);
    let (row_pitch, seat_pitch) = ((11.0 - 2.0) / rows as f32, 0.6);
    for row in 0..rows {
        let rear = 2.0 + row as f32 * row_pitch + 0.5 * (row_pitch - 0.5);
        for seat in 0..seats {
            for side in [-1.0, 1.0] {
                let inner = side * (1.0 + seat as f32 * seat_pitch + 0.05);
                let (left, right) = (inner.min(inner + side * 0.5), inner.max(inner + side * 0.5));
                mesh.push_cuboid([left, rear, 0.0], [right, rear + 0.5, 0.45], CHAIR);
                mesh.push_cuboid([left, rear, 0.45], [right, rear + 0.06, 0.9], CHAIR);
            }
        }
    }

    mesh.push_cuboid([7.0, -5.0, 0.0], [9.0, -3.0, 0.3], RISER);
    mesh.push_cuboid([-12.0, -8.0, 2.9], [12.0, -5.0, 3.2], BALCONY);
    mesh
}

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("Say where to write the files: make_venue <output dir>");
        std::process::exit(2);
    };
    if let Err(message) = write_files(Path::new(&dir)) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn write_files(dir: &Path) -> Result<(), String> {
    let mesh = venue();
    let (obj_text, mtl_text) = obj::write(&mesh.transformed(&Transform::z_up_to_y_up()), "venue.mtl");
    let files =
        [("venue.obj", obj_text.into_bytes()), ("venue.mtl", mtl_text.into_bytes()), ("venue.3ds", tds::write(&mesh, "Venue"))];
    std::fs::create_dir_all(dir).map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::write(&path, bytes).map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        println!("Wrote {}", path.display());
    }
    println!("{} triangles", mesh.triangles.len());
    Ok(())
}
