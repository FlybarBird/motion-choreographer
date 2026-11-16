//! The panel link by itself, with no controller and no server behind it. What
//! it does to the operator's hand on the rig, and to an E-Stop press, must not
//! depend on anything else being quick, or there at all.

use ptz_panel::fake::FakePanel;
use ptz_panel::link::{run_panel, Event, PanelLink, Shared, QUIET_CLOSE, QUIET_LIMIT};
use ptz_panel::map::PanelMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

const JOY_PAN: u32 = 1;
const JOY_TILT: u32 = 2;
const ESTOP: u32 = 56;

struct Link {
    panel: FakePanel,
    shared: Arc<Shared>,
    events: Receiver<Event>,
    presses: Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Link {
    /// A connected panel, and a hand that everything but the link allows to drive.
    fn start() -> Link {
        Self::with_panel(FakePanel::start())
    }

    fn with_panel(panel: FakePanel) -> Link {
        let shared = Shared::new();
        let (events_tx, events) = std::sync::mpsc::channel();
        let (presses_tx, presses) = std::sync::mpsc::channel();
        let link = PanelLink { dial: Some(panel.addr.to_string()), listener: None, map: PanelMap::ptz_pro() };
        let thread = {
            let shared = shared.clone();
            std::thread::spawn(move || run_panel(link, shared, events_tx, presses_tx))
        };
        let link = Link { panel, shared, events, presses, thread: Some(thread) };
        {
            let mut hand = link.shared.hand();
            hand.enabled = true;
            hand.server_up = true;
            hand.camera = "cam1".into();
            hand.speed = 1.0;
        }
        link.wait("the panel to be connected and to have answered a ping", || link.shared.hand().panel_up);
        link
    }

    fn wait(&self, what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(8), "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn pan(&self) -> f64 {
        self.shared.hand().jog(Instant::now()).pan
    }

    fn push(&self) {
        self.panel.deflect(JOY_PAN, 400);
        self.wait("the joystick to drive", || self.pan() == 0.8);
    }

    /// How long until the hand drives nothing.
    fn let_go_after(&self, since: Instant) -> Duration {
        self.wait("the hand to be let go of", || self.shared.hand().jog(Instant::now()).is_zero());
        since.elapsed()
    }

    fn saw(&self, mut which: impl FnMut(&Event) -> bool) -> bool {
        self.events.try_iter().any(|e| which(&e))
    }
}

#[test]
fn an_idle_panel_is_woken_and_stays_awake_past_its_two_second_sleep_timer_even_after_reconnect() {
    let panel = FakePanel::start();
    panel.set_sleep_timer(50);
    let started = Instant::now();
    while !panel.is_sleeping() {
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    // The user's short timer is already configured when the controller connects.
    panel.set_sleep_timer(2000);
    let l = Link::with_panel(panel);
    l.wait("the already-sleeping panel to wake", || !l.panel.is_sleeping());
    let opened = Instant::now();
    while opened.elapsed() < Duration::from_millis(2300) {
        assert!(!l.panel.is_sleeping(), "heartbeats alone must not stand in for disabling operator-inactivity sleep");
        assert!(l.shared.hand().panel_up);
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(l.panel.lines().iter().any(|line| line == "SleepTimer=0"));
    assert!(l.panel.unknown().is_empty());

    l.panel.clear_lines();
    l.panel.set_sleep_timer(2000);
    l.panel.drop_connections();
    l.wait("the panel to be initialized again", || l.panel.lines().iter().any(|line| line == "SleepTimer=0"));
    l.wait("the reconnected link to be proven", || l.shared.hand().panel_up);
    assert!(!l.panel.is_sleeping());
}

#[test]
fn a_panel_that_announces_sleep_is_woken_and_its_unchanged_feedback_is_restored() {
    let l = Link::start();
    {
        let mut surface = l.shared.surface();
        surface.0 += 1;
        surface.1.insert(
            6,
            ptz_panel::link::Cell {
                text: ptz_panel::codec::Text { line1: "Still here".into(), ..Default::default() },
                ..Default::default()
            },
        );
    }
    l.wait("the initial feedback", || l.panel.text(6).line1 == "Still here");
    l.panel.clear_lines();
    l.push();
    l.panel.set_sleep_timer(60);
    l.wait("the controller to wake the panel after its sleep report", || l.panel.lines().iter().any(|line| line == "WakeUp!"));
    l.wait("unchanged feedback to be redrawn", || !l.panel.is_sleeping() && l.panel.text(6).line1 == "Still here");
    assert_eq!(l.pan(), 0.0, "waking must not restart a deflected joystick");
    assert!(l.shared.hand().rest_first);
    assert!(!l.saw(|e| matches!(e, Event::PanelDown(_))), "sleep recovery should keep the same connection");
}

#[test]
fn mapping_tools_also_wake_a_sleeping_panel() {
    let panel = FakePanel::start();
    panel.set_sleep_timer(50);
    let started = Instant::now();
    while !panel.is_sleeping() {
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut session = ptz_panel::tool::Session::open(&panel.addr.to_string(), Duration::from_secs(2)).unwrap();
    assert!(!panel.is_sleeping());
    assert!(session.topology.is_some());
    panel.down(6);
    assert_eq!(session.next_event(Duration::from_secs(1)).unwrap(), Some((6, ptz_panel::codec::Trigger::Down)));
    assert!(panel.unknown().is_empty());
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn a_closed_connection_lets_go_of_the_hand_at_once_and_the_stick_must_rest_before_it_drives_again() {
    let l = Link::start();
    l.push();
    let lost = Instant::now();
    l.panel.drop_connections();
    let took = l.let_go_after(lost);
    assert!(took < Duration::from_millis(150), "the hand was let go of {took:?} after the connection closed");
    l.wait("the link to say the panel is gone", || l.saw(|e| matches!(e, Event::PanelDown(_))));
    {
        let hand = l.shared.hand();
        assert!(hand.rest_first && !hand.stick_deflected(), "{hand:?}");
    }

    // The panel is back, the stick still over and saying so: nothing is driven.
    l.wait("the panel to be connected again", || l.shared.hand().panel_up);
    l.panel.deflect(JOY_PAN, 410);
    l.panel.deflect(JOY_TILT, 200);
    l.wait("the link to say the stick must rest first", || l.saw(|e| matches!(e, Event::RestFirst)));
    assert!(l.shared.hand().jog(Instant::now()).is_zero());
    l.panel.deflect(JOY_PAN, 0);
    std::thread::sleep(Duration::from_millis(100));
    assert!(l.shared.hand().jog(Instant::now()).is_zero(), "one axis at rest is not the joystick at rest");
    l.panel.deflect(JOY_TILT, 0);
    l.push();
}

#[test]
fn a_silent_link_lets_go_of_the_hand_after_the_close_limit_and_is_given_up_after_the_slow_one() {
    let l = Link::start();
    l.push();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(l.pan(), 0.8, "a stick held still on a live link keeps driving");

    let lost = Instant::now();
    l.panel.set_silent(true);
    let took = l.let_go_after(lost);
    println!("silent link: the hand was let go of after {took:?}");
    assert!(took < QUIET_CLOSE + Duration::from_millis(60), "the hand was let go of {took:?} after the link went silent");
    assert!(
        took > QUIET_CLOSE - Duration::from_millis(130),
        "let go of after {took:?}: sooner than a live link could be told from a dead one"
    );
    assert!(l.saw(|e| matches!(e, Event::PanelQuiet)));
    assert!(l.shared.hand().rest_first);

    // The connection itself is given up when nothing has been heard for the slow limit.
    l.wait("the link to give the panel up", || l.saw(|e| matches!(e, Event::PanelDown(_))));
    let gone = lost.elapsed();
    assert!(
        gone > QUIET_LIMIT - Duration::from_millis(150) && gone < QUIET_LIMIT + Duration::from_millis(400),
        "given up after {gone:?}"
    );
    assert!(!l.shared.hand().panel_up);
}

#[test]
fn with_nothing_deflected_a_silent_link_is_given_up_after_the_slow_limit() {
    let l = Link::start();
    std::thread::sleep(Duration::from_millis(500));
    let lost = Instant::now();
    l.panel.set_silent(true);
    l.wait("the link to give the panel up", || l.saw(|e| matches!(e, Event::PanelDown(_))));
    let gone = lost.elapsed();
    println!("silent link, nothing deflected: given up after {gone:?}");
    assert!(
        gone > QUIET_LIMIT - Duration::from_millis(450) && gone < QUIET_LIMIT + Duration::from_millis(400),
        "given up after {gone:?}"
    );
}

#[test]
fn an_estop_press_goes_to_its_own_sender_whatever_else_is_waiting() {
    let l = Link::start();
    // Nobody reads the events: a controller that is busy, or stuck.
    for _ in 0..50 {
        l.panel.turn(5, 1);
    }
    let pressed = Instant::now();
    l.panel.down(ESTOP);
    l.presses.recv_timeout(Duration::from_millis(500)).expect("the press reaches the E-Stop sender");
    assert!(pressed.elapsed() < Duration::from_millis(200), "the press took {:?}", pressed.elapsed());
    l.panel.up(ESTOP);
    assert!(l.presses.recv_timeout(Duration::from_millis(200)).is_err(), "letting go is not a second press");
}
