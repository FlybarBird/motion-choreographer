//! Native clipboard events through the real timeline, including keyboard ownership.

use super::*;
use egui::{Event as Input, Key, Modifiers, OutputCommand};
use egui_kittest::kittest::{NodeT as _, Queryable as _};

fn selected_move() -> (Rig, String) {
    let (song, id) = ptz_core::edit::insert_marked_move(
        &Song::new("s", "Test Song", "01:00:00:00", 1500),
        &Project::default(),
        &presets()[1],
        100,
        200,
    )
    .unwrap();
    let mut r = rig(song);
    r.harness.state_mut().session.timeline.selection.insert(id.clone());
    (r, id)
}

fn copied(r: &mut Rig) -> String {
    r.harness.event(Input::Copy);
    r.harness.step();
    let copies: Vec<_> = r
        .harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(copies.len(), 1, "one native clipboard write");
    copies[0].clone()
}

fn at(r: &mut Rig, frame: i64) {
    let s = &mut r.harness.state_mut().session;
    s.status.frame = frame;
    s.status.frame_f = frame as f64;
}

fn paste(r: &mut Rig, text: &str) {
    r.harness.event(Input::Paste(text.to_string()));
    r.harness.run_steps(2);
}

#[test]
fn native_clipboard_pastes_at_the_playhead_once_and_undo_redo_restore_the_song() {
    let (mut r, id) = selected_move();
    let before = r.song().clone();
    // Native hosts normally emit only Copy. A host that also emits the key must not copy twice.
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::C);
    let text = copied(&mut r);
    assert!(r.puts().is_empty(), "copy is read-only");
    at(&mut r, 600);
    r.harness.key_press_modifiers(Modifiers::CTRL, Key::V);
    paste(&mut r, &text);
    assert_eq!(r.song().motion.len(), 2);
    let selected = r.harness.state().session.selected_ids();
    assert_eq!(selected.len(), 1);
    assert_ne!(selected[0], id);
    let new = r.song().motion_clip(&selected[0]).unwrap();
    assert_eq!((new.start_frame, new.duration_frames), (600, 100));
    assert!(r.harness.state().session.timeline.key_selection.is_empty());
    assert_eq!(r.puts().len(), 1, "one paste is one server edit");
    assert_eq!(r.harness.state().session.undo.len(), 1);
    let after = r.song().clone();
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(*r.song(), before);
    r.harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(*r.song(), after);
}

#[test]
fn clipboard_snapshot_survives_source_deletion_and_pastes_into_an_empty_song() {
    let (mut source, id) = selected_move();
    let text = copied(&mut source);
    source.harness.state_mut().session.edit(|song, project| ptz_core::edit::delete(song, project, &[id]));
    assert!(source.song().motion.is_empty());
    let mut target = rig(Song::new("other", "Other Song", "02:00:00:00", 1500));
    at(&mut target, 450);
    paste(&mut target, &text);
    assert_eq!(target.song().motion.len(), 1);
    assert_eq!(target.song().motion[0].start_frame, 450);
    assert_eq!(target.puts().len(), 1);
}

#[test]
fn copy_needs_no_lease_but_paste_obeys_lease_connection_and_live_edit_permissions() {
    let (mut r, _) = selected_move();
    let before = r.song().clone();
    r.harness.state_mut().session.status.you_hold_lease = false;
    let text = copied(&mut r);
    at(&mut r, 600);
    paste(&mut r, &text);
    assert_eq!(*r.song(), before);
    assert!(r.puts().is_empty());

    r.harness.state_mut().session.status.you_hold_lease = true;
    r.harness.state_mut().session.lost = Some("Disconnected".into());
    paste(&mut r, &text);
    assert_eq!(*r.song(), before);
    assert!(r.puts().is_empty());

    r.harness.state_mut().session.lost = None;
    r.harness.state_mut().session.status.transport.output = Output::Armed;
    paste(&mut r, &text);
    assert_eq!(*r.song(), before);
    assert!(r.puts().is_empty());

    r.harness.state_mut().show_tab(Tab::LiveEdit);
    r.harness.run_steps(2);
    paste(&mut r, &text);
    assert_eq!(*r.song(), before, "opening Live edit is not confirmation");
    r.harness.state_mut().session.status.live_edit = true;
    r.harness.run_steps(2);
    paste(&mut r, &text);
    assert_eq!(r.song().motion.len(), 2);
    assert_eq!(r.puts().len(), 1);
}

#[test]
fn overlap_or_unrelated_clipboard_text_never_partially_edits_the_song() {
    let (mut r, _) = selected_move();
    let text = copied(&mut r);
    let before = r.song().clone();
    at(&mut r, 150);
    paste(&mut r, &text);
    assert_eq!(*r.song(), before);
    assert!(r.puts().is_empty());
    assert!(r.harness.state().session.undo.is_empty());
    at(&mut r, 600);
    paste(&mut r, "ordinary clipboard text");
    assert_eq!(*r.song(), before);
    assert!(r.puts().is_empty());
}

