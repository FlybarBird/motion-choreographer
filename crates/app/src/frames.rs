//! Proxy frames of reference video: asked for through the link, kept as
//! textures while they are in use, and let go least recently used first.

use crate::link::{FrameKey, Link};
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Monitor-sized pictures kept. Each is about half a megabyte on the graphics card.
const FULL_KEPT: usize = 72;
/// Filmstrip pictures kept. Each is about 32 kilobytes.
const THUMBS_KEPT: usize = 600;
/// A request unanswered for this long was crowded out, and may be asked again.
const ASK_AGAIN: Duration = Duration::from_millis(1500);
/// A picture the server did not have is not asked for again for this long.
const MISSING_FOR: Duration = Duration::from_secs(5);

enum Slot {
    Asked(Instant),
    Missing(Instant),
    /// Decoded, waiting for a frame to be drawn so it can become a texture.
    Image(ColorImage),
    Ready(TextureHandle),
}

#[derive(Default)]
pub struct Frames {
    slots: HashMap<FrameKey, (Slot, u64)>,
    clock: u64,
}

pub fn key(media: &str, frame: i64, thumb: bool) -> FrameKey {
    FrameKey { media: media.to_string(), frame, thumb }
}

impl Frames {
    /// Make sure the picture is on its way. Returns true when a request was sent.
    pub fn want(&mut self, link: &Link, key: &FrameKey) -> bool {
        self.clock += 1;
        let now = Instant::now();
        let ask = match self.slots.get_mut(key) {
            None => true,
            Some((Slot::Asked(at), _)) => now.duration_since(*at) > ASK_AGAIN,
            Some((Slot::Missing(at), _)) => now.duration_since(*at) > MISSING_FOR,
            Some((_, used)) => {
                *used = self.clock;
                false
            }
        };
        if ask {
            self.slots.insert(key.clone(), (Slot::Asked(now), self.clock));
            link.want_frame(key.clone());
        }
        ask
    }

    /// A reply from the link.
    pub fn store(&mut self, key: FrameKey, image: Option<ColorImage>) {
        self.clock += 1;
        let slot = match image {
            Some(image) => Slot::Image(image),
            None => Slot::Missing(Instant::now()),
        };
        self.slots.insert(key, (slot, self.clock));
        self.evict();
    }

    /// The picture as a texture, if it has arrived.
    pub fn texture(&mut self, ctx: &egui::Context, key: &FrameKey) -> Option<TextureHandle> {
        self.clock += 1;
        let (slot, used) = self.slots.get_mut(key)?;
        *used = self.clock;
        if let Slot::Image(image) = slot {
            let name = format!("frame:{}:{}:{}", key.media, key.frame, key.thumb);
            *slot = Slot::Ready(ctx.load_texture(name, std::mem::take(image), TextureOptions::LINEAR));
        }
        match slot {
            Slot::Ready(texture) => Some(texture.clone()),
            _ => None,
        }
    }

    /// The texture, if one has already been made. Does not count as a use.
    pub fn ready(&self, key: &FrameKey) -> Option<&TextureHandle> {
        match self.slots.get(key) {
            Some((Slot::Ready(texture), _)) => Some(texture),
            _ => None,
        }
    }

    /// Forget every picture of files that are no longer in `keep`.
    pub fn retain_media(&mut self, keep: impl Fn(&str) -> bool) {
        self.slots.retain(|k, _| keep(&k.media));
    }

    #[cfg(test)]
    pub fn asked(&self) -> Vec<FrameKey> {
        let mut keys: Vec<FrameKey> =
            self.slots.iter().filter(|(_, (slot, _))| matches!(slot, Slot::Asked(_))).map(|(k, _)| k.clone()).collect();
        keys.sort_by_key(|k| (k.thumb, k.frame));
        keys
    }

    fn evict(&mut self) {
        for (thumb, kept) in [(false, FULL_KEPT), (true, THUMBS_KEPT)] {
            let mut held: Vec<(u64, FrameKey)> = self
                .slots
                .iter()
                .filter(|(k, (slot, _))| k.thumb == thumb && matches!(slot, Slot::Image(_) | Slot::Ready(_)))
                .map(|(k, (_, used))| (*used, k.clone()))
                .collect();
            if held.len() <= kept {
                continue;
            }
            held.sort_by_key(|(used, _)| *used);
            for (_, key) in held.drain(..held.len() - kept) {
                self.slots.remove(&key);
            }
        }
        let now = Instant::now();
        self.slots.retain(|_, (slot, _)| match slot {
            Slot::Asked(at) => now.duration_since(*at) < Duration::from_secs(30),
            Slot::Missing(at) => now.duration_since(*at) < Duration::from_secs(30),
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> ColorImage {
        ColorImage::filled([4, 2], egui::Color32::WHITE)
    }

    #[test]
    fn a_picture_is_asked_for_once_and_the_least_recently_used_goes_first() {
        let (link, _cmds, _events) = Link::detached();
        let mut frames = Frames::default();
        let first = key("clip", 0, false);
        assert!(frames.want(&link, &first));
        assert!(!frames.want(&link, &first), "already on its way");
        assert_eq!(frames.asked(), vec![first.clone()]);

        frames.store(first.clone(), Some(picture()));
        assert!(!frames.want(&link, &first), "already here");
        let ctx = egui::Context::default();
        assert!(frames.ready(&first).is_none(), "not a texture until it is drawn");
        assert!(frames.texture(&ctx, &first).is_some());
        assert!(frames.ready(&first).is_some());

        // Fill past the limit: the first, untouched since, is the one let go.
        for n in 1..=FULL_KEPT as i64 {
            frames.store(key("clip", n, false), Some(picture()));
        }
        assert!(frames.texture(&ctx, &first).is_none());
        assert!(frames.texture(&ctx, &key("clip", 1, false)).is_some());
        // Thumbnails have their own allowance.
        frames.store(key("clip", 0, true), Some(picture()));
        assert!(frames.texture(&ctx, &key("clip", 0, true)).is_some());

        // A picture the server does not have is not asked for again straight away.
        let gone = key("clip", 9000, false);
        frames.want(&link, &gone);
        frames.store(gone.clone(), None);
        assert!(!frames.want(&link, &gone));

        frames.retain_media(|id| id != "clip");
        assert!(frames.ready(&key("clip", 1, false)).is_none());
    }
}
