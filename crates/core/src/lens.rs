//! A camera's lens and what follows from it: angle of view at a zoom, how much
//! of the room the frame holds at a distance, and depth of field.
//!
//! The picture is 16:9. `sensor_width_mm` is the width of that picture area on
//! the sensor, so its height is nine sixteenths of it.

use serde::{Deserialize, Serialize};

/// Width of the FR7's full-frame 16:9 picture area.
pub const FULL_FRAME_WIDTH_MM: f64 = 35.6;
/// Width of its Super 35 crop.
pub const SUPER_35_WIDTH_MM: f64 = 23.7;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Lens {
    pub name: String,
    pub sensor_width_mm: f64,
    /// Focal length at zoom 0.
    pub wide_mm: f64,
    /// Focal length at zoom 1. Equal to `wide_mm` for a prime.
    pub tele_mm: f64,
    /// Widest aperture, for depth of field.
    pub f_number: f64,
}

impl Default for Lens {
    /// The zoom sold with the FR7, on full frame.
    fn default() -> Self {
        Lens::zoom("FE PZ 28-135mm F4 G OSS", 28.0, 135.0, 4.0)
    }
}

pub const CUSTOM: &str = "Custom";

impl Lens {
    fn zoom(name: &str, wide_mm: f64, tele_mm: f64, f_number: f64) -> Lens {
        Lens { name: name.into(), sensor_width_mm: FULL_FRAME_WIDTH_MM, wide_mm, tele_mm, f_number }
    }

    /// Lenses offered by name. Any field can be typed over afterwards.
    pub fn presets() -> Vec<Lens> {
        vec![
            Lens::default(),
            Lens::zoom("FE PZ 16-35mm F4 G", 16.0, 35.0, 4.0),
            Lens::zoom("FE 24-70mm F2.8 GM II", 24.0, 70.0, 2.8),
            Lens::zoom("FE 70-200mm F2.8 GM OSS II", 70.0, 200.0, 2.8),
            Lens::zoom("FE 200-600mm F5.6-6.3 G OSS", 200.0, 600.0, 6.3),
        ]
    }

    pub fn validate(&self) -> Result<(), String> {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if !positive(self.sensor_width_mm) || self.sensor_width_mm > 200.0 {
            return Err("the sensor width must be above 0 mm".into());
        }
        if !positive(self.wide_mm) || !positive(self.tele_mm) || self.tele_mm > 5000.0 {
            return Err("both focal lengths must be above 0 mm".into());
        }
        if self.tele_mm < self.wide_mm {
            return Err("the tele end cannot be shorter than the wide end".into());
        }
        if !positive(self.f_number) {
            return Err("the aperture must be above 0".into());
        }
        Ok(())
    }

    pub fn sensor_height_mm(&self) -> f64 {
        self.sensor_width_mm * 9.0 / 16.0
    }

    pub fn sensor_diagonal_mm(&self) -> f64 {
        self.sensor_width_mm.hypot(self.sensor_height_mm())
    }

    /// Focal length at a zoom position from 0 to 1. Equal steps of zoom multiply
    /// the focal length by equal amounts: Sony does not publish the real curve,
    /// so the two ends are exact and the middle is this assumption.
    pub fn focal_mm(&self, zoom: f64) -> f64 {
        let (wide, tele) = (self.wide_mm.max(1e-3), self.tele_mm.max(self.wide_mm).max(1e-3));
        wide * (tele / wide).powf(zoom.clamp(0.0, 1.0))
    }

    fn angle_deg(&self, size_mm: f64, zoom: f64) -> f64 {
        2.0 * (size_mm / (2.0 * self.focal_mm(zoom))).atan().to_degrees()
    }

    /// Horizontal angle of view in degrees.
    pub fn hfov_deg(&self, zoom: f64) -> f64 {
        self.angle_deg(self.sensor_width_mm, zoom)
    }

    pub fn vfov_deg(&self, zoom: f64) -> f64 {
        self.angle_deg(self.sensor_height_mm(), zoom)
    }

    pub fn dfov_deg(&self, zoom: f64) -> f64 {
        self.angle_deg(self.sensor_diagonal_mm(), zoom)
    }

    /// Width and height of what the frame holds at a distance, in the distance's unit.
    pub fn frame_at(&self, zoom: f64, distance: f64) -> (f64, f64) {
        let f = self.focal_mm(zoom);
        (distance * self.sensor_width_mm / f, distance * self.sensor_height_mm() / f)
    }

    /// How far back the camera has to be for the frame to be `width` wide.
    pub fn distance_for_width(&self, zoom: f64, width: f64) -> f64 {
        width * self.focal_mm(zoom) / self.sensor_width_mm
    }

    /// The largest blur that still reads as sharp: the picture diagonal over 1500.
    pub fn circle_of_confusion_mm(&self) -> f64 {
        self.sensor_diagonal_mm() / 1500.0
    }

    /// Focused here, everything from half this distance to infinity is sharp. Metres.
    pub fn hyperfocal_m(&self, zoom: f64) -> f64 {
        let f = self.focal_mm(zoom);
        (f * f / (self.f_number.max(1e-3) * self.circle_of_confusion_mm()) + f) / 1000.0
    }

