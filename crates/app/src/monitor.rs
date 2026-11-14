//! Program monitor: a drawn stage seen from the camera pose. It stands in for
//! the camera picture, so a move can be judged before any hardware is live.

use crate::session::Session;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use ptz_core::api::PoseSource;
use ptz_core::pose::{Axis, Pose};

/// Horizontal field of view in degrees for a zoom of 0 (wide) to 1 (tight).
pub fn hfov_deg(zoom: f64) -> f64 {
    let z = zoom.clamp(0.0, 1.0);
    62.0 * (6.0f64 / 62.0).powf(z)
}

/// Camera lens height in metres for an elevation in millimetres.
pub fn lens_height_m(elevation_mm: f64) -> f64 {
    1.2 + elevation_mm / 1000.0
}

#[derive(Clone, Copy)]
struct V3 {
    x: f64,
    y: f64,
    z: f64,
}

fn v(x: f64, y: f64, z: f64) -> V3 {
    V3 { x, y, z }
}

pub struct Camera {
    pos: V3,
    yaw: f64,
    pitch: f64,
    focal: f64,
    centre: Pos2,
}

const NEAR: f64 = 0.4;

impl Camera {
    pub fn new(pose: &Pose, rect: Rect) -> Camera {
        let hfov = hfov_deg(pose.zoom).to_radians();
        Camera {
            pos: v(0.0, lens_height_m(pose.elevation), 0.0),
            // Positive pan turns the camera to its left.
            yaw: -pose.pan.to_radians(),
            pitch: pose.tilt.to_radians(),
            focal: rect.width() as f64 / 2.0 / (hfov / 2.0).tan(),
            centre: rect.center(),
        }
    }

    /// World point to camera space: x right, y up, z forward.
    fn view(&self, p: V3) -> V3 {
        let (dx, dy, dz) = (p.x - self.pos.x, p.y - self.pos.y, p.z - self.pos.z);
        let (sy, cy) = self.yaw.sin_cos();
        let x1 = dx * cy - dz * sy;
        let z1 = dx * sy + dz * cy;
        let (sp, cp) = self.pitch.sin_cos();
        let y2 = dy * cp - z1 * sp;
        let z2 = dy * sp + z1 * cp;
        v(x1, y2, z2)
    }

    fn screen(&self, c: V3) -> Pos2 {
        pos2(self.centre.x + (self.focal * c.x / c.z) as f32, self.centre.y - (self.focal * c.y / c.z) as f32)
    }

    /// Where a world point lands on screen, when it is in front of the camera.
    pub fn project(&self, x: f64, y: f64, z: f64) -> Option<Pos2> {
        let c = self.view(v(x, y, z));
        (c.z > NEAR).then(|| self.screen(c))
    }

    fn segment(&self, a: V3, b: V3) -> Option<[Pos2; 2]> {
        let (mut a, mut b) = (self.view(a), self.view(b));
        if a.z < NEAR && b.z < NEAR {
            return None;
        }
        if a.z < NEAR || b.z < NEAR {
            let t = (NEAR - a.z) / (b.z - a.z);
            let cut = v(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, NEAR);
            if a.z < NEAR {
                a = cut;
            } else {
                b = cut;
            }
        }
        Some([self.screen(a), self.screen(b)])
    }

    /// A flat polygon, clipped against the near plane.
    fn polygon(&self, pts: &[V3]) -> Vec<Pos2> {
        let cam: Vec<V3> = pts.iter().map(|p| self.view(*p)).collect();
        let mut out = Vec::new();
        for i in 0..cam.len() {
            let (a, b) = (cam[i], cam[(i + 1) % cam.len()]);
            let (ain, bin) = (a.z >= NEAR, b.z >= NEAR);
            if ain {
                out.push(self.screen(a));
            }
            if ain != bin {
                let t = (NEAR - a.z) / (b.z - a.z);
                out.push(self.screen(v(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, NEAR)));
            }
        }
        out
    }
}

const WALL_Z: f64 = 16.0;
const STAGE_FRONT_Z: f64 = 9.0;
const STAGE_H: f64 = 0.5;
const HALF_W: f64 = 10.0;
pub const LECTERN_X: f64 = 5.3;
pub const AWARDS_X: f64 = -3.9;
pub const MARK_Z: f64 = 12.0;

