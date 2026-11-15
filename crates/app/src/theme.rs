//! The look of the whole app: one palette, one type scale, one spacing unit, and the widgets drawn with them.
//!
//! The rules, in short. Grounds are neutral greys and carry no meaning. One accent, blue, marks what has
//! the operator's attention: the next step, the tab in front, the control in hand. Red, amber and green
//! are kept for the state of the machine and mean the same thing wherever they appear. Regions are
//! divided by hairline rules, not by boxes or shadows. Type is Inter in four sizes, with figures that
//! are read as numbers in JetBrains Mono. Every distance is a multiple of [`U`].

use egui::{Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, StrokeKind, TextStyle};

// ---------------------------------------------------------------- grounds

/// The window behind the panels, the wells that values are typed into, and the ink on bright plates.
pub const BASE: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0f);
/// Recessed strips: bars, rulers, tab rows.
pub const PANEL_DARK: Color32 = Color32::from_rgb(0x15, 0x15, 0x15);
/// The surface of a panel.
pub const PANEL: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1b);
/// Timeline lanes, alternating.
pub const LANE: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
pub const LANE_ALT: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1b);
/// A hairline rule between regions.
pub const LINE: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);

/// A control at rest, and under the pointer.
pub const CONTROL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x36, 0x36, 0x36);
/// The edge of a control.
pub const CONTROL_LINE: Color32 = Color32::from_rgb(0x47, 0x47, 0x47);

/// Text, and text that only explains.
pub const OFF_WHITE: Color32 = Color32::from_rgb(0xed, 0xed, 0xed);
pub const MUTED: Color32 = Color32::from_rgb(0x8f, 0x8f, 0x8f);

// ------------------------------------------------------ accent and states

/// The one accent: the next thing to do, the tab in front, the control in hand, locked timecode.
pub const ACCENT: Color32 = Color32::from_rgb(0x35, 0x74, 0xf0);
/// The accent under its older name, where it marks a selection or a lock.
pub const BLUE: Color32 = ACCENT;
/// Caution: something is not as programmed, is on its way, or needs a decision.
pub const AMBER: Color32 = Color32::from_rgb(0xf7, 0xb5, 0x00);
/// Live: output reaches the rig.
pub const GREEN: Color32 = Color32::from_rgb(0x30, 0xc8, 0x5a);
/// Stop: E-Stop, faults, held cameras, and Record, which writes over what was programmed.
pub const RED: Color32 = Color32::from_rgb(0xf2, 0x30, 0x30);

/// What a clip on the timeline is, on its leading edge. Identity, not state: these are soft where the state colours are full.
pub const MOVE_CLIP: Color32 = Color32::from_rgb(0x6a, 0xa6, 0xff);
pub const KEYS_CLIP: Color32 = Color32::from_rgb(0xa8, 0x94, 0xf5);
pub const AUDIO_CLIP: Color32 = Color32::from_rgb(0x4f, 0xc4, 0xb0);
pub const VIDEO_CLIP: Color32 = Color32::from_rgb(0xd0, 0x8a, 0xe0);

// ------------------------------------------------------------ measures

/// The spacing unit. Every gap, margin and control height is a whole number of these.
pub const U: f32 = 4.0;
/// Between a control and its neighbour.
pub const GAP: f32 = 2.0 * U;
/// From the edge of a panel to what is in it.
pub const PAD: f32 = 3.0 * U;
/// The corner of a control. Panels, lanes and tabs are square.
pub const RADIUS: u8 = 2;
/// A hairline, and the rule that marks what is in front or in hand.
pub const RULE: f32 = 1.0;
pub const RULE_STRONG: f32 = 2.0;
/// Control heights: in a toolbar, in a form, and in the transport bar.
pub const CONTROL_SMALL: f32 = 5.0 * U;
pub const CONTROL_H: f32 = 6.0 * U;
pub const CONTROL_LARGE: f32 = 8.0 * U;
/// The strip of tools at the top of a tab.
pub const TOOLBAR_H: f32 = 8.0 * U;

// ---------------------------------------------------------------- type

/// The type scale. Nothing is set in a size that is not one of these.
pub const MICRO: f32 = 10.0;
pub const SMALL: f32 = 11.0;
pub const BODY: f32 = 13.0;
pub const TITLE: f32 = 16.0;
pub const DISPLAY: f32 = 22.0;
pub const HERO: f32 = 28.0;

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

/// Running text in a painted surface.
pub fn body() -> FontId {
    FontId::proportional(BODY)
}

