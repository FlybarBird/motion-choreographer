//! Audit: every route against the edit lease and the access key.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::pose::Pose;
use serde_json::{json, Value};

const NO_LEASE: &str = "Take the edit lease to control the show";

enum Body {
    None,
    Json(Value),
    Bytes(&'static [u8]),
}

/// Every route that changes something, with a body its handler accepts.
fn mutating(api: &Api) -> Vec<(&'static str, String, Body)> {
    let w = api.working("walk-in").unwrap();
    let project = api.project().unwrap();
    let put = serde_json::to_value(PutWorking { expected_counter: w.edit_counter, song: w.song }).unwrap();
    let preset = serde_json::to_value(StorePreset { name: "X".into(), pose: Some(Pose::home()), camera: None }).unwrap();
    let first_preset = api.presets().unwrap()[0].id.clone();
    let mut routes = vec![
        ("POST", "/songs".to_string(), Body::Json(json!({ "name": "New" }))),
        ("DELETE", "/songs/walk-in".into(), Body::None),
        ("POST", "/songs/walk-in/load".into(), Body::None),
        ("PUT", "/songs/walk-in/working".into(), Body::Json(put)),
        ("POST", "/songs/walk-in/save".into(), Body::None),
        ("POST", "/songs/walk-in/revert".into(), Body::None),
        ("POST", "/songs/walk-in/copy".into(), Body::Json(json!({ "name": "Copy" }))),
        ("POST", "/songs/walk-in/commit".into(), Body::None),
        ("POST", "/estop/reset".into(), Body::None),
        ("PUT", "/project".into(), Body::Json(serde_json::to_value(project).unwrap())),
        ("POST", "/presets".into(), Body::Json(preset)),
        ("POST", "/presets/sync".into(), Body::None),
        ("DELETE", format!("/presets/{first_preset}"), Body::None),
        ("POST", "/rig/jog".into(), Body::Json(json!({ "pan": 1.0, "tilt": 0.0, "zoom": 0.0, "focus": 0.0, "elevation": 0.0 }))),
        ("POST", "/record/c1/refit".into(), Body::Json(json!({ "scale": 1.0 }))),
        ("POST", "/media?name=x.wav".into(), Body::Bytes(b"RIFF")),
        ("DELETE", "/media/m-click".into(), Body::None),
        ("POST", "/media/m-click/start_tc".into(), Body::Json(json!({ "start_tc": "01:00:00:00" }))),
        ("POST", "/stage/model?name=x".into(), Body::Bytes(b"mesh")),
        ("DELETE", "/stage/model".into(), Body::None),
    ];
    for action in ["play", "stop", "cue", "seek", "lock", "unlock", "arm", "disarm", "record_on", "record_off"] {
        routes.push((
            "POST",
            format!("/transport/{action}"),
            Body::Json(json!({ "frame": 10, "source": "ltc", "axes": ["pan"] })),
        ));
    }
    routes
}

const READS: [&str; 13] = [
    "/status",
    "/songs",
    "/songs/walk-in/working",
    "/songs/walk-in/revisions",
    "/project",
    "/inputs",
    "/presets",
    "/presets/stage-wide/thumb.jpg",
    "/media",
    "/media/m-click/peaks?spp=256",
    "/media/m-click/frames/0.jpg",
    "/stage/model",
    "/log?since=0",
];

fn call(api: &Api, method: &str, path: &str, body: &Body) -> Result<Value, ApiErr> {
    match (method, body) {
        ("POST", Body::None) => api.post_empty(path),
        ("POST", Body::Json(v)) => api.post(path, v),
        ("POST", Body::Bytes(b)) => api.post_bytes(path, b),
        ("PUT", Body::Json(v)) => api.put(path, v),
        ("DELETE", _) => api.delete(path),
        _ => unreachable!(),
    }
}

#[test]
fn every_mutating_route_needs_the_lease_and_estop_does_not() {
    let t = TestServer::start("audit-routes-lease");
    let holder = t.client("holder");
    holder.take_lease("Luke-MacBook", false).unwrap();
    let other = t.client("other");
    let before = (other.songs().unwrap(), other.project().unwrap(), other.presets().unwrap());
    let mut open = Vec::new();
    for (method, path, body) in mutating(&other) {
        match call(&other, method, &path, &body) {
            Err(ApiErr::Refused { code: 409, msg }) if msg == NO_LEASE => {}
            got => open.push(format!("{method} {path} -> {got:?}")),
        }
    }
    assert!(open.is_empty(), "routes that did not ask for the lease:\n{}", open.join("\n"));
    assert_eq!((other.songs().unwrap(), other.project().unwrap(), other.presets().unwrap()), before);

    // No id at all is nobody, not the holder.
    let anonymous = Api::new(&t.url, "", None);
    assert_eq!(refused(anonymous.post_empty::<Value>("/songs/walk-in/save"), 409), NO_LEASE);
    // Releasing someone else's lease does nothing.
    other.release_lease().unwrap();
    assert!(holder.status().unwrap().you_hold_lease);
    // The two E-Stop keys are not transport actions; the button has its own address and needs nothing.
    assert!(matches!(other.post_empty::<Value>("/transport/estop"), Err(ApiErr::Refused { code: 404, .. })));
    assert!(matches!(other.post_empty::<Value>("/transport/reset_estop"), Err(ApiErr::Refused { code: 404, .. })));
    assert!(anonymous.post_empty::<Status>("/estop").unwrap().transport.estop);
    assert_eq!(refused(other.post_empty::<Value>("/estop/reset"), 409), NO_LEASE);
    holder.post_empty::<Status>("/estop/reset").unwrap();
    // Every read answers without the lease.
    for path in READS {
        match other.get_bytes(path) {
            Ok(_) | Err(ApiErr::Refused { code: 404, .. }) => {}
            got => panic!("GET {path} -> {:?}", got.map(|b| b.len())),
        }
    }

    // A forced takeover moves the lease at once: the old holder is refused on its next request.
    other.take_lease("Booth", true).unwrap();
    assert_eq!(refused(holder.post_empty::<Value>("/songs/walk-in/save"), 409), NO_LEASE);
    assert!(other.post_empty::<Value>("/songs/walk-in/save").is_ok());
}

#[test]
fn every_route_needs_the_access_key_when_one_is_set() {
    let t = TestServer::start_keyed("audit-routes-key", "s3cret");
    let right = t.client("holder");
    right.take_lease("Luke-MacBook", false).unwrap();
    let mut open = Vec::new();
    for (who, api) in [("no key", Api::new(&t.url, "holder", None)), ("wrong key", Api::new(&t.url, "holder", Some("S3cret")))] {
        for (method, path, body) in mutating(&right) {
            if !matches!(call(&api, method, &path, &body), Err(ApiErr::Refused { code: 401, .. })) {
                open.push(format!("{who}: {method} {path}"));
            }
        }
        for path in READS.iter().copied().chain(["/ws", "/ws?client=holder"]) {
            if !matches!(api.get_bytes(path), Err(ApiErr::Refused { code: 401, .. })) {
                open.push(format!("{who}: GET {path}"));
            }
        }
        for (method, path) in [("POST", "/lease"), ("DELETE", "/lease"), ("POST", "/estop"), ("POST", "/estop/reset")] {
            let body =
                if path == "/lease" && method == "POST" { Body::Json(json!({ "name": "x", "force": true })) } else { Body::None };
            if !matches!(call(&api, method, path, &body), Err(ApiErr::Refused { code: 401, .. })) {
                open.push(format!("{who}: {method} {path}"));
            }
        }
    }
    assert!(open.is_empty(), "routes that answered without the key:\n{}", open.join("\n"));
    // Nothing the keyless stations sent changed anything: the holder still holds, and E-Stop is not latched.
    let st = right.status().unwrap();
    assert!(st.you_hold_lease && !st.transport.estop);
    // The key is also taken from the query, which is how a browser WebSocket has to send it.
    let by_query = Api::new(&t.url, "viewer", None);
    assert!(by_query.get::<Status>("/status?key=s3cret").is_ok());
    // Outside the API prefix there is nothing to reach.
    assert!(matches!(Api::new(&t.url, "x", None).get_bytes("/../project.json"), Err(ApiErr::Refused { .. })));
}
