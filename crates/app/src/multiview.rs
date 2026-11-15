//! Multiview: every camera's picture at once, in a grid sized to the camera count.

use crate::monitor::{badge, picture_rect};
use crate::session::Session;
use crate::stage;
use crate::theme::*;
use egui::{vec2, Align2, Rect, Sense, Stroke, StrokeKind, Ui};

/// Columns and rows for a number of cameras: 1, 2x1, 2x2, 3x2, 3x3, 4x3.
pub fn grid(cameras: usize) -> (usize, usize) {
    match cameras {
        0 | 1 => (1, 1),
        2 => (2, 1),
        3 | 4 => (2, 2),
        5 | 6 => (3, 2),
        7..=9 => (3, 3),
        _ => (4, 3),
    }
}

/// The picture rectangle of each tile inside `area`, in camera order.
pub fn tiles(area: Rect, cameras: usize) -> Vec<Rect> {
    let (columns, rows) = grid(cameras);
    let gap = GAP;
    let cell = vec2(
        (area.width() - gap * (columns as f32 - 1.0)) / columns as f32,
        (area.height() - gap * (rows as f32 - 1.0)) / rows as f32,
    );
    (0..cameras)
        .map(|i| {
            let (column, row) = (i % columns, i / columns);
            let min = area.min + vec2(column as f32 * (cell.x + gap), row as f32 * (cell.y + gap));
            picture_rect(Rect::from_min_size(min, cell))
        })
        .collect()
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
    let area = resp.rect.shrink(GAP);
    painter.rect_filled(resp.rect, 0.0, BASE);
    s.probe_tiles.clear();
    let count = s.project.cameras.len();
    if area.width() < 60.0 || area.height() < 40.0 || count == 0 {
        return;
    }
    let rects = tiles(area, count);
    let poses = s.poses();
    let selected = s.cam_index();
    let output = s.status.transport.output;
    let pixels_per_point = ui.ctx().pixels_per_point();

    // Tiles are drawn in turn, starting after the last one that got its turn, so a heavy model
    // slows every tile equally instead of leaving the last ones stale.
    let first = s.world.turn % count;
    let mut textures = vec![None; count];
    let mut last_drawn = None;
    for step in 0..count {
        let i = (first + step) % count;
        let size = stage::pixel_size(rects[i], pixels_per_point, 640.0);
        let name = format!("multi:{}", s.project.cameras[i].id);
        textures[i] = stage::camera_picture(ui.ctx(), s, &name, i, &poses, size, false);
        if s.world.drew {
            last_drawn = Some(i);
        }
    }
    if let Some(i) = last_drawn {
        s.world.turn = (i + 1) % count;
    }

    let mut choose = None;
    for (i, rect) in rects.iter().enumerate() {
        let camera = &s.project.cameras[i];
        let status = s.status.camera(&camera.id);
        let color = camera_color(i);
        stage::paint_picture(&painter, *rect, textures[i]);
        let on = i == selected;
        // A tile is framed in its camera's state, as the Program monitor is: red when it is held or has
        // faulted, green while output reaches it. The camera in hand is framed in the accent otherwise.
        let (word, ink) = crate::camera_ui::state(status, output);
        let frame = match word {
            "held" | "fault" | "stopping" => Some(RED),
            "live" => Some(ink),
            _ if on => Some(ACCENT),
            _ => None,
        };
        painter.rect_stroke(
            *rect,
            0.0,
            Stroke::new(if frame.is_some() { RULE_STRONG } else { RULE }, frame.unwrap_or(CONTROL_LINE)),
            StrokeKind::Outside,
        );
        if rect.width() > 110.0 {
            let label = format!("{} {}", i + 1, camera.name);
            // The camera in hand has its name on its own colour, as it has in the Stage view.
            let (fill, name_ink) = if on { (color, ink_on(color)) } else { (plate_fill(), color) };
            badge(&painter, rect.left_top() + vec2(U, U), Align2::LEFT_TOP, &label, fill, name_ink);
            badge(&painter, rect.right_top() + vec2(-U, U), Align2::RIGHT_TOP, word, plate_fill(), ink);
            let focal = format!("{:.0} mm", camera.lens.focal_mm(poses[i].zoom));
            badge(&painter, rect.right_bottom() + vec2(-U, -U), Align2::RIGHT_BOTTOM, &focal, plate_fill(), OFF_WHITE);
        }
        let r = ui.interact(*rect, ui.id().with(("multiview_tile", i)), Sense::click()).on_hover_text(format!(
            "Select camera {}: {}",
            i + 1,
            camera.name
        ));
        if r.clicked() {
            choose = Some(camera.id.clone());
        }
        s.probe_tiles.push((camera.id.clone(), *rect));
    }
    if let Some(wait) = crate::monitor::stage_wait(s) {
        badge(&painter, area.center_bottom(), Align2::CENTER_BOTTOM, wait, plate_fill(), AMBER);
    }
    if let Some(id) = choose {
        s.select_camera(&id);
        s.axis_lanes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn the_grid_grows_with_the_camera_count_and_every_tile_is_sixteen_by_nine_inside_the_area() {
        let shapes: Vec<(usize, usize)> = (1..=12).map(grid).collect();
        assert_eq!(shapes, vec![(1, 1), (2, 1), (2, 2), (2, 2), (3, 2), (3, 2), (3, 3), (3, 3), (3, 3), (4, 3), (4, 3), (4, 3)]);
        let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(1000.0, 500.0));
        for count in 1..=12 {
            let rects = tiles(area, count);
            assert_eq!(rects.len(), count);
            for (i, r) in rects.iter().enumerate() {
                assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-3, "{count} cameras, tile {i}: {r:?}");
                assert!(area.expand(0.01).contains_rect(*r), "{count} cameras, tile {i}: {r:?}");
                for other in &rects[..i] {
                    assert!(!r.intersects(*other), "{count} cameras: tiles overlap");
                }
            }
        }
    }
}