/// Labels on lanes, badges and readouts.
pub fn small() -> FontId {
    FontId::proportional(SMALL)
}

/// The smallest text: ruler figures, details under a name.
pub fn micro() -> FontId {
    FontId::proportional(MICRO)
}

/// Figures: timecode, positions, anything read as a number.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// How far apart the letters of a label in capitals are set.
const TRACKING: f32 = 0.7;

/// The name of a region, in small spaced capitals: a tab, a section, a column of a table.
pub fn caps(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(semibold(MICRO)).extra_letter_spacing(TRACKING)
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

// -------------------------------------------------------------- colour

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// One colour per axis, for its curve and its name. Identity, not state.
pub fn axis_color(axis: ptz_core::Axis) -> Color32 {
    use ptz_core::Axis::*;
    match axis {
        Pan => Color32::from_rgb(0x62, 0xa8, 0xf5),
        Tilt => Color32::from_rgb(0xf5, 0x8a, 0xa5),
        Zoom => Color32::from_rgb(0x58, 0xd3, 0xb8),
        Focus => Color32::from_rgb(0xd9, 0xc5, 0x6a),
        Elevation => Color32::from_rgb(0xb7, 0x94, 0xf6),
    }
}

/// One colour per camera, used for its chip, its lane and its marker on the stage.
pub fn camera_color(index: usize) -> Color32 {
    const COLORS: [Color32; 12] = [
        Color32::from_rgb(0x6a, 0xa6, 0xff),
        Color32::from_rgb(0x5f, 0xd0, 0xa0),
        Color32::from_rgb(0xff, 0xa3, 0x66),
        Color32::from_rgb(0xc4, 0x93, 0xff),
        Color32::from_rgb(0x5a, 0xd1, 0xd9),
        Color32::from_rgb(0xff, 0x8f, 0xa8),
        Color32::from_rgb(0xe3, 0xd3, 0x6b),
        Color32::from_rgb(0x9a, 0xa5, 0xff),
        Color32::from_rgb(0xb5, 0xe0, 0x6a),
        Color32::from_rgb(0x7f, 0xc8, 0xf5),
        Color32::from_rgb(0xf0, 0x8f, 0xd0),
        Color32::from_rgb(0xc2, 0xc8, 0xd2),
    ];
    COLORS[index % COLORS.len()]
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

/// The state of the machine as one colour, the same wherever it is shown: the rule under the transport,
/// the clock, the playhead and the frame of a monitor. Red while E-Stop is latched, amber while the rig
/// travels to the playhead by itself, green while output reaches the rig and follows the timeline.
/// None in rehearsal, when nothing reaches the rig.
pub fn machine_ink(t: &ptz_core::transport::TransportState, live: bool) -> Option<Color32> {
    use ptz_core::transport::Output;
    if t.estop {
        Some(RED)
    } else if t.output == Output::Positioning {
        Some(AMBER)
    } else if live || t.output == Output::Armed {
        Some(GREEN)
    } else {
        None
    }
}

/// The colour of the show's clock wherever it is written: the machine's state, and plain ink in rehearsal.
pub fn clock_ink(t: &ptz_core::transport::TransportState, live: bool) -> Color32 {
    machine_ink(t, live).unwrap_or(OFF_WHITE)
}

/// The colour of the playhead: the machine's state, and the accent in rehearsal.
pub fn playhead_ink(t: &ptz_core::transport::TransportState, live: bool) -> Color32 {
    machine_ink(t, live).unwrap_or(ACCENT)
}

// --------------------------------------------------------------- style

pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(SMALL)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Monospace, FontId::monospace(BODY - 1.0)),
            (TextStyle::Heading, semibold(TITLE)),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(OFF_WHITE);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(RULE, CONTROL_LINE);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.extreme_bg_color = BASE;
        v.faint_bg_color = LANE;
        v.code_bg_color = PANEL_DARK;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = AMBER;
        v.error_fg_color = RED;
        // What is switched on, or chosen from a list: the accent, held back so that it does not compete with the next step.
        v.selection.bg_fill = mix(PANEL, ACCENT, 0.5);
        v.selection.stroke = Stroke::new(RULE, Color32::WHITE);
        v.text_cursor.stroke = Stroke::new(RULE_STRONG, ACCENT);
        // A rule round a menu or a dialog sets it off; nothing casts a shadow.
        v.popup_shadow = egui::Shadow::NONE;
        v.window_shadow = egui::Shadow::NONE;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };

        let radius = CornerRadius::same(RADIUS);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(RULE, LINE);
        w.noninteractive.fg_stroke = Stroke::new(RULE, OFF_WHITE);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = CONTROL;
        w.inactive.weak_bg_fill = CONTROL;
        w.inactive.bg_stroke = Stroke::new(RULE, CONTROL_LINE);
        w.inactive.fg_stroke = Stroke::new(RULE, OFF_WHITE);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = CONTROL_HOVER;
        w.hovered.weak_bg_fill = CONTROL_HOVER;
        w.hovered.bg_stroke = Stroke::new(RULE, MUTED);
        w.hovered.fg_stroke = Stroke::new(RULE, Color32::WHITE);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = mix(CONTROL_HOVER, ACCENT, 0.35);
        w.active.weak_bg_fill = mix(CONTROL_HOVER, ACCENT, 0.35);
        w.active.bg_stroke = Stroke::new(RULE, ACCENT);
        w.active.fg_stroke = Stroke::new(RULE, Color32::WHITE);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.bg_fill = CONTROL_HOVER;
        w.open.weak_bg_fill = CONTROL_HOVER;
        w.open.bg_stroke = Stroke::new(RULE, ACCENT);
        w.open.fg_stroke = Stroke::new(RULE, OFF_WHITE);
        w.open.corner_radius = radius;

        style.spacing.item_spacing = egui::vec2(GAP, GAP);
        style.spacing.button_padding = egui::vec2(GAP + 2.0, U);
        style.spacing.interact_size.y = CONTROL_H;
        style.spacing.menu_margin = egui::Margin::same(U as i8);
        style.spacing.window_margin = egui::Margin::same((4.0 * U) as i8);
    });
}

