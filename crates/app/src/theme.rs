//! Colours and widget styling.

use egui::{Color32, CornerRadius, Stroke};

pub const NAVY: Color32 = Color32::from_rgb(0x09, 0x15, 0x40);
pub const CHARCOAL: Color32 = Color32::from_rgb(0x26, 0x27, 0x30);
pub const OFF_WHITE: Color32 = Color32::from_rgb(0xf1, 0xf2, 0xee);
pub const BLUE: Color32 = Color32::from_rgb(0x43, 0x81, 0xc1);
pub const GREEN: Color32 = Color32::from_rgb(0x5a, 0xff, 0x15);

/// Warnings: moves that overrun a limit, refused drops.
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb0, 0x20);
/// E-Stop and faults.
pub const RED: Color32 = Color32::from_rgb(0xe5, 0x3b, 0x3b);

pub const PANEL_DARK: Color32 = Color32::from_rgb(0x1b, 0x1c, 0x24);
pub const LANE: Color32 = Color32::from_rgb(0x20, 0x21, 0x2a);
pub const LANE_ALT: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x26);
pub const LINE: Color32 = Color32::from_rgb(0x3a, 0x3c, 0x4a);
pub const MUTED: Color32 = Color32::from_rgb(0x9a, 0x9d, 0xab);
pub const KEYS_CLIP: Color32 = Color32::from_rgb(0x6b, 0x5f, 0xc7);
pub const AUDIO_CLIP: Color32 = Color32::from_rgb(0x2f, 0x7a, 0x6b);
pub const VIDEO_CLIP: Color32 = Color32::from_rgb(0x8a, 0x5a, 0x9c);

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

pub fn axis_color(axis: ptz_core::Axis) -> Color32 {
    use ptz_core::Axis::*;
    match axis {
        Pan => Color32::from_rgb(0x5b, 0xa8, 0xf0),
        Tilt => Color32::from_rgb(0xf0, 0x8a, 0x5b),
        Zoom => Color32::from_rgb(0x8f, 0xe0, 0x6a),
        Focus => Color32::from_rgb(0xe0, 0xc8, 0x5a),
        Elevation => Color32::from_rgb(0xd0, 0x7a, 0xe0),
    }
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(OFF_WHITE);
        v.panel_fill = CHARCOAL;
        v.window_fill = CHARCOAL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = PANEL_DARK;
        v.faint_bg_color = LANE;
        v.code_bg_color = PANEL_DARK;
        v.hyperlink_color = BLUE;
        v.warn_fg_color = AMBER;
        v.error_fg_color = RED;
        v.selection.bg_fill = with_alpha(BLUE, 150);
        v.selection.stroke = Stroke::new(1.0, OFF_WHITE);

        let radius = CornerRadius::same(5);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CHARCOAL;
        w.noninteractive.weak_bg_fill = CHARCOAL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = Color32::from_rgb(0x34, 0x36, 0x44);
        w.inactive.weak_bg_fill = Color32::from_rgb(0x34, 0x36, 0x44);
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x46, 0x48, 0x58));
        w.inactive.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = Color32::from_rgb(0x40, 0x43, 0x55);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x40, 0x43, 0x55);
        w.hovered.bg_stroke = Stroke::new(1.0, BLUE);
        w.hovered.fg_stroke = Stroke::new(1.5, OFF_WHITE);
        w.hovered.corner_radius = radius;

        w.active.bg_fill = BLUE;
        w.active.weak_bg_fill = BLUE;
        w.active.bg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.active.fg_stroke = Stroke::new(1.5, OFF_WHITE);
        w.active.corner_radius = radius;

        w.open.bg_fill = Color32::from_rgb(0x34, 0x36, 0x44);
        w.open.weak_bg_fill = Color32::from_rgb(0x34, 0x36, 0x44);
        w.open.bg_stroke = Stroke::new(1.0, BLUE);
        w.open.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.open.corner_radius = radius;

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
    });
}

pub fn dock_style(egui_style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(egui_style);
    s.tab_bar.bg_fill = PANEL_DARK;
    s.tab_bar.hline_color = LINE;
    s.tab_bar.height = 28.0;
    s.tab.tab_body.bg_fill = CHARCOAL;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.inner_margin = egui::Margin::same(0);
    for (state, fill, text) in [
        (&mut s.tab.active, CHARCOAL, OFF_WHITE),
        (&mut s.tab.focused, CHARCOAL, OFF_WHITE),
        (&mut s.tab.inactive, PANEL_DARK, MUTED),
        (&mut s.tab.hovered, Color32::from_rgb(0x30, 0x32, 0x40), OFF_WHITE),
        (&mut s.tab.active_with_kb_focus, CHARCOAL, OFF_WHITE),
        (&mut s.tab.inactive_with_kb_focus, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.focused_with_kb_focus, CHARCOAL, OFF_WHITE),
    ] {
        state.bg_fill = fill;
        state.text_color = text;
        state.outline_color = LINE;
    }
    s.separator.color_idle = NAVY;
    s.separator.color_hovered = BLUE;
    s.separator.color_dragged = BLUE;
    s.separator.width = 3.0;
    s.main_surface_border_stroke = Stroke::NONE;
    s.overlay.selection_color = with_alpha(BLUE, 90);
    s
}

/// One colour per camera, used for its chip, its lane and its marker on the stage.
pub fn camera_color(index: usize) -> Color32 {
    const COLORS: [Color32; 12] = [
        Color32::from_rgb(0x43, 0x81, 0xc1),
        Color32::from_rgb(0x5a, 0xff, 0x15),
        Color32::from_rgb(0xff, 0xb0, 0x20),
        Color32::from_rgb(0xd0, 0x7a, 0xe0),
        Color32::from_rgb(0x4f, 0xd6, 0xc8),
        Color32::from_rgb(0xf0, 0x6a, 0x6a),
        Color32::from_rgb(0xe0, 0xd8, 0x5a),
        Color32::from_rgb(0x8f, 0x9c, 0xff),
        Color32::from_rgb(0xf0, 0x8a, 0x5b),
        Color32::from_rgb(0x6a, 0xd0, 0x8a),
        Color32::from_rgb(0xe0, 0x6a, 0xb0),
        Color32::from_rgb(0xb8, 0xc0, 0xcc),
    ];
    COLORS[index % COLORS.len()]
}
