//! Boxes for lengths in the units the show is read in. The value behind a box is always
//! millimetres or metres; only what is shown and typed changes.

use egui::DragValue;
use ptz_core::Units;

/// A typed number as a tape measure gives it: "14.5", "14 3/8", "3/4", with or without its unit after it.
pub fn typed(text: &str) -> Option<f64> {
    let text = text.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace() || c == '"' || c == '\'');
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut total: Option<f64> = None;
    for part in text.split_whitespace() {
        let value = match part.split_once('/') {
            Some((over, under)) => {
                let (over, under): (f64, f64) = (over.parse().ok()?, under.parse().ok()?);
                if under == 0.0 {
                    return None;
                }
                over / under
            }
            None => part.parse::<f64>().ok()?,
        };
        total = Some(total.unwrap_or(0.0) + value);
    }
    total.filter(|v| v.is_finite()).map(|v| if negative { -v } else { v })
}

fn shown_in<'a>(value: &'a mut f64, per: f64, decimals: usize, unit: &'static str, speed: f64) -> DragValue<'a> {
    DragValue::new(value)
        .speed(speed * per)
        .custom_formatter(move |v, _| format!("{:.decimals$}", v / per))
        .custom_parser(move |text| typed(text).map(|v| v * per))
        .suffix(format!(" {unit}"))
}

/// A box for a height kept in millimetres: the column, the lens above the floor.
pub fn height_box(mm: &mut f64, units: Units) -> DragValue<'_> {
    match units {
        Units::Metric => shown_in(mm, 1.0, 0, units.short(), 1.0),
        Units::Imperial => shown_in(mm, units.mm_per_short(), 2, units.short(), 0.05),
    }
}

/// A box for a distance on the stage, kept in metres.
pub fn distance_box(m: &mut f64, units: Units) -> DragValue<'_> {
    shown_in(m, units.m_per_long(), 2, units.long(), 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_tape_measure_says_is_read_as_a_number() {
        assert_eq!(typed("14.5"), Some(14.5));
        assert_eq!(typed(" 14 3/8 "), Some(14.375));
        assert_eq!(typed("3/4"), Some(0.75));
        assert_eq!(typed("22 in"), Some(22.0));
        assert_eq!(typed("22\""), Some(22.0));
        assert_eq!(typed("560mm"), Some(560.0));
        assert_eq!(typed("-2.5"), Some(-2.5));
        assert_eq!(typed("-1 1/2"), Some(-1.5));
        for nonsense in ["", "in", "1/0", "a/2", "1 two 3", "1.2.3"] {
            assert_eq!(typed(nonsense), None, "{nonsense:?}");
        }
    }

    #[test]
    fn lengths_are_worded_in_the_units_of_the_show() {
        assert_eq!(Units::Metric.height(558.8), "559 mm");
        assert_eq!(Units::Imperial.height(558.8), "22.0 in");
        assert_eq!(Units::Metric.distance(3.048, 2), "3.05 m");
        assert_eq!(Units::Imperial.distance(3.048, 2), "10.00 ft");
        assert_eq!(ptz_core::Axis::Elevation.format_in(254.0, Units::Imperial), "10.0 in");
        assert_eq!(ptz_core::Axis::Pan.format_in(12.0, Units::Imperial), "12.0°", "angles are the same in both");
    }
}