    /// Nearest and farthest sharp distance, in metres, when focused on a subject
    /// `subject_m` away at the widest aperture. No far limit means sharp to infinity.
    pub fn sharp_range_m(&self, zoom: f64, subject_m: f64) -> (f64, Option<f64>) {
        let f = self.focal_mm(zoom) / 1000.0;
        let h = self.hyperfocal_m(zoom);
        let s = subject_m.max(f);
        let near = s * (h - f) / (h + s - 2.0 * f);
        let far = (s < h).then(|| s * (h - f) / (h - s));
        (near, far)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, within: f64) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn the_kit_zoom_on_full_frame_matches_hand_calculation() {
        let lens = Lens::default();
        assert!(lens.validate().is_ok());
        assert!(close(lens.sensor_height_mm(), 20.025, 1e-9));
        assert!(close(lens.focal_mm(0.0), 28.0, 1e-9) && close(lens.focal_mm(1.0), 135.0, 1e-9));
        // 2 atan(17.8 / 28) and 2 atan(10.0125 / 28).
        assert!(close(lens.hfov_deg(0.0), 64.87, 0.01), "{}", lens.hfov_deg(0.0));
        assert!(close(lens.vfov_deg(0.0), 39.35, 0.01), "{}", lens.vfov_deg(0.0));
        assert!(close(lens.hfov_deg(1.0), 15.02, 0.01), "{}", lens.hfov_deg(1.0));
        assert!(close(lens.vfov_deg(1.0), 8.48, 0.01), "{}", lens.vfov_deg(1.0));
        // Half way is the geometric mean: sqrt(28 x 135).
        assert!(close(lens.focal_mm(0.5), 61.48, 0.01));
        assert!(close(lens.hfov_deg(0.5), 32.29, 0.01), "{}", lens.hfov_deg(0.5));
        let (w, h) = lens.frame_at(0.5, 10.0);
        assert!(close(w, 5.790, 0.002) && close(h, 3.257, 0.002), "{w} x {h}");
        assert!(close(lens.distance_for_width(0.5, w), 10.0, 1e-9));
        assert!(lens.dfov_deg(0.0) > lens.hfov_deg(0.0));
        // Out-of-range zoom is held at the ends.
        assert_eq!(lens.focal_mm(-1.0), 28.0);
        assert!(close(lens.focal_mm(3.0), 135.0, 1e-9));
    }

    #[test]
    fn other_lenses_and_a_crop_sensor() {
        let long = &Lens::presets()[3];
        assert_eq!(long.name, "FE 70-200mm F2.8 GM OSS II");
        // 2 atan(17.8 / 200).
        assert!(close(long.hfov_deg(1.0), 10.17, 0.01), "{}", long.hfov_deg(1.0));
        let (w, _) = long.frame_at(1.0, 30.0);
        assert!(close(w, 5.34, 0.001), "{w}");

        // The same focal length on Super 35 sees about two thirds as wide.
        let crop = Lens { sensor_width_mm: SUPER_35_WIDTH_MM, ..Lens::default() };
        assert!(close(crop.hfov_deg(0.0), 45.88, 0.01), "{}", crop.hfov_deg(0.0));
        assert!(close(crop.frame_at(0.0, 10.0).0 / Lens::default().frame_at(0.0, 10.0).0, 23.7 / 35.6, 1e-9));

        // A prime has one angle at every zoom.
        let prime = Lens { name: CUSTOM.into(), wide_mm: 50.0, tele_mm: 50.0, ..Lens::default() };
        assert!(prime.validate().is_ok());
        assert_eq!(prime.hfov_deg(0.0), prime.hfov_deg(0.7));
        for lens in Lens::presets() {
            assert!(lens.validate().is_ok(), "{}", lens.name);
        }
    }

    #[test]
    fn depth_of_field_matches_hand_calculation() {
        let lens = Lens::default();
        // Diagonal sqrt(35.6^2 + 20.025^2) = 40.846 mm, over 1500.
        assert!(close(lens.circle_of_confusion_mm(), 0.02723, 1e-5));
        // 61.48^2 / (4 x 0.02723) + 61.48 = 34 763 mm.
        let h = lens.hyperfocal_m(0.5);
        assert!(close(h, 34.76, 0.02), "{h}");
        // Near: 12.4 x (H - f) / (H + 12.4 - 2 f); far: 12.4 x (H - f) / (H - 12.4).
        let (near, far) = lens.sharp_range_m(0.5, 12.4);
        assert!(close(near, 9.15, 0.01), "{near}");
        assert!(close(far.unwrap(), 19.24, 0.02), "{far:?}");
        // Wide open at the wide end, focused past the hyperfocal distance, everything behind is sharp.
        let wide_h = lens.hyperfocal_m(0.0);
        assert!(close(wide_h, 7.23, 0.01), "{wide_h}");
        let (near, far) = lens.sharp_range_m(0.0, 8.0);
        assert!(far.is_none() && near < 8.0 && near > 3.5, "{near}");
        // A faster lens at the same focal length holds less in focus.
        let fast = Lens { f_number: 2.8, ..Lens::default() };
        assert!(fast.hyperfocal_m(0.5) > h);
    }

    #[test]
    fn a_lens_that_cannot_exist_is_refused() {
        let bad = |change: fn(&mut Lens)| {
            let mut lens = Lens::default();
            change(&mut lens);
            lens.validate().unwrap_err()
        };
        assert_eq!(bad(|l| l.sensor_width_mm = 0.0), "the sensor width must be above 0 mm");
        assert_eq!(bad(|l| l.wide_mm = -5.0), "both focal lengths must be above 0 mm");
        assert_eq!(bad(|l| l.tele_mm = f64::NAN), "both focal lengths must be above 0 mm");
        assert_eq!(bad(|l| l.tele_mm = 20.0), "the tele end cannot be shorter than the wide end");
        assert_eq!(bad(|l| l.f_number = 0.0), "the aperture must be above 0");
    }

    #[test]
    fn an_old_or_partial_lens_reads_with_defaults() {
        let lens: Lens = serde_json::from_str(r#"{ "wide_mm": 24, "tele_mm": 70 }"#).unwrap();
        assert_eq!((lens.wide_mm, lens.tele_mm, lens.sensor_width_mm, lens.f_number), (24.0, 70.0, 35.6, 4.0));
    }
}
