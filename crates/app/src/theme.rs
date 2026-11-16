//! The look of the whole app, after the Motion Choreographer brand (brand/BRAND.md): one palette, one
//! type scale, one spacing unit, and the widgets drawn with them.
//!
//! The rules, in short. The ground is Ink and the type is Paper; greys carry structure and nothing else.
//! Colour does a job: Signal red means live (on air, the rig follows the timeline, and E-STOP and Record,
//! which act on it at once), green means ready (armed, the next thing to go), amber means a warning.
//! Nothing else is coloured. Regions are divided by 1 px rules, never by boxes or shadows; corners are
//! square. Type is Inter in two weights, 400 and 700, with labels in spaced capitals. Every distance is a
//! multiple of [`U`].

use egui::{Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, StrokeKind, TextStyle};

// ---------------------------------------------------------------- brand

/// The ground, and type on Paper.
pub const INK: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
/// Type on Ink, and the primary control.
pub const PAPER: Color32 = Color32::WHITE;
/// Live. The keyframe. The only accent.
pub const SIGNAL: Color32 = Color32::from_rgb(0xff, 0x30, 0x00);
/// Secondary text on dark.
pub const GREY_400: Color32 = Color32::from_rgb(0xa3, 0xa3, 0xa3);
/// Text that is there but not in use.
pub const GREY_600: Color32 = Color32::from_rgb(0x6b, 0x6b, 0x6b);
/// Rules on dark.
pub const GREY_800: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
/// Surface on dark.
pub const GREY_900: Color32 = Color32::from_rgb(0x16, 0x16, 0x16);

// -------------------------------------------- the same, by what they are for

/// The wells that values are typed into, and the ground behind pictures.
pub const BASE: Color32 = GREY_900;
/// Bars, rulers, tab rows.
pub const PANEL_DARK: Color32 = INK;
/// The surface of a panel.
pub const PANEL: Color32 = INK;
/// Timeline lanes, alternating.
pub const LANE: Color32 = INK;
pub const LANE_ALT: Color32 = Color32::from_rgb(0x10, 0x10, 0x10);
/// A 1 px rule between regions.
pub const LINE: Color32 = GREY_800;

/// A control at rest, and under the pointer.
pub const CONTROL: Color32 = GREY_900;
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
/// The edge of a control.
pub const CONTROL_LINE: Color32 = GREY_800;

/// Text, and text that only explains.
pub const OFF_WHITE: Color32 = PAPER;
pub const MUTED: Color32 = GREY_400;

// ------------------------------------------------------ signals

/// What is in hand and what to do next: Paper. The primary button, the tab in front, the selection.
pub const ACCENT: Color32 = PAPER;
/// The accent under its older name, where it marks a selection or a lock.
pub const BLUE: Color32 = ACCENT;
/// The playhead while nothing reaches the rig.
pub const PLAYHEAD: Color32 = PAPER;
/// Caution: something is not as programmed, is on its way, or needs a decision.
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb8, 0x00);
/// Preview: armed, ready, next.
pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xc2, 0x4e);
/// Program: live. E-STOP, faults and Record, which act on what is live, say it in the same red.
pub const RED: Color32 = SIGNAL;

/// What a clip on the timeline is, on its leading edge. Greys: nothing but state is coloured.
pub const MOVE_CLIP: Color32 = PAPER;
pub const KEYS_CLIP: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd4);
pub const AUDIO_CLIP: Color32 = GREY_400;
pub const VIDEO_CLIP: Color32 = GREY_400;

// ------------------------------------------------------------ measures

/// The spacing unit. Every gap, margin and control height is a whole number of these.
pub const U: f32 = 4.0;
/// Between a control and its neighbour.
pub const GAP: f32 = 2.0 * U;
/// From the edge of a panel to what is in it.
pub const PAD: f32 = 3.0 * U;
/// Corners are square.
pub const RADIUS: u8 = 0;
/// A rule, and the rule that marks what is in front or in hand.
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

/// Inter at 700, the brand's one other weight. The name is kept from when it was a semibold.
const SEMIBOLD: &str = "inter-bold";
/// Figures, in Inter. Where they change while they are read they are also set in fixed cells: see [`figures`].
const FIGURES: &str = "inter-figures";
const ICONS: &str = "phosphor";

/// Heavier text for names and headings: Inter Bold.
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

/// Figures: timecode, positions, anything read as a number. Inter, as everything is.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FIGURES.into()))
}

