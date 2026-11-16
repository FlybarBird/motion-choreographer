//! The Motion Choreographer mark and lockup, drawn from the brand kit's own files (`brand/`), so that
//! the wordmark is the kit's outlines and not type set here.

use egui::{Color32, ColorImage, Rect, Response, Sense, TextureHandle, TextureOptions, Ui};

/// The name of the product, wherever it is written.
pub const NAME: &str = "Motion Choreographer";

/// The mark with heavier stems, which the kit asks for at 24 px and below. White with the red keyframe.
const MARK_SMALL: &[u8] =
    include_bytes!("../../../brand/logo/mark-small/png/motion-choreographer-mark-small-color-reversed-48px.png");
/// The primary lockup: the mark and the name on two lines. White with the red keyframe.
const LOCKUP: &[u8] =
    include_bytes!("../../../brand/logo/lockup-horizontal/png/motion-choreographer-lockup-horizontal-color-reversed-256px.png");
/// The app icon as a full square, for the window where the system asks for one.
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../../../brand/icon/png/app-icon-256.png");

fn decode(png: &[u8]) -> ColorImage {
    let picture = image::load_from_memory(png).expect("the brand kit's pictures are PNG files").to_rgba8();
    ColorImage::from_rgba_unmultiplied([picture.width() as usize, picture.height() as usize], picture.as_raw())
}

/// A picture of the kit as a texture, made the first time it is drawn.
fn texture(ctx: &egui::Context, name: &'static str, png: &'static [u8]) -> TextureHandle {
    let id = egui::Id::new(("brand", name));
    if let Some(made) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return made;
    }
    let smooth = TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR };
    let made = ctx.load_texture(name, decode(png), smooth);
    ctx.data_mut(|d| d.insert_temp(id, made.clone()));
    made
}

fn show(ui: &mut Ui, name: &'static str, png: &'static [u8], height: f32) -> Response {
    let texture = texture(ui.ctx(), name, png);
    let [w, h] = texture.size();
    let size = egui::vec2(height * w as f32 / h as f32, height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, NAME));
    if ui.is_rect_visible(rect) {
        let whole = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(texture.id(), rect, whole, Color32::WHITE);
    }
    response
}

/// The mark alone, for where the name is already on the screen.
pub fn mark(ui: &mut Ui, height: f32) -> Response {
    show(ui, "mark-small", MARK_SMALL, height)
}

/// The mark and the name, flush left.
pub fn lockup(ui: &mut Ui, height: f32) -> Response {
    show(ui, "lockup", LOCKUP, height)
}

/// The icon of the window.
#[cfg(not(target_os = "macos"))]
pub fn window_icon() -> egui::IconData {
    let picture = decode(ICON);
    egui::IconData { width: picture.width() as u32, height: picture.height() as u32, rgba: picture.as_raw().to_vec() }
}
