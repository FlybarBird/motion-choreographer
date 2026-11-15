//! The edit lease from the panel: who holds it is always shown, and another
//! station's lease is taken over only by a deliberate hold.

mod common;

use common::*;
use ptz_panel::codec::{Color, Lit};
use std::time::Duration;

#[test]
fn another_stations_lease_is_named_and_taken_over_only_by_a_hold() {
    let b = Bench::start("lease-other");
    let desk = b.server.client("desk");
    desk.take_lease("Front desk", false).expect("the desk takes the lease");

    b.mode(SHOW);
    b.wait("the lease key to name the holder", || b.line(K1) == "Take over Front desk");
    assert_eq!(b.title(K1), "Hold", "the key says it takes a hold");
    assert_eq!(b.light(K1), (Lit::On, Color::Amber));
    b.wait("the status display to name the holder", || {
        b.panel.history(STATUS_TILE).iter().any(|t| t.line2 == "Lease: Front desk")
    });

    // The panel only watches: what it tries is refused in the server's words, and nothing changes.
    let songs = b.api.songs().expect("songs").len();
    b.press_key("New");
    b.wait_refused("Take the edit lease to control the show");
    assert_eq!(b.api.songs().expect("songs").len(), songs);

    // A tap takes nothing and says what a hold would do. Most of a hold takes nothing either.
    b.press(K1);
    b.wait_said("Hold Take over Front desk for 1 second. Front desk holds the edit lease. Taking it over ends their editing");
    b.hold(K1, Duration::from_millis(700));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(b.status().lease.expect("the lease").holder, "Front desk");
    assert!(desk.status().expect("status").you_hold_lease);

    let took = b.hold_until(K1, "the hold to take the lease over", || b.status().lease.is_some_and(|l| l.holder == "Panel"));
    assert!(took >= Duration::from_millis(950), "taken over after only {took:?} of holding");
    b.wait_said("Panel holds the edit lease");
    b.wait_key("Release");
    assert_eq!(b.light(K1), (Lit::On, Color::Blue));
    assert!(!desk.status().expect("status").you_hold_lease);
    let record = b.log().into_iter().rfind(|r| r.kind == "lease").expect("a lease record");
    assert_eq!((&record.data["holder"], &record.data["took_from"]), (&"Panel".into(), &"Front desk".into()));

    // The desk edits no more, and the panel does.
    assert_eq!(
        desk.new_song("From the desk", None).expect_err("the desk has lost the lease").message(),
        "Take the edit lease to control the show"
    );
    b.press_key("New");
    b.wait("the panel's song to exist", || b.api.songs().expect("songs").len() == songs + 1);

    // Released from the panel, the lease is anyone's again.
    b.wait_key("Release");
    b.press_key("Release");
    b.wait_said("The edit lease is released");
    assert!(b.status().lease.is_none());
    b.wait_key("Take");
    desk.take_lease("Front desk", false).expect("the desk takes the free lease");
}

#[test]
fn the_station_name_is_the_one_given() {
    let b = Bench::with("lease-name", true, |o| o.station = "Stage left".into());
    b.mode(SHOW);
    b.press_key("Take");
    b.wait("the panel to hold the lease under its name", || b.status().lease.is_some_and(|l| l.holder == "Stage left"));
    b.wait_said("Stage left holds the edit lease");
    assert_eq!(b.status().panel.expect("the panel's state").station, "Stage left");
}