fn fill(painter: &Painter, cam: &Camera, pts: &[V3], color: Color32) {
    let poly = cam.polygon(pts);
    if poly.len() >= 3 {
        painter.add(egui::Shape::convex_polygon(poly, color, Stroke::NONE));
    }
}

fn line(painter: &Painter, cam: &Camera, a: V3, b: V3, stroke: Stroke) {
    if let Some(seg) = cam.segment(a, b) {
        painter.line_segment(seg, stroke);
    }
}

fn person(painter: &Painter, cam: &Camera, x: f64, z: f64, color: Color32) {
    let floor = STAGE_H;
    let (w, shoulder, top) = (0.24, 1.45, 1.78);
    fill(
        painter,
        cam,
        &[v(x - w, floor, z), v(x + w, floor, z), v(x + w, floor + shoulder, z), v(x - w, floor + shoulder, z)],
        color,
    );
    if let (Some(c), Some(edge)) = (cam.project(x, floor + top - 0.13, z), cam.project(x + 0.13, floor + top - 0.13, z)) {
        painter.circle_filled(c, (edge.x - c.x).abs().max(1.0), mix(color, OFF_WHITE, 0.25));
    }
}

pub fn paint_stage(painter: &Painter, rect: Rect, pose: &Pose) {
    let cam = Camera::new(pose, rect);
    let p = painter.with_clip_rect(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(8, 10, 22));

    // Auditorium floor and its grid.
    fill(
        &p,
        &cam,
        &[v(-HALF_W, 0.0, 0.5), v(HALF_W, 0.0, 0.5), v(HALF_W, 0.0, STAGE_FRONT_Z), v(-HALF_W, 0.0, STAGE_FRONT_Z)],
        Color32::from_rgb(20, 22, 36),
    );
    let grid = Stroke::new(1.0, Color32::from_rgb(42, 48, 78));
    for i in -10..=10 {
        line(&p, &cam, v(i as f64, 0.0, 0.5), v(i as f64, 0.0, STAGE_FRONT_Z), grid);
    }
    for i in 1..=9 {
        line(&p, &cam, v(-HALF_W, 0.0, i as f64), v(HALF_W, 0.0, i as f64), grid);
    }

    // Back wall with a screen.
    let wall = Color32::from_rgb(24, 30, 62);
    fill(
        &p,
        &cam,
        &[v(-HALF_W, STAGE_H, WALL_Z), v(HALF_W, STAGE_H, WALL_Z), v(HALF_W, 7.0, WALL_Z), v(-HALF_W, 7.0, WALL_Z)],
        wall,
    );
    fill(
        &p,
        &cam,
        &[v(-4.0, 2.6, WALL_Z - 0.05), v(4.0, 2.6, WALL_Z - 0.05), v(4.0, 6.2, WALL_Z - 0.05), v(-4.0, 6.2, WALL_Z - 0.05)],
        mix(NAVY, BLUE, 0.35),
    );
    for i in -4..=4 {
        let x = i as f64 * 2.5;
        line(&p, &cam, v(x, STAGE_H, WALL_Z), v(x, 7.0, WALL_Z), Stroke::new(1.0, Color32::from_rgb(34, 42, 84)));
    }

    // Stage deck and riser.
    fill(
        &p,
        &cam,
        &[
            v(-HALF_W, STAGE_H, STAGE_FRONT_Z),
            v(HALF_W, STAGE_H, STAGE_FRONT_Z),
            v(HALF_W, STAGE_H, WALL_Z),
            v(-HALF_W, STAGE_H, WALL_Z),
        ],
        Color32::from_rgb(44, 40, 46),
    );
    for i in 0..=7 {
        let z = STAGE_FRONT_Z + i as f64;
        line(&p, &cam, v(-HALF_W, STAGE_H, z), v(HALF_W, STAGE_H, z), Stroke::new(1.0, Color32::from_rgb(58, 54, 62)));
    }
    fill(
        &p,
        &cam,
        &[
            v(-HALF_W, 0.0, STAGE_FRONT_Z),
            v(HALF_W, 0.0, STAGE_FRONT_Z),
            v(HALF_W, STAGE_H, STAGE_FRONT_Z),
            v(-HALF_W, STAGE_H, STAGE_FRONT_Z),
        ],
        Color32::from_rgb(30, 28, 34),
    );
    line(
        &p,
        &cam,
        v(-HALF_W, STAGE_H, STAGE_FRONT_Z),
        v(HALF_W, STAGE_H, STAGE_FRONT_Z),
        Stroke::new(1.5, with_alpha(OFF_WHITE, 90)),
    );

    // Awards positions: two marks and two people.
    for dx in [-0.55, 0.55] {
        let x = AWARDS_X + dx;
        fill(
            &p,
            &cam,
            &[
                v(x - 0.3, STAGE_H + 0.005, MARK_Z - 0.3),
                v(x + 0.3, STAGE_H + 0.005, MARK_Z - 0.3),
                v(x + 0.3, STAGE_H + 0.005, MARK_Z + 0.3),
                v(x - 0.3, STAGE_H + 0.005, MARK_Z + 0.3),
            ],
            with_alpha(GREEN, 70),
        );
        person(&p, &cam, x, MARK_Z, Color32::from_rgb(120, 126, 150));
    }

    // Lectern with a presenter behind it.
    person(&p, &cam, LECTERN_X, MARK_Z + 0.45, Color32::from_rgb(150, 132, 120));
    let (lx, lw) = (LECTERN_X, 0.38);
    fill(
        &p,
        &cam,
        &[
            v(lx - lw, STAGE_H, MARK_Z),
            v(lx + lw, STAGE_H, MARK_Z),
            v(lx + lw, STAGE_H + 1.15, MARK_Z),
            v(lx - lw, STAGE_H + 1.15, MARK_Z),
        ],
        mix(BLUE, CHARCOAL, 0.45),
    );
    fill(
        &p,
        &cam,
        &[
            v(lx - lw, STAGE_H + 1.15, MARK_Z),
            v(lx + lw, STAGE_H + 1.15, MARK_Z),
            v(lx + lw, STAGE_H + 1.22, MARK_Z + 0.35),
            v(lx - lw, STAGE_H + 1.22, MARK_Z + 0.35),
        ],
        mix(BLUE, OFF_WHITE, 0.2),
    );

    // Frame guides: centre cross and thirds.
    let guide = Stroke::new(1.0, with_alpha(OFF_WHITE, 28));
    for t in [1.0 / 3.0, 2.0 / 3.0] {
        p.vline(rect.left() + rect.width() * t, rect.y_range(), guide);
        p.hline(rect.x_range(), rect.top() + rect.height() * t, guide);
    }
    let c = rect.center();
    p.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], Stroke::new(1.0, with_alpha(OFF_WHITE, 120)));
    p.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], Stroke::new(1.0, with_alpha(OFF_WHITE, 120)));
}