/// The dock: tabs are words on a strip, the one in front is underlined in the accent, and panels
/// meet at a hairline.
pub fn dock_style(egui_style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(egui_style);
    s.tab_bar.bg_fill = PANEL_DARK;
    s.tab_bar.hline_color = LINE;
    s.tab_bar.height = 7.0 * U;
    s.tab_bar.corner_radius = CornerRadius::ZERO;
    // The first tab's name starts at the panel margin, in line with what is under it.
    s.tab_bar.inner_margin = egui::Margin { left: (PAD - TAB_PADDING) as i8, ..Default::default() };
    s.tab.tab_body.bg_fill = PANEL;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.inner_margin = egui::Margin::same(0);
    s.tab.tab_body.corner_radius = CornerRadius::ZERO;
    // The rule under the strip runs its whole width; [`tab_in_front`] draws the accent over it.
    s.tab.hline_below_active_tab_name = true;
    s.tab.spacing = 0.0;
    for (state, fill, text) in [
        (&mut s.tab.active, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.focused, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.inactive, PANEL_DARK, MUTED),
        (&mut s.tab.hovered, CONTROL, OFF_WHITE),
        (&mut s.tab.active_with_kb_focus, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.inactive_with_kb_focus, PANEL_DARK, OFF_WHITE),
        (&mut s.tab.focused_with_kb_focus, PANEL_DARK, OFF_WHITE),
    ] {
        state.bg_fill = fill;
        state.text_color = text;
        state.outline_color = Color32::TRANSPARENT;
        state.corner_radius = CornerRadius::ZERO;
    }
    s.separator.color_idle = BASE;
    s.separator.color_hovered = ACCENT;
    s.separator.color_dragged = ACCENT;
    s.separator.width = RULE_STRONG;
    s.separator.extra_interact_width = U;
    s.main_surface_border_stroke = Stroke::NONE;
    s.overlay.selection_color = with_alpha(ACCENT, 70);
    s
}

/// The room egui_dock leaves on either side of a tab's name.
const TAB_PADDING: f32 = 8.0;

/// The title of a dock tab, as it is set on the tab strip.
pub fn tab_title(title: &str) -> egui::WidgetText {
    caps(title).into()
}

/// Mark the tab that is in front of its group: the accent rule under its name.
pub fn tab_in_front(response: &egui::Response) {
    let rect = response.rect;
    let painter = response.ctx.layer_painter(response.layer_id);
    painter.hline(rect.shrink2(egui::vec2(TAB_PADDING, 0.0)).x_range(), rect.bottom() - RULE, Stroke::new(RULE_STRONG, ACCENT));
}

// ------------------------------------------------------------- layout

/// A hairline across what is left of the width.
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), RULE), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(RULE, LINE));
}

/// The heading of a section of a panel: a rule, then its name in capitals.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(U);
    hairline(ui);
    ui.add_space(U);
    ui.label(caps(title).color(MUTED));
}