/// How far apart the letters of a label are set: 0.08 em.
const TRACKING: f32 = 0.9;

/// A label: the name of a region, a section, a column, in spaced capitals.
pub fn caps(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(FontId::proportional(SMALL)).extra_letter_spacing(TRACKING)
}

/// Paint figures that change while they are read, each in a cell as wide as the widest figure, so that
/// they hold still. Inter's figures are proportional, and egui cannot turn on its tabular ones.
/// Returns the rectangle they were set in.
pub fn figures(
    painter: &egui::Painter,
    at: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> egui::Rect {
    let cell = painter.layout_no_wrap("0".into(), font.clone(), color).size();
    let narrow = |c: char| !c.is_ascii_digit() && c != '-' && c != '+' && c != ' ';
    let width_of = |c: char| if narrow(c) { painter.layout_no_wrap(c.to_string(), font.clone(), color).size().x } else { cell.x };
    let width: f32 = text.chars().map(width_of).sum();
    let rect = anchor.anchor_size(at, egui::vec2(width, cell.y));
    let mut x = rect.left();
    for c in text.chars() {
        let w = width_of(c);
        painter.text(egui::pos2(x + w / 2.0, rect.top()), egui::Align2::CENTER_TOP, c, font.clone(), color);
        x += w;
    }
    rect
}

fn fonts() -> egui::FontDefinitions {
    use egui::epaint::text::{FontTweak, VariationCoords};
    let mut fonts = egui::FontDefinitions::default();
    let inter: &'static [u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
    let weight = |w: f32| {
        let mut data = egui::FontData::from_static(inter);
        data.tweak = FontTweak { coords: VariationCoords::new([(b"wght", w)]), ..Default::default() };
        data
    };
    fonts.font_data.insert("inter".into(), weight(400.0).into());
    fonts.font_data.insert(SEMIBOLD.into(), weight(700.0).into());
    fonts.font_data.insert(
        "jetbrains-mono".into(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf")).into(),
    );
    fonts.font_data.insert(ICONS.into(), egui::FontData::from_static(egui_phosphor::Variant::Regular.font_bytes()).into());

    // egui's own fonts stay behind ours for the symbols Inter does not carry.
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &[&str]| first.iter().map(|n| n.to_string()).chain(fallback.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, with(&["inter", ICONS]));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with(&[SEMIBOLD, ICONS]));
    fonts.families.insert(FontFamily::Name(FIGURES.into()), with(&["inter"]));
    fonts.families.insert(FontFamily::Name(ICONS.into()), with(&[ICONS]));
    // The log and the fields of codes keep a typewriter face, where columns have to line up.
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

/// One ink per axis, for its curve and its name: steps of grey from Paper, since nothing but state is
/// coloured. Pan is the brightest.
pub fn axis_color(axis: ptz_core::Axis) -> Color32 {
    use ptz_core::Axis::*;
    match axis {
        Pan => PAPER,
        Tilt => Color32::from_rgb(0xd4, 0xd4, 0xd4),
        Zoom => Color32::from_rgb(0xb4, 0xb4, 0xb4),
        Focus => Color32::from_rgb(0x94, 0x94, 0x94),
        Elevation => Color32::from_rgb(0x78, 0x78, 0x78),
    }
}

/// A camera's ink, for its chip, its lane and its marker on the stage. Cameras are told apart by their
/// numbers and names, not by colour: every camera is the same light grey.
pub fn camera_color(_index: usize) -> Color32 {
    Color32::from_rgb(0xd4, 0xd4, 0xd4)
}

/// The ink that reads on a filled plate of this colour: Ink on Paper, Signal, green and amber, as the
/// brand sets labels on red; Paper on anything dark.
pub fn ink_on(fill: Color32) -> Color32 {
    let luma = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if luma > 95.0 {
        INK
    } else {
        PAPER
    }
}

/// The state of the machine as one colour, the same wherever it is shown: the rules round the transport,
/// the clock, the playhead and the frame of a monitor. Red when it is live (the rig follows the timeline
/// as it plays) and while E-Stop is latched; green while it is armed and ready; amber while the rig
/// travels to the playhead by itself. None in rehearsal, when nothing reaches the rig.
pub fn machine_ink(t: &ptz_core::transport::TransportState, live: bool) -> Option<Color32> {
    use ptz_core::transport::{Output, Run};
    if t.estop {
        Some(RED)
    } else if t.output == Output::Positioning {
        Some(AMBER)
    } else if live || t.output == Output::Armed {
        Some(if t.run == Run::Playing { RED } else { GREEN })
    } else {
        None
    }
}

/// The colour of the show's clock wherever it is written: the machine's state, and Paper in rehearsal.
pub fn clock_ink(t: &ptz_core::transport::TransportState, live: bool) -> Color32 {
    machine_ink(t, live).unwrap_or(PAPER)
}

/// The colour of the playhead: the machine's state, and Paper in rehearsal.
pub fn playhead_ink(t: &ptz_core::transport::TransportState, live: bool) -> Color32 {
    machine_ink(t, live).unwrap_or(PLAYHEAD)
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
        v.override_text_color = None;
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
        // What is switched on: a darker field and the accent in its words, never a filled plate.
        v.selection.bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x3a);
        v.selection.stroke = Stroke::new(RULE, PAPER);
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
        w.noninteractive.bg_stroke = Stroke::new(RULE, LINE);
        w.noninteractive.corner_radius = radius;

        // A button at rest is its words in a 1 px rule, with no ground of its own; a field to type in is
        // a Grey 900 well in the same rule.
        w.inactive.bg_fill = CONTROL;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        // A control at rest is a field a shade lighter than its panel, with no rule round it.
        w.inactive.bg_stroke = Stroke::new(RULE, GREY_800);
        w.inactive.fg_stroke = Stroke::new(RULE, OFF_WHITE);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = CONTROL_HOVER;
        w.hovered.weak_bg_fill = CONTROL_HOVER;
        w.hovered.bg_stroke = Stroke::new(RULE, GREY_400);
        w.hovered.fg_stroke = Stroke::new(RULE, PAPER);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = GREY_800;
        w.active.weak_bg_fill = GREY_800;
        w.active.bg_stroke = Stroke::new(RULE, PAPER);
        w.active.fg_stroke = Stroke::new(RULE, PAPER);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.bg_fill = CONTROL_HOVER;
        w.open.weak_bg_fill = CONTROL_HOVER;
        w.open.bg_stroke = Stroke::new(RULE, GREY_400);
        w.open.fg_stroke = Stroke::new(RULE, OFF_WHITE);
        w.open.corner_radius = radius;

        style.spacing.item_spacing = egui::vec2(GAP, GAP);
        style.spacing.button_padding = egui::vec2(GAP + 2.0, U);
        style.spacing.interact_size.y = CONTROL_H;
        style.spacing.menu_margin = egui::Margin::same(GAP as i8);
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
        (&mut s.tab.inactive, PANEL_DARK, GREY_600),
        (&mut s.tab.hovered, PANEL_DARK, OFF_WHITE),
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
    painter.hline(rect.shrink2(egui::vec2(TAB_PADDING, 0.0)).x_range(), rect.bottom() - RULE, Stroke::new(RULE, PAPER));
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
    let strip = egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(PAD as i8, U as i8)).show(ui, |ui| {
        ui.set_min_height(TOOLBAR_H - 2.0 * U);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.y = CONTROL_SMALL;
            ui.spacing_mut().item_spacing = egui::vec2(U, U);
            add(ui)
        })
        .inner
    });
    // The rule lies on the row of pixels under the strip.
    let rule = strip.response.rect.bottom() + RULE / 2.0;
    ui.painter().hline(ui.max_rect().x_range(), rule, Stroke::new(RULE, LINE));
    // What the tab shows begins at the rule, not a gap below it.
    ui.add_space(RULE - ui.spacing().item_spacing.y);
    strip.inner
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
    let mut job = egui::text::LayoutJob::single_section(text.to_uppercase(), format);
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
    // A tag, as the brand's LIVE: the state's colour filled, with Ink on it.
    let (fill, ink, edge) =
        if solid { (color, ink_on(color), color) } else { (Color32::TRANSPARENT, color, Color32::TRANSPARENT) };
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
    /// A plate filled with the colour: E-STOP alone.
    Filled(Color32),
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
    hazard(ui.painter(), ground);
    let place = egui::Rect::from_center_size(ground.center(), size);
    ui.scope_builder(egui::UiBuilder::new().max_rect(place), |ui| {
        icon_button_sensing(ui, true, glyph, label, Look::Filled(RED), size, egui::Sense::click(), TITLE - 1.0)
    })
    .inner
}

