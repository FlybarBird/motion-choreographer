//! Brand palette: navy / off-white / green / blue / charcoal.

use egui::{Color32, Stroke, Visuals};

pub const NAVY: Color32 = Color32::from_rgb(0x09, 0x15, 0x40);
pub const OFF_WHITE: Color32 = Color32::from_rgb(0xf1, 0xf2, 0xee);
pub const GREEN: Color32 = Color32::from_rgb(0x5a, 0xff, 0x15);
pub const BLUE: Color32 = Color32::from_rgb(0x43, 0x81, 0xc1);
pub const CHARCOAL: Color32 = Color32::from_rgb(0x26, 0x27, 0x30);
pub const CHARCOAL_HI: Color32 = Color32::from_rgb(0x36, 0x38, 0x45);
pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x3b, 0x3b);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x29);

pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.window_fill = NAVY;
    visuals.panel_fill = NAVY;
    visuals.extreme_bg_color = CHARCOAL;
    visuals.faint_bg_color = CHARCOAL_HI;
    visuals.override_text_color = Some(OFF_WHITE);
    visuals.widgets.inactive.bg_fill = CHARCOAL;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, OFF_WHITE);
    visuals.widgets.hovered.bg_fill = CHARCOAL_HI;
    visuals.widgets.active.bg_fill = BLUE;
    visuals.selection.bg_fill = BLUE.gamma_multiply(0.45);
    visuals.hyperlink_color = BLUE;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = DANGER;
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    ctx.set_style(style);
}