/// What a panel says when it has nothing to show: set flush left at the panel's margin, like everything in it.
pub fn empty<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let said = egui::Frame::new().inner_margin(egui::Margin::same(PAD as i8)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = U;
        add(ui)
    });
    said.inner
}

/// The name of a field in a form: quieter than the value beside it.
pub fn field(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).color(MUTED))
}

/// Text that only explains.
pub fn note(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).color(MUTED))
}

/// The strip of tools at the top of a tab: one row, a margin on either side, a hairline under it.
/// Controls in it are the small size.
pub fn toolbar<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inner = egui::Frame::new()
        .fill(PANEL)
        .inner_margin(egui::Margin::symmetric(PAD as i8, U as i8))
        .show(ui, |ui| {
            ui.set_min_height(TOOLBAR_H - 2.0 * U);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().interact_size.y = CONTROL_SMALL;
                ui.spacing_mut().item_spacing = egui::vec2(U, U);
                add(ui)
            })
            .inner
        })
        .inner;
    let rule = ui.min_rect().bottom();
    ui.painter().hline(ui.max_rect().x_range(), rule, Stroke::new(RULE, LINE));
    inner
}

/// The margin round the scrolling contents of a form panel.
pub fn panel_margin() -> egui::Margin {
    egui::Margin::symmetric(PAD as i8, GAP as i8)
}

/// How far the window behind a dialog is dimmed.
pub const BACKDROP: Color32 = Color32::from_black_alpha(150);

/// The frame of a dialog: a panel with a rule round it and room inside.
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(RULE, CONTROL_LINE))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::same((5.0 * U) as i8))
}

/// The title of a dialog, with a hairline under it.
pub fn dialog_title(ui: &mut egui::Ui, text: impl Into<RichText>) {
    ui.label(text.into().font(semibold(TITLE)));
    hairline(ui);
    ui.add_space(U);
}

/// A plate with a word on it over a picture: dark enough to read on anything behind it.
pub fn plate(
    painter: &egui::Painter,
    at: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
    fill: Color32,
    ink: Color32,
) -> egui::Rect {
    let galley = painter.layout_no_wrap(text.to_string(), small(), ink);
    let size = galley.size() + egui::vec2(2.0 * GAP, 2.0 * U);
    let r = anchor.anchor_size(at, size);
    painter.rect_filled(r, RADIUS, fill);
    painter.galley(r.min + egui::vec2(GAP, U), galley, ink);
    r
}

/// The fill of a plate that says nothing about state.
pub fn plate_fill() -> Color32 {
    with_alpha(BASE, 215)
}

// -------------------------------------------------------------- widgets

/// A filled button: the answer a dialog is asking for.
pub fn filled(label: impl Into<String>, fill: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label).font(semibold(BODY)).color(ink_on(fill))).fill(fill).stroke(Stroke::NONE)
}

const CHIP_PADDING: egui::Vec2 = egui::vec2(2.0 * GAP, 2.0 * U);

fn chip_job(text: &str, ink: Color32, max_width: f32) -> egui::text::LayoutJob {
    let format = egui::TextFormat { font_id: semibold(MICRO), color: ink, extra_letter_spacing: TRACKING, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), format);
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - CHIP_PADDING.x).max(0.0));
    job
}

/// How wide a chip with this word on it is.
pub fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter().layout_job(chip_job(text, Color32::PLACEHOLDER, f32::INFINITY)).size().x + CHIP_PADDING.x
}

/// How high every chip is.
pub fn chip_height(ui: &egui::Ui) -> f32 {
    ui.ctx().fonts_mut(|f| f.row_height(&semibold(MICRO))) + CHIP_PADDING.y
}

/// A chip of state: a short word on a plate, solid when the state is one to act on.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32, solid: bool) -> egui::Response {
    chip_within(ui, text, color, solid, f32::INFINITY)
}

