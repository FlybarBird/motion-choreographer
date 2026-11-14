//! The stage model on the server: taken as a mesh file, kept across restarts, placed with the project.

mod common;

use common::*;
use ptz_core::camera::{Unit, UpAxis};
use ptz_core::transport::Action;
use ptz_scene::mesh::Mesh;

/// A hall 40 by 25 by 9 in the file's units, with a platform in it.
fn hall(scale: f32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.push_cuboid([0.0; 3], [40.0 * scale, 25.0 * scale, 9.0 * scale], [120, 120, 130]);
    mesh.push_cuboid([15.0 * scale, 18.0 * scale, 0.0], [25.0 * scale, 24.0 * scale, 1.0 * scale], [90, 60, 40]);
    mesh
}

#[test]
fn a_model_is_stored_served_and_kept_across_a_restart() {
    let t = TestServer::start("stage-keep");
    let api = t.client("a");
    assert!(api.project().unwrap().stage.show_builtin);
    assert_eq!(refused(api.stage_model(), 404), "No stage model has been imported");

    let file = hall(1.0).to_bytes();
    assert_eq!(
        refused(api.upload_stage_model("hall.obj", Unit::M, UpAxis::Y, &file), 409),
        "Take the edit lease to control the show"
    );
    api.take_lease("Luke", false).unwrap();
    let project = api.upload_stage_model("C:\\venues\\Hall A.obj", Unit::M, UpAxis::Y, &file).unwrap();
    let model = project.stage.model.clone().expect("the project names the model");
    assert_eq!((model.name.as_str(), model.revision, model.triangles), ("Hall A.obj", 1, 24));
    assert_eq!((model.min, model.max), ([0.0; 3], [40.0, 25.0, 9.0]));
    assert_eq!((model.unit, model.up), (Unit::M, UpAxis::Y));
    assert_eq!(model.size_m(), [40.0, 9.0, 25.0], "with Y up the file's Y is the height");
    assert!(!project.stage.show_builtin, "a venue replaces the built-in stage");
    assert_eq!(api.stage_model().unwrap(), file);
    assert!(t.dir.join("stage/model.ptzmesh").is_file());

    // A second upload replaces the first and counts up, so clients know to fetch again.
    let bigger = hall(1000.0).to_bytes();
    let project = api.upload_stage_model("hall-mm.3ds", Unit::Mm, UpAxis::Z, &bigger).unwrap();
    let model = project.stage.model.clone().unwrap();
    assert_eq!((model.name.as_str(), model.revision, model.unit), ("hall-mm.3ds", 2, Unit::Mm));
    assert_eq!(model.size_m(), [40.0, 25.0, 9.0]);
    assert_eq!(api.stage_model().unwrap(), bigger);

    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let api = t.client("b");
    assert_eq!(api.project().unwrap().stage, project.stage);
    assert_eq!(api.stage_model().unwrap(), bigger);
    let log = std::fs::read_to_string(std::fs::read_dir(dir.join("log")).unwrap().next().unwrap().unwrap().path()).unwrap();
    assert_eq!(log.matches("\"stage_model\"").count(), 2);
    drop(t);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_upload_is_refused_and_the_model_in_use_stays() {
    let t = TestServer::start("stage-bad");
    let api = t.client("a");
    api.take_lease("Luke", false).unwrap();
    let file = hall(1.0).to_bytes();
    let before = api.upload_stage_model("hall.obj", Unit::M, UpAxis::Z, &file).unwrap();

    let mut cut = file.clone();
    cut.truncate(file.len() - 7);
    let why = refused(api.upload_stage_model("cut.obj", Unit::M, UpAxis::Z, &cut), 422);
    assert!(why.starts_with("The stage model was not imported: "), "{why}");
    let why = refused(api.upload_stage_model("text.obj", Unit::M, UpAxis::Z, b"v 0 0 0\nf 1 1 1\n"), 422);
    assert!(why.starts_with("The stage model was not imported: "), "{why}");
    let empty = Mesh::default().to_bytes();
    let why = refused(api.upload_stage_model("empty.obj", Unit::M, UpAxis::Z, &empty), 422);
    assert!(why.contains("no triangles"), "{why}");

    assert_eq!(api.project().unwrap().stage, before.stage);
    assert_eq!(api.stage_model().unwrap(), file);
    let left: Vec<String> =
        std::fs::read_dir(t.dir.join("stage")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(left, vec!["model.ptzmesh"], "nothing is left behind by a refused upload");
}

#[test]
fn the_project_places_the_model_but_cannot_invent_one_and_removing_it_brings_the_built_in_stage_back() {
    let t = TestServer::start("stage-place");
    let api = t.client("a");
    api.take_lease("Luke", false).unwrap();

    // No file was uploaded, so a project that names one is refused.
    let mut invented = api.project().unwrap();
    invented.stage.model = Some(ptz_core::camera::StageModel { name: "ghost.obj".into(), revision: 1, ..Default::default() });
    let why = refused(api.put_project(&invented), 409);
    assert_eq!(why, "The stage model is changed by importing a file or removing it, not with the project");

    let uploaded = api.upload_stage_model("hall.obj", Unit::guess(40_000.0), UpAxis::Z, &hall(1000.0).to_bytes()).unwrap();
    assert_eq!(uploaded.stage.model.as_ref().unwrap().unit, Unit::Mm, "forty thousand units across is a hall in millimetres");

    // How it sits in the world, and whether the built-in stage shows with it, are project settings.
    let mut placed = uploaded.clone();
    {
        let model = placed.stage.model.as_mut().unwrap();
        model.unit = Unit::Cm;
        model.up = UpAxis::Y;
        model.offset = [-20.0, 3.0, 0.0];
        model.rotation_deg = 90.0;
    }
    placed.stage.show_builtin = true;
    placed.cameras[0].mount.x = 4.5;
    placed.cameras[0].mount.heading_deg = -30.0;
    assert_eq!(api.put_project(&placed).unwrap(), placed);
    assert_eq!(api.project().unwrap(), placed);

    // What describes the file itself is the server's to say.
    for change in [
        (|m: &mut ptz_core::camera::StageModel| m.triangles = 5) as fn(&mut ptz_core::camera::StageModel),
        |m| m.name = "other.obj".into(),
        |m| m.revision += 1,
        |m| m.max[0] = 1.0,
    ] {
        let mut lie = placed.clone();
        change(lie.stage.model.as_mut().unwrap());
        refused(api.put_project(&lie), 409);
    }
    let mut gone = placed.clone();
    gone.stage.model = None;
    refused(api.put_project(&gone), 409);
    let mut bad = placed.clone();
    bad.stage.model.as_mut().unwrap().offset[2] = f64::NAN;
    assert!(api.put_project(&bad).is_err());
    let mut bad = placed.clone();
    bad.cameras[0].mount.base_height_mm = 9000.0;
    assert!(refused(api.put_project(&bad), 422).contains("base height"));

    // Armed, the stage is left alone.
    let song = api.new_song("Opening", None).unwrap();
    api.commit(&song.song.id).unwrap();
    api.load_song(&song.song.id).unwrap();
    do_act(&api, Action::Arm);
    assert_eq!(refused(api.delete_stage_model(), 409), "Disarm to change the stage model");
    assert_eq!(
        refused(api.upload_stage_model("hall.obj", Unit::M, UpAxis::Z, &hall(1.0).to_bytes()), 409),
        "Disarm to change the stage model"
    );
    do_act(&api, Action::Disarm);

    let after = api.delete_stage_model().unwrap();
    assert!(after.stage.model.is_none() && after.stage.show_builtin);
    assert_eq!(after.cameras, placed.cameras, "the cameras stay where they were put");
    assert!(!t.dir.join("stage/model.ptzmesh").exists());
    refused(api.stage_model(), 404);
    assert_eq!(refused(api.delete_stage_model(), 404), "There is no stage model to remove");
}

#[test]
fn a_project_that_names_a_model_whose_file_is_gone_opens_with_the_built_in_stage() {
    let t = TestServer::start("stage-missing");
    let api = t.client("a");
    api.take_lease("Luke", false).unwrap();
    api.upload_stage_model("hall.obj", Unit::M, UpAxis::Z, &hall(1.0).to_bytes()).unwrap();
    let dir = t.stop_keeping_data();
    std::fs::remove_file(dir.join("stage/model.ptzmesh")).unwrap();
    let t = TestServer::reopen(&dir);
    let stage = t.client("b").project().unwrap().stage;
    assert!(stage.model.is_none() && stage.show_builtin);
    drop(t);
    let _ = std::fs::remove_dir_all(dir);
}
