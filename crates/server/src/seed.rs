//! First-run content, so a fresh install opens on something that plays.

use crate::rig::seeded_presets;
use crate::storage::Storage;
use ptz_core::edit;
use ptz_core::pose::{Preset, Project};
use ptz_core::song::Song;

fn secs(project: &Project, s: f64) -> i64 {
    (project.rate.fps() * s).round() as i64
}

fn build(
    project: &Project,
    id: &str,
    name: &str,
    start_tc: &str,
    length_s: f64,
    moves: &[(f64, &Preset)],
    markers: &[(f64, &str)],
) -> Song {
    let mut song = Song::new(id, name, start_tc, secs(project, length_s));
    for (at, preset) in moves {
        if let Ok((next, _)) = edit::insert_preset_move(&song, project, preset, secs(project, *at)) {
            song = next;
        }
    }
    for (at, label) in markers {
        if let Ok(next) = edit::add_marker(&song, project, secs(project, *at), label) {
            song = next;
        }
    }
    song
}

pub fn seed(storage: &Storage) -> std::io::Result<()> {
    let project = Project::default();
    let presets = seeded_presets();
    let (wide, lectern, awards) = (&presets[0], &presets[1], &presets[2]);

    let walk_in = build(
        &project,
        "walk-in",
        "Walk-In",
        "01:00:00:00",
        40.0,
        &[(2.0, wide), (12.0, lectern), (24.0, awards)],
        &[(2.0, "Doors"), (12.0, "Host walks")],
    );

    let mut opening = build(
        &project,
        "opening-number",
        "Opening Number",
        "01:10:00:00",
        90.0,
        &[(1.0, wide), (20.0, awards), (45.0, lectern), (70.0, wide)],
        &[(20.0, "Verse"), (45.0, "Chorus"), (70.0, "Button")],
    );
    // One slow, deliberate push with a custom time.
    if let Some(id) = opening.motion.get(1).map(|c| c.id.clone()) {
        if let Ok(next) = edit::set_custom_duration(&opening, &project, &id, secs(&project, 14.0)) {
            opening = next;
        }
    }

    let awards_bed = build(
        &project,
        "awards-bed",
        "Awards Bed",
        "02:00:00:00",
        60.0,
        &[(0.0, awards)],
        &[(30.0, "Winner announced")],
    );

    storage.save_project(&project)?;
    storage.save_presets(&presets)?;
    for song in [&walk_in, &opening, &awards_bed] {
        storage.save_working(song)?;
        storage.save_draft(song)?;
    }
    // Walk-In ships committed, so Arm can be tried on the simulator straight away.
    storage.commit(&walk_in, "installer")?;
    storage.save_current(Some("walk-in"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptz_core::compile::compile;

    #[test]
    fn seeded_songs_are_valid_and_feasible() {
        let dir = std::env::temp_dir().join(format!("ptz-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = Storage::open(&dir).unwrap();
        seed(&st).unwrap();
        let project = st.load_project();
        assert_eq!(st.song_ids().len(), 3);
        for id in st.song_ids() {
            let song = st.load_working(&id).unwrap();
            song.validate(project.rate).unwrap();
            let c = compile(&song, &project);
            assert!(c.issues.is_empty(), "{id}: {:?}", c.issues);
            assert!(!song.motion.is_empty());
        }
        assert_eq!(st.load_working("walk-in").unwrap().motion.len(), 3);
        assert_eq!(st.revisions("walk-in").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
