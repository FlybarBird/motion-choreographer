//! Colours, type and widget styling.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

/// The window behind the panels, and the ink on bright fills.
pub const BASE: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x0e);
pub const PANEL: Color32 = Color32::from_rgb(0x18, 0x19, 0x1c);
pub const PANEL_DARK: Color32 = Color32::from_rgb(0x11, 0x12, 0x14);
pub const OFF_WHITE: Color32 = Color32::from_rgb(0xe9, 0xea, 0xed);
pub const MUTED: Color32 = Color32::from_rgb(0x8c, 0x90, 0x99);
pub const LINE: Color32 = Color32::from_rgb(0x2c, 0x2e, 0x34);
pub const LANE: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x21);
pub const LANE_ALT: Color32 = Color32::from_rgb(0x16, 0x17, 0x1a);

/// A control at rest, and under the pointer.
pub const CONTROL: Color32 = Color32::from_rgb(0x26, 0x28, 0x2d);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x32, 0x35, 0x3b);
pub const CONTROL_LINE: Color32 = Color32::from_rgb(0x38, 0x3b, 0x42);

/// The next thing to do, and whatever has the operator's attention.
pub const ACCENT: Color32 = Color32::from_rgb(0xf5, 0xa6, 0x23);
/// Warnings: moves that overrun a limit, refused drops.
pub const AMBER: Color32 = ACCENT;
/// Camera moves, selections and locked timecode.
pub const BLUE: Color32 = Color32::from_rgb(0x4a, 0x90, 0xe2);
/// Live output.
pub const GREEN: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x74);
/// E-Stop and faults.
pub const RED: Color32 = Color32::from_rgb(0xef, 0x47, 0x47);

pub const KEYS_CLIP: Color32 = Color32::from_rgb(0x7a, 0x6a, 0xe0);
pub const AUDIO_CLIP: Color32 = Color32::from_rgb(0x2a, 0x8c, 0x78);
pub const VIDEO_CLIP: Color32 = Color32::from_rgb(0x9a, 0x5f, 0xb0);

pub mod icon {
    pub use egui_phosphor::regular::*;
}

const SEMIBOLD: &str = "inter-semibold";
const ICONS: &str = "phosphor";

/// Heavier text for names and headings; egui's `strong` only brightens.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), egui::FontData::from_static(bytes).into());
    };
    add("inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(SEMIBOLD, include_bytes!("../assets/fonts/Inter-SemiBold.ttf"));
    add("jetbrains-mono", include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"));
    add(ICONS, egui_phosphor::Variant::Regular.font_bytes());

    // egui's own fonts stay behind ours for the symbols Inter does not carry.
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &[&str]| first.iter().map(|n| n.to_string()).chain(fallback.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, with(&["inter", ICONS]));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with(&[SEMIBOLD, ICONS]));
    fonts.families.insert(FontFamily::Name(ICONS.into()), with(&[ICONS]));
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "jetbrains-mono".into());
    fonts
}

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
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
            (TextStyle::Heading, semibold(17.0)),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(OFF_WHITE);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, CONTROL_LINE);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = BASE;
        v.faint_bg_color = LANE;
        v.code_bg_color = PANEL_DARK;
        v.hyperlink_color = BLUE;
        v.warn_fg_color = AMBER;
        v.error_fg_color = RED;
        v.selection.bg_fill = with_alpha(ACCENT, 90);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.popup_shadow.color = Color32::from_black_alpha(140);
        v.window_shadow.color = Color32::from_black_alpha(160);

        let radius = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = CONTROL;
        w.inactive.weak_bg_fill = CONTROL;
        w.inactive.bg_stroke = Stroke::new(1.0, CONTROL_LINE);
        w.inactive.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = CONTROL_HOVER;
        w.hovered.weak_bg_fill = CONTROL_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, mix(CONTROL_LINE, OFF_WHITE, 0.35));
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = mix(CONTROL_HOVER, ACCENT, 0.35);
        w.active.weak_bg_fill = mix(CONTROL_HOVER, ACCENT, 0.35);
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.bg_fill = CONTROL_HOVER;
        w.open.weak_bg_fill = CONTROL_HOVER;
        w.open.bg_stroke = Stroke::new(1.0, ACCENT);
        w.open.fg_stroke = Stroke::new(1.0, OFF_WHITE);
        w.open.corner_radius = radius;

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.window_margin = egui::Margin::same(16);
    });
}

pub fn dock_style(egui_style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(egui_style);
    let top = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
    s.tab_bar.bg_fill = PANEL_DARK;
    s.tab_bar.hline_color = LINE;
    s.tab_bar.height = 30.0;
    s.tab_bar.corner_radius = top;
    s.tab.tab_body.bg_fill = PANEL;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.inner_margin = egui::Margin::same(0);
    s.tab.tab_body.corner_radius = CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 };
    s.tab.hline_below_active_tab_name = false;
    s.tab.spacing = 2.0;
    for (state, fill, text) in [
        (&mut s.tab.active, PANEL, OFF_WHITE),
        (&mut s.tab.focused, PANEL, OFF_WHITE),
        (&mut s.tab.inactive, PANEL_DARK, MUTED),
        (&mut s.tab.hovered, CONTROL, OFF_WHITE),
        (&mut s.tab.active_with_kb_focus, PANEL, OFF_WHITE),
        (&mut s.tab.inactive_with_kb_focus, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.focused_with_kb_focus, PANEL, OFF_WHITE),
    ] {
        state.bg_fill = fill;
        state.text_color = text;
        state.outline_color = Color32::TRANSPARENT;
        state.corner_radius = top;
    }
    s.separator.color_idle = BASE;
    s.separator.color_hovered = ACCENT;
    s.separator.color_dragged = ACCENT;
    s.separator.width = 4.0;
    s.main_surface_border_stroke = Stroke::NONE;
    s.overlay.selection_color = with_alpha(ACCENT, 70);
    s
}

