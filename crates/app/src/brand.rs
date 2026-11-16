//! The Motion Choreographer mark and lockup, as the window draws them, and the app's icon.
//! The pictures are the brand kit's own (brand/logo, brand/icon), bundled with the app.

use egui::{Context, TextureHandle, TextureOptions};

/// The product's name, as the brand writes it.
pub const NAME: &str = "Motion Choreographer";
/// The line that goes under the name.
pub const DESCRIPTOR: &str = "PTZ and camera animation for live production";

const MARK: &[u8] = include_bytes!("../assets/brand/mark-reversed-128.png");
const LOCKUP: &[u8] = include_bytes!("../assets/brand/lockup-reversed-256.png");
const ICON: &[u8] = include_bytes!("../assets/brand/app-icon-256.png");

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

fn texture(ctx: &Context, name: &'static str, bytes: &'static [u8]) -> Option<TextureHandle> {
    let id = egui::Id::new(("brand", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let t = ctx.load_texture(name, decode(bytes)?, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// The mark: a keyframe held in an M, for dark grounds.
pub fn mark(ctx: &Context) -> Option<TextureHandle> {
    texture(ctx, "brand-mark", MARK)
}

/// The primary lockup, mark and name on two lines, for dark grounds.
pub fn lockup(ctx: &Context) -> Option<TextureHandle> {
    texture(ctx, "brand-lockup", LOCKUP)
}

/// The app icon, for the window.
pub fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(ICON).ok()?.to_rgba8();
    Some(egui::IconData { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

/// Draw the mark `height` high at the cursor. A picture that a test, and a screen reader, find by the name.
pub fn mark_widget(ui: &mut egui::Ui, height: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(height, height), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, NAME));
    if let Some(t) = mark(ui.ctx()) {
        ui.painter().image(
            t.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    response
}