#[test]
fn text_fields_dialogs_and_hidden_timeline_keep_clipboard_commands() {
    let (mut r, _) = selected_move();
    let text = copied(&mut r);
    let before = r.song().clone();
    at(&mut r, 600);
    let name = r
        .harness
        .query_all(egui_kittest::kittest::by().role(egui::accesskit::Role::TextInput))
        .find(|node| node.accesskit_node().value().as_deref() == Some("Test Song"))
        .unwrap()
        .rect();
    r.click(name.center());
    assert!(r.harness.ctx.text_edit_focused());
    paste(&mut r, "hello");
    assert_eq!(r.song().motion, before.motion, "text paste must not alter timeline clips");
    r.harness.key_press(Key::Escape);
    r.harness.run_steps(2);
    r.click(r.lane_point(Lane::Audio, 600.0));

    r.press("New");
    paste(&mut r, &text);
    assert_eq!(r.song().motion, before.motion, "a dialog owns its paste");
    r.press("Cancel");

    r.harness.state_mut().session.settings = Some(Page::Cameras);
    paste(&mut r, &text);
    assert_eq!(r.song().motion, before.motion);
    r.harness.state_mut().session.settings = None;
    r.harness.state_mut().show_tab(Tab::Log);
    r.harness.run_steps(2);
    paste(&mut r, &text);
    assert_eq!(r.song().motion, before.motion, "hidden timeline cannot be pasted into");
}

#[test]
fn the_toolbar_paste_button_requests_the_native_clipboard_without_a_selection() {
    let mut r = rig(Song::new("empty", "Empty", "01:00:00:00", 1500));
    r.harness.get_by_label("Paste").click();
    r.harness.step();
    assert!(r
        .harness
        .output()
        .viewport_output
        .values()
        .any(|output| { output.commands.iter().any(|command| matches!(command, egui::ViewportCommand::RequestPaste)) }));
    assert!(r.puts().is_empty(), "the native Paste response supplies the payload");
}

#[test]
fn clipboard_and_enter_in_one_frame_still_belong_to_the_text_field() {
    for copy in [false, true] {
        let (mut r, _) = selected_move();
        let text = copied(&mut r);
        let before = r.song().motion.clone();
        at(&mut r, 600);
        let name = r
            .harness
            .query_all(egui_kittest::kittest::by().role(egui::accesskit::Role::TextInput))
            .find(|node| node.accesskit_node().value().as_deref() == Some("Test Song"))
            .unwrap()
            .rect();
        r.click(name.center());
        r.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        r.harness.run_steps(1);
        r.harness.event(if copy { Input::Copy } else { Input::Paste(text) });
        r.harness.key_press(Key::Enter);
        r.harness.step();
        assert_eq!(r.song().motion, before);
        if copy {
            assert!(r.harness.output().platform_output.commands.iter().all(|command| {
                !matches!(command, OutputCommand::CopyText(text) if text.starts_with("{\"format\":\"motion_choreographer_timeline\""))
            }), "copying text must not be overwritten by the timeline after Enter releases focus");
        }
    }
}

#[test]
fn keys_selected_in_the_graph_paste_into_their_clip_without_a_clip_selection() {
    let (song, id) = keys_song();
    let before = song.clone();
    let mut r = rig(song);
    at(&mut r, 200);
    r.harness.state_mut().show_tab(Tab::Graph);
    r.harness.run_steps(3);
    let clip = r.song().motion_clip(&id).unwrap();
    let axis = ptz_core::Axis::Pan;
    let point = r.plot().key_pos(clip, axis, &clip.keys(axis)[1]);
    r.click(point);
    assert!(r.harness.state().session.timeline.selection.is_empty());
    assert!(r.harness.state().session.timeline.key_selection.contains(&(id.clone(), axis, 1)));
    let text = copied(&mut r);
    at(&mut r, 150);
    paste(&mut r, &text);
    assert_eq!(r.song().motion.len(), 1);
    let keys = r.song().motion_clip(&id).unwrap().keys(axis);
    assert_eq!(keys.len(), 4);
    assert_eq!((keys[1].frame, keys[1].value), (50, 30.0));
    assert_eq!(r.puts().len(), 1);
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(*r.song(), before);
}

/// A mixed selection proves that refusing its media reference also refuses the
/// otherwise valid motion and marker, rather than pasting just those items.
fn selected_media() -> Rig {
    use ptz_core::song::{Marker, MediaClip, Track};
    let (mut song, motion) = ptz_core::edit::insert_marked_move(
        &Song::new("media-source", "Media Source", "01:00:00:00", 1500),
        &Project::default(),
        &presets()[1],
        100,
        200,
    )
    .unwrap();
    song.media.push(MediaClip {
        id: "audio".into(),
        media_id: "click".into(),
        track: Track::Audio,
        start_frame: 100,
        in_frame: 10,
        out_frame: 210,
        media_frames: 310,
        name: "Trimmed click".into(),
        muted: true,
    });
    song.markers.push(Marker { id: "mark".into(), frame: 160, name: "Beat".into() });
    let mut r = rig(song);
    r.events.send(Event::Media(vec![click_media()])).unwrap();
    r.harness.run_steps(2);
    r.harness.state_mut().session.timeline.selection = [motion, "audio".into(), "mark".into()].into_iter().collect();
    let _ = r.cmds.try_iter().count();
    r
}

