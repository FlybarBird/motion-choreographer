//! The window as a picture, for tests that have to see what was drawn and not
//! only where things were laid out. A small software rasteriser of egui's own
//! triangles: text and flat fills, which is all the bars are made of. Pictures
//! the app uploads (stage views, video frames, thumbnails) are drawn from the pixels it gave.

use super::Rig;
use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, Pos2, Rect};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Every picture the window has uploaded, as the pixels it was given. It stands where a renderer
/// would in the test harness, so nothing uploaded is missed.
#[derive(Clone, Default)]
pub struct Textures(Arc<Mutex<HashMap<egui::TextureId, egui::ColorImage>>>);

impl egui_kittest::TestRenderer for Textures {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut kept = self.0.lock().expect("the pictures are not poisoned");
        let mut delta = std::mem::take(delta);
        let changes = delta.set.iter().flat_map(|(id, changes)| changes.iter().map(move |change| (*id, change)));
        for (id, change) in changes {
            let egui::ImageData::Color(image) = &change.image;
            match (change.pos, kept.get_mut(&id)) {
                (Some([left, top]), Some(whole)) => {
                    for y in 0..image.size[1] {
                        let to = (top + y) * whole.size[0] + left;
                        whole.pixels[to..to + image.size[0]]
                            .copy_from_slice(&image.pixels[y * image.size[0]..(y + 1) * image.size[0]]);
                    }
                }
                _ => {
                    kept.insert(id, image.as_ref().clone());
                }
            }
        }
        for id in &delta.free {
            kept.remove(id);
        }
        // Everything in it has been taken: it may be dropped without complaint.
        delta.clear();
    }
}

pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Pixels per point of the window.
    pub scale: f32,
    /// Row by row from the top left, opaque.
    pub pixels: Vec<[f32; 3]>,
}

fn channels(c: Color32) -> [f32; 4] {
    // egui's colours are premultiplied, and it blends them as they are.
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

/// The font atlas at a texture coordinate, between its four nearest texels.
fn sample(atlas: &egui::ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [w, h] = atlas.size;
    let (x, y) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |dx: f32, dy: f32| {
        let xi = (x0 + dx).clamp(0.0, w as f32 - 1.0) as usize;
        let yi = (y0 + dy).clamp(0.0, h as f32 - 1.0) as usize;
        channels(atlas.pixels[yi * w + xi])
    };
    let (a, b, c, d) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0));
    std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

impl Rig {
    /// What the last frame drew.
    pub fn picture(&self) -> Picture {
        Picture::of(&self.harness.ctx, self.harness.output(), &self.textures)
    }
}

impl Picture {
    /// What a frame drew, from the shapes it put out and the pictures uploaded so far.
    pub fn of(ctx: &egui::Context, output: &egui::FullOutput, textures: &Textures) -> Picture {
        let scale = ctx.pixels_per_point();
        let size = ctx.content_rect().size() * scale;
        let (width, height) = (size.x.round() as usize, size.y.round() as usize);
        let atlas = ctx.fonts(|f| f.image());
        let uploaded = textures.0.lock().expect("the pictures are not poisoned");
        let mut pixels = vec![[0.0f32; 3]; width * height];
        for ClippedPrimitive { clip_rect, primitive } in ctx.tessellate(output.shapes.clone(), scale) {
            let Primitive::Mesh(mesh) = primitive else { continue };
            // Text and flat fills are both cut from the font atlas; anything else is a picture.
            let texture =
                if mesh.texture_id == egui::TextureId::default() { Some(&atlas) } else { uploaded.get(&mesh.texture_id) };
            let clip =
                Rect::from_min_max((clip_rect.min.to_vec2() * scale).to_pos2(), (clip_rect.max.to_vec2() * scale).to_pos2())
                    .intersect(Rect::from_min_size(Pos2::ZERO, size));
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let v = triangle.map(|i| mesh.vertices[i as usize]);
                let p = v.map(|v| (v.pos.to_vec2() * scale).to_pos2());
                let area = (p[1] - p[0]).x * (p[2] - p[0]).y - (p[1] - p[0]).y * (p[2] - p[0]).x;
                if area.abs() < 1e-9 {
                    continue;
                }
                let bounds = Rect::from_points(&p).intersect(clip);
                if bounds.is_negative() {
                    continue;
                }
                let colors = v.map(|v| channels(v.color));
                for y in bounds.top().floor() as usize..(bounds.bottom().ceil() as usize).min(height) {
                    for x in bounds.left().floor() as usize..(bounds.right().ceil() as usize).min(width) {
                        let at = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        if !clip.contains(at) {
                            continue;
                        }
                        // The share of each corner at this pixel's centre.
                        let edge = |a: Pos2, b: Pos2| ((b - a).x * (at - a).y - (b - a).y * (at - a).x) / area;
                        let w = [edge(p[1], p[2]), edge(p[2], p[0]), edge(p[0], p[1])];
                        if w.iter().any(|share| *share < -1e-4) {
                            continue;
                        }
                        let mix = |f: &dyn Fn(usize) -> f32| w[0] * f(0) + w[1] * f(1) + w[2] * f(2);
                        let mut ink: [f32; 4] = std::array::from_fn(|c| mix(&|i| colors[i][c]));
                        if let Some(texture) = texture {
                            let texel = sample(texture, mix(&|i| v[i].uv.x), mix(&|i| v[i].uv.y));
                            ink = std::array::from_fn(|c| ink[c] * texel[c]);
                        }
                        let under = &mut pixels[y * width + x];
                        for c in 0..3 {
                            under[c] = ink[c] + under[c] * (1.0 - ink[3]);
                        }
                    }
                }
            }
        }
        Picture { width, height, scale, pixels }
    }

    pub fn at(&self, point: Pos2) -> Color32 {
        let x = ((point.x * self.scale) as usize).min(self.width - 1);
        let y = ((point.y * self.scale) as usize).min(self.height - 1);
        let [r, g, b] = self.pixels[y * self.width + x].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        Color32::from_rgb(r, g, b)
    }

    /// The share of the pixels in `rect` that are within a little of `color`.
    pub fn share_of(&self, rect: Rect, color: Color32) -> f32 {
        let near = |a: Color32| (0..3).all(|c| (a.to_array()[c] as i32 - color.to_array()[c] as i32).abs() <= 6);
        let (mut all, mut same) = (0, 0);
        let mut y = rect.top() + 0.5 / self.scale;
        while y < rect.bottom() {
            let mut x = rect.left() + 0.5 / self.scale;
            while x < rect.right() {
                all += 1;
                same += near(self.at(egui::pos2(x, y))) as usize;
                x += 1.0 / self.scale;
            }
            y += 1.0 / self.scale;
        }
        same as f32 / all.max(1) as f32
    }

    /// Save the part of the picture inside `rect` (in points) as a PNG.
    pub fn save(&self, rect: Rect, path: &std::path::Path) {
        let rect =
            rect.intersect(Rect::from_min_size(Pos2::ZERO, egui::vec2(self.width as f32, self.height as f32) / self.scale));
        let (left, top) = ((rect.left() * self.scale) as u32, (rect.top() * self.scale) as u32);
        let (w, h) = ((rect.width() * self.scale) as u32, (rect.height() * self.scale) as u32);
        let image = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb(
                self.pixels[(top + y) as usize * self.width + (left + x) as usize]
                    .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
            )
        });
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("the folder for pictures");
        }
        image.save(path).expect("the picture is written");
    }
}