fn badge(painter: &Painter, at: Pos2, anchor: Align2, text: &str, fill: Color32, ink: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), ink);
    let size = galley.size() + vec2(14.0, 6.0);
    let r = anchor.anchor_size(at, size);
    painter.rect_filled(r, 4.0, fill);
    painter.galley(r.min + vec2(7.0, 3.0), galley, ink);
    r
}

pub fn show(ui: &mut Ui, s: &Session) {
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(avail, Sense::hover());
    let area = resp.rect;
    painter.rect_filled(area, 0.0, PANEL_DARK);

    let readout_h = 26.0;
    let room = Rect::from_min_max(area.min + vec2(8.0, 8.0), area.max - vec2(8.0, readout_h + 8.0));
    if room.width() < 40.0 || room.height() < 30.0 {
        return;
    }
    let (w, h) = if room.width() / room.height() > 16.0 / 9.0 {
        (room.height() * 16.0 / 9.0, room.height())
    } else {
        (room.width(), room.width() * 9.0 / 16.0)
    };
    let pic = Rect::from_center_size(room.center(), vec2(w, h));

    let (pose, source) = s.monitor_pose();
    paint_stage(&painter, pic, &pose);

    let live = s.status.hardware_live;
    let t = &s.status.transport;
    let border = if t.estop {
        RED
    } else if live {
        GREEN
    } else {
        LINE
    };
    painter.rect_stroke(pic, 0.0, Stroke::new(if live || t.estop { 2.0 } else { 1.0 }, border), StrokeKind::Outside);

    let simulated = !s.cam_status().hardware;
    let (text, fill_c, ink) = if t.estop {
        ("E-STOP".to_string(), RED, OFF_WHITE)
    } else if live {
        (if simulated { "LIVE  simulator".to_string() } else { "LIVE  rig is moving".to_string() }, GREEN, NAVY)
    } else {
        ("REHEARSE  nothing is sent to the rig".to_string(), with_alpha(NAVY, 230), OFF_WHITE)
    };
    badge(&painter, pic.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, &text, fill_c, ink);
    let what = match source {
        PoseSource::Timeline => "Programmed pose",
        PoseSource::Rig => "Rig pose",
        PoseSource::Record => "Recording: rig on record axes",
    };
    badge(&painter, pic.right_top() + vec2(-8.0, 8.0), Align2::RIGHT_TOP, what, with_alpha(PANEL_DARK, 220), OFF_WHITE);
    if s.status.transport.record {
        painter.circle_filled(pic.right_top() + vec2(-16.0, 44.0), 6.0, RED);
    }
    badge(
        &painter,
        pic.left_bottom() + vec2(8.0, -8.0),
        Align2::LEFT_BOTTOM,
        &s.timecode_at(s.playhead().floor() as i64),
        with_alpha(PANEL_DARK, 220),
        GREEN,
    );

    // Pose readout under the picture.
    let y = pic.bottom() + 6.0 + readout_h / 2.0;
    let step = (pic.width() / 5.0).min(150.0);
    for (i, axis) in Axis::ALL.iter().enumerate() {
        let x = pic.left() + step * i as f32;
        painter.circle_filled(pos2(x + 4.0, y), 3.5, axis_color(*axis));
        painter.text(
            pos2(x + 12.0, y),
            Align2::LEFT_CENTER,
            format!("{} {}", axis.label(), axis.format(pose.get(*axis))),
            FontId::proportional(11.5),
            OFF_WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 540.0))
    }

    #[test]
    fn field_of_view_narrows_with_zoom() {
        assert!((hfov_deg(0.0) - 62.0).abs() < 1e-9);
        assert!((hfov_deg(1.0) - 6.0).abs() < 1e-9);
        assert!(hfov_deg(0.5) < 25.0 && hfov_deg(0.5) > 15.0);
    }

    #[test]
    fn panning_to_a_subject_centres_it() {
        // Lectern MCU is seeded at pan -24: the lectern sits on the picture's vertical centre line.
        let mut pose = Pose::home();
        pose.pan = -(LECTERN_X / MARK_Z).atan().to_degrees();
        let cam = Camera::new(&pose, rect());
        let p = cam.project(LECTERN_X, 1.5, MARK_Z).unwrap();
        assert!((p.x - 480.0).abs() < 1.0, "{p:?}");
        // With no pan it is right of centre.
        let cam = Camera::new(&Pose::home(), rect());
        assert!(cam.project(LECTERN_X, 1.5, MARK_Z).unwrap().x > 600.0);
        // Positive pan looks left: the awards marks come to centre.
        pose.pan = -(AWARDS_X / MARK_Z).atan().to_degrees();
        assert!(pose.pan > 0.0);
        let cam = Camera::new(&pose, rect());
        assert!((cam.project(AWARDS_X, 1.5, MARK_Z).unwrap().x - 480.0).abs() < 1.0);
    }

    #[test]
    fn tilt_zoom_and_elevation_move_the_picture_the_right_way() {
        let base = Pose::home();
        let at = |pose: &Pose| Camera::new(pose, rect()).project(1.0, 2.0, 12.0).unwrap();
        let p0 = at(&base);
        let mut up = base;
        up.tilt += 5.0;
        assert!(at(&up).y > p0.y, "tilting up moves the subject down the frame");
        let mut tight = base;
        tight.zoom += 0.3;
        assert!(at(&tight).x - 480.0 > p0.x - 480.0, "zooming in spreads points from the centre");
        let mut high = base;
        high.elevation += 400.0;
        assert!(at(&high).y > p0.y, "raising the camera moves the subject down the frame");
    }

    #[test]
    fn points_behind_the_camera_are_dropped() {
        let cam = Camera::new(&Pose::home(), rect());
        assert!(cam.project(0.0, 1.0, -3.0).is_none());
        assert!(cam.segment(v(0.0, 0.0, -5.0), v(0.0, 0.0, -1.0)).is_none());
        assert!(cam.segment(v(0.0, 0.0, -5.0), v(0.0, 0.0, 5.0)).is_some());
    }
}