fn assert_media_paste_refused(r: &Rig, before: &Song, reason: &str) {
    assert_eq!(r.song(), before, "a bad media reference must refuse the entire mixed selection");
    assert!(r.puts().is_empty(), "a refused paste sends no song edit");
    assert!(r.harness.state().session.undo.is_empty(), "a refused paste adds no undo step");
    assert!(r.toasts().iter().any(|toast| toast.contains(reason)), "{:?}", r.toasts());
}

#[test]
fn media_clipboard_pastes_within_its_session_and_keeps_source_trims_and_mute() {
    let mut r = selected_media();
    let before = r.song().clone();
    let text = copied(&mut r);
    assert!(r.puts().is_empty(), "copy is read-only");
    at(&mut r, 600);
    paste(&mut r, &text);
    assert_eq!((r.song().motion.len(), r.song().media.len(), r.song().markers.len()), (2, 2, 2));
    let new = &r.song().media[1];
    assert_eq!((new.media_id.as_str(), new.start_frame, new.in_frame, new.out_frame), ("click", 600, 10, 210));
    assert!(new.muted);
    assert_eq!(new.media_frames, 310);
    assert_eq!(r.song().motion[1].start_frame, 600);
    assert_eq!(r.song().markers[1].frame, 660);
    assert_eq!(r.harness.state().session.selected_ids().len(), 3);
    assert_eq!(r.puts().len(), 1, "the complete mixed paste is one edit");
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(*r.song(), before, "undo removes the complete mixed paste");
}

#[test]
fn media_clipboard_scope_survives_opening_another_song_on_the_same_session() {
    let mut r = selected_media();
    let text = copied(&mut r);
    let scope = r.harness.state().session.clipboard_media_scope.clone();
    let destination = Song::new("media-destination", "Media Destination", "02:00:00:00", 1500);
    r.events
        .send(Event::Working(
            Box::new(WorkingSong { song: destination, edit_counter: 0, dirty: false, revision: None, issues: Vec::new() }),
            Why::Open,
        ))
        .unwrap();
    r.harness.run_steps(2);
    assert_eq!(r.harness.state().session.clipboard_media_scope, scope, "changing songs keeps the same media library");
    at(&mut r, 400);
    paste(&mut r, &text);
    assert_eq!(r.song().id, "media-destination");
    assert_eq!((r.song().motion.len(), r.song().media.len(), r.song().markers.len()), (1, 1, 1));
    assert_eq!((r.song().media[0].media_id.as_str(), r.song().media[0].start_frame), ("click", 400));
    assert_eq!(r.song().markers[0].frame, 460);
    assert_eq!(r.puts().len(), 1);
}

#[test]
fn media_clipboard_refuses_an_identical_media_slug_in_another_session() {
    let mut source = selected_media();
    let text = copied(&mut source);
    let mut target = rig(Song::new("other", "Other", "01:00:00:00", 1500));
    target.events.send(Event::Media(vec![click_media()])).unwrap();
    target.harness.run_steps(2);
    assert!(target.harness.state().session.media.iter().any(|media| media.id == "click"));
    assert_ne!(target.harness.state().session.clipboard_media_scope, source.harness.state().session.clipboard_media_scope);
    let before = target.song().clone();
    at(&mut target, 600);
    paste(&mut target, &text);
    assert_media_paste_refused(&target, &before, "media library or server connection has changed");
}

#[test]
fn media_clipboard_refuses_an_old_scope_after_media_refresh_until_copied_again() {
    let mut r = selected_media();
    let text = copied(&mut r);
    let scope = r.harness.state().session.clipboard_media_scope.clone();
    let before = r.song().clone();
    let selection = r.harness.state().session.timeline.selection.clone();
    // Even identical metadata can hide a removed and reimported file under the same slug.
    r.events.send(Event::Media(vec![click_media()])).unwrap();
    r.harness.run_steps(2);
    assert_ne!(r.harness.state().session.clipboard_media_scope, scope);
    at(&mut r, 600);
    paste(&mut r, &text);
    assert_media_paste_refused(&r, &before, "media library or server connection has changed");
    assert_eq!(r.harness.state().session.timeline.selection, selection, "refusal preserves the selection");

    let current = copied(&mut r);
    paste(&mut r, &current);
    assert_eq!((r.song().motion.len(), r.song().media.len(), r.song().markers.len()), (2, 2, 2));
    assert_eq!(r.puts().len(), 1, "copying from the current library makes the reference usable again");
}

#[test]
fn media_clipboard_refuses_missing_media_even_when_the_scope_is_current() {
    let mut r = selected_media();
    // A song can retain a clip whose source file has gone away. Copy it after the
    // refresh so its scope is current, and the missing-reference check must still run.
    r.events.send(Event::Media(Vec::new())).unwrap();
    r.harness.run_steps(2);
    let before = r.song().clone();
    let text = copied(&mut r);
    at(&mut r, 600);
    paste(&mut r, &text);
    assert_media_paste_refused(&r, &before, "copied media file is not available");
}
