//! Native clipboard commands shared by Timeline and Live edit.

use crate::session::Session;
use crate::timeline::KeyRef;
use egui::{Context, Event, Key, ViewportCommand};
use ptz_core::clipboard::{self, Clipboard, Contents};
use ptz_core::song::{ClipBody, Song};
use serde::{Deserialize, Serialize};

const FORMAT: &str = "motion_choreographer_timeline";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    format: String,
    selection: Clipboard,
    media_scope: Option<String>,
}

/// An identity, not a credential. It prevents a reused media slug on another
/// connection or a refreshed library from silently referring to another file.
pub fn media_scope() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{}-{time}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn encode(snapshot: Clipboard, s: &Session) -> Result<String, clipboard::Error> {
    snapshot.validate()?;
    let media_scope = (!snapshot.media_ids().is_empty()).then(|| s.clipboard_media_scope.clone());
    let text = serde_json::to_string(&Payload { format: FORMAT.into(), selection: snapshot, media_scope })
        .map_err(|_| clipboard::Error::BadFormat)?;
    if text.len() > clipboard::MAX_JSON_BYTES {
        return Err(clipboard::Error::TooLarge);
    }
    Ok(text)
}

fn decode(text: &str, s: &Session) -> Result<Clipboard, String> {
    if text.len() > clipboard::MAX_JSON_BYTES {
        return Err(clipboard::Error::TooLarge.to_string());
    }
    let payload: Payload = serde_json::from_str(text).map_err(|_| clipboard::Error::BadFormat.to_string())?;
    if payload.format != FORMAT {
        return Err(clipboard::Error::BadFormat.to_string());
    }
    payload.selection.validate().map_err(|why| why.to_string())?;
    if !payload.selection.media_ids().is_empty() && payload.media_scope.as_ref() != Some(&s.clipboard_media_scope) {
        return Err("The media library or server connection has changed. Copy the media clips again before pasting".into());
    }
    Ok(payload.selection)
}

fn busy(s: &Session) -> bool {
    s.timeline.is_dragging() || s.graph.is_dragging()
}

pub fn can_copy(s: &Session) -> bool {
    s.song.is_some() && !busy(s) && (!s.timeline.selection.is_empty() || !s.timeline.key_selection.is_empty())
}

pub fn paste_unavailable(s: &Session) -> Option<String> {
    if s.song.is_none() {
        Some("Open a song to paste into".into())
    } else if busy(s) {
        Some("Finish the current drag before pasting".into())
    } else {
        s.can_edit().err()
    }
}

/// Request the system clipboard instead of pasting an older, private copy.
pub fn request_paste(ctx: &Context, s: &mut Session) {
    if let Some(why) = paste_unavailable(s) {
        s.toast(why, true);
    } else {
        ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
    }
}

pub fn copy(ctx: &Context, s: &mut Session) {
    if !can_copy(s) {
        return;
    }
    let song = s.song.as_ref().unwrap();
    let snapshot = if s.timeline.key_selection.is_empty() {
        clipboard::capture(song, s.project.rate, &s.selected_ids())
    } else {
        let keys = s.timeline.key_selection.iter().cloned().collect::<Vec<_>>();
        clipboard::capture_key_selection(song, s.project.rate, &keys)
    };
    match snapshot.and_then(|snapshot| encode(snapshot, s)) {
        Ok(text) => ctx.copy_text(text),
        Err(why) => s.toast(why.to_string(), true),
    }
}

fn pasted(s: &Session, text: &str) -> Result<(Song, Vec<String>, Vec<KeyRef>), String> {
    if let Some(why) = paste_unavailable(s) {
        return Err(why);
    }
    let song = s.song.as_ref().ok_or("Open a song to paste into")?;
    let snapshot = decode(text, s)?;
    for media in snapshot.media_ids() {
        if !s.media.iter().any(|item| item.id == media) {
            return Err("A copied media file is not available on this server. Import it before pasting".into());
        }
    }
    let at = s.playhead().round() as i64;
    // Selected individual keys can be inserted into an existing keys clip.
    // Outside that clip they become a new clip, on their original camera.
    if let Contents::Keys { camera, .. } = &snapshot.contents {
        let key_parents = s.timeline.key_selection.iter().map(|key| &key.0).collect::<std::collections::BTreeSet<_>>();
        let target_id = if key_parents.len() == 1 {
            key_parents.iter().next().copied()
        } else if s.timeline.selection.len() == 1 {
            s.timeline.selection.iter().next()
        } else {
            None
        };
        if let Some(target) = target_id.and_then(|id| song.motion_clip(id)).filter(|clip| {
            clip.camera == *camera
                && matches!(clip.body, ClipBody::Keys { .. })
                && (clip.start_frame..=clip.end_frame()).contains(&at)
        }) {
            let (next, selected) =
                clipboard::paste_keys(song, &s.project, &snapshot, &target.id, at).map_err(|why| why.to_string())?;
            let keys = selected.into_iter().map(|(axis, index)| (target.id.clone(), axis, index)).collect();
            return Ok((next, vec![target.id.clone()], keys));
        }
    }
    let (next, ids) = clipboard::paste(song, &s.project, &snapshot, at).map_err(|why| why.to_string())?;
    Ok((next, ids, Vec::new()))
}

pub fn paste(ctx: &Context, s: &mut Session, text: &str) {
    match pasted(s, text) {
        Ok((next, ids, keys)) => {
            // An old gesture must never commit its earlier song over this edit.
            s.timeline.cancel();
            s.graph.cancel();
            s.commit_edit(next);
            s.timeline.selection = ids.into_iter().collect();
            s.timeline.key_selection = keys.into_iter().collect();
            ctx.request_repaint();
        }
        Err(why) => s.toast(why, true),
    }
}

/// eframe emits Copy/Paste events for native shortcuts. Some hosts also send
/// key events; process each command once and leave text editing to egui.
pub fn shortcuts(ctx: &Context, s: &mut Session) {
    let events = ctx.input(|input| input.events.clone());
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            !matches!(event, Event::Copy | Event::Paste(_))
                && !matches!(event, Event::Key { key: Key::C | Key::V, pressed: true, modifiers, .. }
                    if modifiers.command || modifiers.ctrl)
        });
    });
    let native_copy = events.iter().any(|event| matches!(event, Event::Copy));
    let native_paste = events.iter().any(|event| matches!(event, Event::Paste(_)));
    let (mut copied, mut pasted) = (false, false);
    for event in events {
        match event {
            Event::Copy if !copied => {
                copy(ctx, s);
                copied = true;
            }
            Event::Paste(text) if !pasted => {
                paste(ctx, s, &text);
                pasted = true;
            }
            Event::Key { key: Key::C, pressed: true, modifiers, .. }
                if (modifiers.command || modifiers.ctrl) && !native_copy && !copied =>
            {
                copy(ctx, s);
                copied = true;
            }
            Event::Key { key: Key::V, pressed: true, modifiers, .. }
                if (modifiers.command || modifiers.ctrl) && !native_paste && !pasted =>
            {
                request_paste(ctx, s);
                pasted = true;
            }
            _ => {}
        }
    }
}