/// What stands in the stop button's place while the stop is latched: the same yellow ground and the
/// same red plate, saying so. It is not a button; the way out is the reset beside it.
pub fn stop_latched(ui: &mut egui::Ui, text: &str, ground: egui::Vec2, size: egui::Vec2) -> egui::Response {
    let (ground, response) = ui.allocate_exact_size(ground, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let plate = egui::Rect::from_center_size(ground.center(), size);
    hazard(ui.painter(), ground);
    ui.painter().rect_filled(plate, RADIUS, RED);
    let format =
        egui::TextFormat { font_id: semibold(SMALL), color: ink_on(RED), extra_letter_spacing: TRACKING, ..Default::default() };
    let galley = ui.painter().layout_job(egui::text::LayoutJob::single_section(text.to_string(), format));
    ui.painter().galley(plate.center() - galley.size() / 2.0, galley, ink_on(RED));
    response
}

/// The ground kept round E-STOP: black and yellow stripes, which nothing else in the window has.
pub fn hazard(painter: &egui::Painter, ground: egui::Rect) {
    let p = painter.with_clip_rect(ground.intersect(painter.clip_rect()));
    p.rect_filled(ground, 0.0, Color32::from_rgb(0x14, 0x14, 0x14));
    let (step, width) = (10.0, 5.0);
    let mut x = ground.left() - ground.height();
    while x < ground.right() {
        let stripe = vec![
            egui::pos2(x, ground.bottom()),
            egui::pos2(x + width, ground.bottom()),
            egui::pos2(x + width + ground.height(), ground.top()),
            egui::pos2(x + ground.height(), ground.top()),
        ];
        p.add(egui::Shape::convex_polygon(stripe, AMBER, Stroke::NONE));
        x += step;
    }
}

fn icon_button_sensing(
    ui: &mut egui::Ui,
    enabled: bool,
    _glyph: &str,
    label: &str,
    look: Look,
    min_size: egui::Vec2,
    sense: egui::Sense,
    text_size: f32,
) -> egui::Response {
    // Words only, in Inter Bold capitals, as the brand sets its buttons: a filled plate for the thing to do
    // next and for GO, a 1 px rule round the others. The glyph is kept by the callers and not drawn.
    let painter = ui.painter().clone();
    let words = |ink: Color32| {
        let format = egui::TextFormat {
            font_id: semibold(text_size - 2.0),
            color: ink,
            extra_letter_spacing: TRACKING,
            ..Default::default()
        };
        painter.layout_job(egui::text::LayoutJob::single_section(label.to_uppercase(), format))
    };
    let probe = words(PAPER);
    let want = (probe.size() + egui::vec2(2.0 * PAD, 2.0 * U)).max(min_size).max(egui::vec2(0.0, ui.spacing().interact_size.y));
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(want, sense);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
        if ui.is_rect_visible(rect) {
            let hot = enabled && response.hovered();
            let down = enabled && response.is_pointer_button_down_on();
            let (fill, edge, ink) = match look {
                _ if !enabled => (Color32::TRANSPARENT, GREY_800, GREY_600),
                Look::Solid(c) | Look::Filled(c) => (if down { mix(c, INK, 0.2) } else { c }, Color32::TRANSPARENT, ink_on(c)),
                Look::Outline(c) => (if down { with_alpha(c, 40) } else { Color32::TRANSPARENT }, c, c),
                Look::Plain => (if down { GREY_800 } else { Color32::TRANSPARENT }, if hot { PAPER } else { GREY_800 }, PAPER),
                Look::Quiet => {
                    (if down { GREY_800 } else { Color32::TRANSPARENT }, Color32::TRANSPARENT, if hot { PAPER } else { GREY_400 })
                }
            };
            let p = ui.painter();
            p.rect(rect, 0.0, fill, Stroke::new(RULE, edge), StrokeKind::Inside);
            let galley = words(ink);
            p.galley(rect.center() - galley.size() / 2.0, galley, ink);
            if response.has_focus() {
                p.rect_stroke(rect.expand(2.0), 0.0, Stroke::new(RULE, PAPER), StrokeKind::Inside);
            }
        }
        response
    })
    .inner
}

/// A small square in a colour with a figure on it: a camera's number, wherever the camera is named.
pub fn number_tag(painter: &egui::Painter, rect: egui::Rect, number: usize, color: Color32) {
    painter.rect_stroke(rect.shrink(0.5), RADIUS, Stroke::new(RULE, color), StrokeKind::Inside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("{number}"), semibold(SMALL), color);
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