/// A chip no wider than `max_width`: words that do not fit are cut short with an ellipsis.
/// A screen reader, and a test, still find all of them.
pub fn chip_within(ui: &mut egui::Ui, text: &str, color: Color32, solid: bool, max_width: f32) -> egui::Response {
    let (fill, ink, edge) =
        if solid { (color, ink_on(color), color) } else { (Color32::TRANSPARENT, color, with_alpha(color, 90)) };
    let galley = ui.painter().layout_job(chip_job(text, ink, max_width));
    // Sized to its word, whatever the height of the row it sits in.
    let (rect, response) = ui.allocate_exact_size(galley.size() + CHIP_PADDING, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if ui.is_rect_visible(rect) {
        ui.painter().rect(rect, RADIUS, fill, Stroke::new(RULE, edge), StrokeKind::Inside);
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
    icon_button_sensing(ui, enabled, glyph, label, look, min_size, egui::Sense::click(), BODY)
}

/// An [`icon_button`] that only a click presses. It never has the keyboard, so Tab passes it by and
/// neither Space nor Enter can press it. For the controls that the manual says have no key.
pub fn click_only_button(
    ui: &mut egui::Ui,
    enabled: bool,
    glyph: &str,
    label: &str,
    look: Look,
    min_size: egui::Vec2,
) -> egui::Response {
    icon_button_sensing(ui, enabled, glyph, label, look, min_size, egui::Sense::CLICK, BODY)
}

/// The stop button. It is larger than any other control, its words are set a size up, and it stands
/// on a yellow ground that nothing else in the window has: red on yellow is the emergency stop and
/// only that. `ground` is the whole place kept for it; the button itself is `size` in its middle.
pub fn stop_button(ui: &mut egui::Ui, glyph: &str, label: &str, ground: egui::Rect, size: egui::Vec2) -> egui::Response {
    ui.painter().rect_filled(ground, RADIUS, AMBER);
    let place = egui::Rect::from_center_size(ground.center(), size);
    ui.scope_builder(egui::UiBuilder::new().max_rect(place), |ui| {
        icon_button_sensing(ui, true, glyph, label, Look::Solid(RED), size, egui::Sense::click(), TITLE - 1.0)
    })
    .inner
}

/// What stands in the stop button's place while the stop is latched: the same yellow ground and the
/// same red plate, saying so. It is not a button; the way out is the reset beside it.
pub fn stop_latched(ui: &mut egui::Ui, text: &str, ground: egui::Vec2, size: egui::Vec2) -> egui::Response {
    let (ground, response) = ui.allocate_exact_size(ground, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let plate = egui::Rect::from_center_size(ground.center(), size);
    ui.painter().rect_filled(ground, RADIUS, AMBER);
    ui.painter().rect_filled(plate, RADIUS, RED);
    let galley = ui.painter().layout_job(chip_job(text, ink_on(RED), f32::INFINITY));
    ui.painter().galley(plate.center() - galley.size() / 2.0, galley, ink_on(RED));
    response
}

fn icon_button_sensing(
    ui: &mut egui::Ui,
    enabled: bool,
    glyph: &str,
    label: &str,
    look: Look,
    min_size: egui::Vec2,
    sense: egui::Sense,
    text_size: f32,
) -> egui::Response {
    let id = egui::Id::new("icon_button_glyph");
    let size = text_size + 3.0;
    let ink = match look {
        Look::Solid(c) if enabled => ink_on(c),
        Look::Outline(c) if enabled => c,
        _ => OFF_WHITE,
    };
    let text = RichText::new(label).color(ink);
    let text = if matches!(look, Look::Solid(_)) { text.font(semibold(text_size)) } else { text.size(text_size) };
    let mut button = egui::Button::new((egui::Atom::custom(id, egui::vec2(size, size)), text)).min_size(min_size).sense(sense);
    match look {
        Look::Solid(c) if enabled => button = button.fill(c).stroke(Stroke::NONE),
        Look::Outline(c) if enabled => button = button.fill(with_alpha(c, 18)).stroke(Stroke::new(RULE, c)),
        Look::Quiet => button = button.frame_when_inactive(false),
        _ => {}
    }
    ui.add_enabled_ui(enabled, |ui| {
        let r = button.atom_ui(ui);
        if let Some(rect) = r.rect(id) {
            let ink = if enabled { ink } else { ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.4) };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, icon_font(size), ink);
        }
        r.response
    })
    .inner
}

/// A small square in a colour with a figure on it: a camera's number, wherever the camera is named.
pub fn number_tag(painter: &egui::Painter, rect: egui::Rect, number: usize, color: Color32) {
    painter.rect_filled(rect, RADIUS, color);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("{number}"), semibold(SMALL), ink_on(color));
}

/// The size of a [`number_tag`] in a row of text.
pub const NUMBER_TAG: egui::Vec2 = egui::vec2(5.0 * U, 4.0 * U + 2.0);

/// A level in a well: how far along its range something is. For tracking error and positions.
pub fn level_bar(ui: &mut egui::Ui, width: f32) -> (egui::Rect, egui::Painter) {
    let (bar, _) = ui.allocate_exact_size(egui::vec2(width, U), egui::Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(bar, 0.0, BASE);
    (bar, painter)
}