/// The ink that reads on a filled plate of this colour.
pub fn ink_on(fill: Color32) -> Color32 {
    let luma = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if luma > 140.0 {
        BASE
    } else {
        Color32::WHITE
    }
}

/// A filled button: the answer a dialog is asking for.
pub fn filled(label: impl Into<String>, fill: Color32) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label).font(semibold(13.0)).color(ink_on(fill))).fill(fill).stroke(Stroke::NONE)
}

const CHIP_PADDING: egui::Vec2 = egui::vec2(14.0, 6.0);

fn chip_font() -> FontId {
    semibold(10.5)
}

/// How wide a chip with this word on it is.
pub fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), chip_font(), Color32::PLACEHOLDER).size().x + CHIP_PADDING.x
}

/// How high every chip is.
pub fn chip_height(ui: &egui::Ui) -> f32 {
    ui.ctx().fonts_mut(|f| f.row_height(&chip_font())) + CHIP_PADDING.y
}

/// A chip of state: a short word on a plate, solid when the state is one to act on.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32, solid: bool) -> egui::Response {
    chip_within(ui, text, color, solid, f32::INFINITY)
}

/// A chip no wider than `max_width`: words that do not fit are cut short with an ellipsis.
/// A screen reader, and a test, still find all of them.
pub fn chip_within(ui: &mut egui::Ui, text: &str, color: Color32, solid: bool, max_width: f32) -> egui::Response {
    let (fill, ink, edge) =
        if solid { (color, ink_on(color), color) } else { (with_alpha(color, 30), color, with_alpha(color, 110)) };
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), chip_font(), ink);
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - CHIP_PADDING.x).max(0.0));
    let galley = ui.painter().layout_job(job);
    // Sized to its word, whatever the height of the row it sits in.
    let (rect, response) = ui.allocate_exact_size(galley.size() + CHIP_PADDING, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if ui.is_rect_visible(rect) {
        ui.painter().rect(rect, 5.0, fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
    }
    response
}

/// How a button with an icon is filled.
#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    /// A control like any other.
    Plain,
    /// No plate until the pointer is over it: for rows of secondary actions.
    Quiet,
    /// Filled: the one thing to do next, or a state that is on.
    Solid(Color32),
    /// Outlined in a colour: consequential, but not the next step.
    Outline(Color32),
}

/// A button with an icon ahead of its label. The label alone is what a screen reader, and a test, finds.
pub fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    glyph: &str,
    label: &str,
    look: Look,
    min_size: egui::Vec2,
) -> egui::Response {
    let id = egui::Id::new("icon_button_glyph");
    let size = 15.0;
    let ink = match look {
        Look::Solid(c) if enabled => ink_on(c),
        Look::Outline(c) if enabled => c,
        _ => OFF_WHITE,
    };
    let text = egui::RichText::new(label).color(ink);
    let text = if matches!(look, Look::Solid(_)) { text.font(semibold(13.0)) } else { text };
    let mut button = egui::Button::new((egui::Atom::custom(id, egui::vec2(size, size)), text)).min_size(min_size);
    match look {
        Look::Solid(c) if enabled => button = button.fill(c).stroke(Stroke::NONE),
        Look::Outline(c) if enabled => button = button.fill(with_alpha(c, 22)).stroke(Stroke::new(1.0, with_alpha(c, 150))),
        Look::Quiet => button = button.frame_when_inactive(false),
        _ => {}
    }
    ui.add_enabled_ui(enabled, |ui| {
        let r = button.atom_ui(ui);
        if let Some(rect) = r.rect(id) {
            let ink = if enabled { ink } else { ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.4) };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, icon_font(size + 1.0), ink);
        }
        r.response
    })
    .inner
}

/// One colour per camera, used for its chip, its lane and its marker on the stage.
pub fn camera_color(index: usize) -> Color32 {
    const COLORS: [Color32; 12] = [
        Color32::from_rgb(0x4a, 0x90, 0xe2),
        Color32::from_rgb(0x4c, 0xd9, 0x7b),
        Color32::from_rgb(0xff, 0x8a, 0x4c),
        Color32::from_rgb(0xc7, 0x7d, 0xff),
        Color32::from_rgb(0x3f, 0xd4, 0xc6),
        Color32::from_rgb(0xf0, 0x60, 0x7a),
        Color32::from_rgb(0xe6, 0xd4, 0x4f),
        Color32::from_rgb(0x8f, 0x9c, 0xff),
        Color32::from_rgb(0xa3, 0xe0, 0x4f),
        Color32::from_rgb(0x5c, 0xc8, 0xf0),
        Color32::from_rgb(0xe8, 0x6a, 0xb8),
        Color32::from_rgb(0xb8, 0xc0, 0xcc),
    ];
    COLORS[index % COLORS.len()]
}
