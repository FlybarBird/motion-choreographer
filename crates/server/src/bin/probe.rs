//! Transport probe. Drives a running server over HTTP into every transport
//! state it can reach and sends every action in each one. An action guidance
//! lists as enabled must succeed; any other must be refused with the reason
//! guidance gave. Exits non-zero on any mismatch.

use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::edit;
use ptz_core::transport::{self, Action, Event, Output, Run, TimeMode, TransportState};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const HELP: &str = "\
ptz-probe: check every transport action in every reachable state

Usage: ptz-probe [--server http://127.0.0.1:8787] [--access-key <key>] [--verbose]

The probe takes the edit lease, creates a scratch song, plays, arms and
E-Stops, then removes the scratch song. It only runs against the simulator rig.
";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Act(Action),
    /// Push the rig off the playhead pose first, so Arm stays in Positioning.
    ArmHeld,
}

impl Step {
    fn action(self) -> Action {
        match self {
            Step::Act(a) => a,
            Step::ArmHeld => Action::Arm,
        }
    }
}

struct Probe {
    api: Api,
    watcher: Api,
    song: String,
    verbose: bool,
    checks: usize,
    failures: Vec<String>,
}

fn short(s: &TransportState) -> String {
    let run = match s.run {
        Run::Stopped => "stopped",
        Run::Playing => "playing",
        Run::WaitingForCode => "waiting",
    };
    let time = match s.time {
        TimeMode::FreeRun => "free",
        TimeMode::LockedFile => "file",
        TimeMode::LockedExternal => "code",
    };
    let output = match s.output {
        Output::Rehearse => "rehearse",
        Output::Positioning => "positioning",
        Output::Armed => "armed",
    };
    format!(
        "{run:<7} {time:<4} {output:<11} {} {}",
        if s.record { "rec" } else { "   " },
        if s.estop { "ESTOP" } else { "     " }
    )
}

fn sort_key(s: &TransportState) -> (u8, u8, u8, bool, bool) {
    (s.estop as u8, s.time as u8, s.output as u8, s.record, s.run != Run::Stopped)
}

/// States the server may settle in after `action`, allowing for the events
/// that follow on their own (the rig arriving, code entering).
fn expected_after(s: &TransportState, action: Action) -> Vec<TransportState> {
    let first = transport::apply(s, action, true);
    let mut out = vec![first];
    let mut i = 0;
    while i < out.len() {
        for ev in [Event::RigArrived, Event::CodeEntered] {
            let n = transport::on_event(&out[i], ev);
            if !out.contains(&n) {
                out.push(n);
            }
        }
        i += 1;
    }
    out
}

impl Probe {
    fn fail(&mut self, msg: String) {
        eprintln!("  MISMATCH: {msg}");
        self.failures.push(msg);
    }

    fn check(&mut self, ok: bool, msg: impl FnOnce() -> String) {
        self.checks += 1;
        if !ok {
            self.fail(msg());
        }
    }

    /// Status once the transport has stopped changing.
    fn settle(&mut self) -> Result<Status, ApiErr> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut last = self.api.status()?;
        loop {
            std::thread::sleep(Duration::from_millis(40));
            let now = self.api.status()?;
            if now.transport == last.transport || Instant::now() > deadline {
                return Ok(now);
            }
            last = now;
        }
    }

    fn request_for(action: Action) -> TransportRequest {
        match action {
            Action::Seek => TransportRequest { frame: Some(5), ..Default::default() },
            Action::Lock => TransportRequest { source: Some("artnet".into()), ..Default::default() },
            _ => TransportRequest::default(),
        }
    }

    fn do_step(&mut self, step: Step) -> Result<Status, ApiErr> {
        if step == Step::ArmHeld {
            let until = Instant::now() + Duration::from_millis(400);
            while Instant::now() < until {
                self.api.jog(&Jog { elevation: 1.0, ..Jog::default() })?;
                std::thread::sleep(Duration::from_millis(50));
            }
            self.api.jog(&Jog::default())?;
        }
        self.api.act(step.action(), &Self::request_for(step.action()))?;
        self.settle()
    }

    /// Return to idle using only actions guidance lists as enabled, and never E-Stop.
    fn go_idle(&mut self) -> Result<Status, ApiErr> {
        for _ in 0..12 {
            let st = self.settle()?;
            let s = st.transport;
            let next = if s.estop {
                Action::ResetEstop
            } else if s.run != Run::Stopped {
                Action::Stop
            } else if s.output != Output::Rehearse {
                Action::Disarm
            } else if s.record {
                Action::RecordOff
            } else if s.locked() {
                Action::Unlock
            } else {
                // Park the playhead and put the virtual rig back on the timeline.
                let st = self.api.act(Action::Cue, &TransportRequest::default())?;
                if !st.song.as_ref().is_some_and(|x| x.committed_clean) {
                    if st.song.as_ref().is_some_and(|x| x.dirty) {
                        self.api.save(&self.song)?;
                    }
                    self.api.commit(&self.song)?;
                }
                return self.settle();
            };
            if !st.guidance.allows(next) {
                let msg = format!("dead end: in [{}] the way back to idle needs {:?}, which is not enabled", short(&s), next);
                self.fail(msg.clone());
                return Err(ApiErr::Network(msg));
            }
            self.api.act(next, &TransportRequest::default())?;
        }
        let msg = "dead end: idle was not reached in 12 steps".to_string();
        self.fail(msg.clone());
        Err(ApiErr::Network(msg))
    }

    fn goto(&mut self, recipe: &[Step], want: &TransportState) -> Result<Status, ApiErr> {
        let mut st = self.go_idle()?;
        for step in recipe {
            st = self.do_step(*step)?;
        }
        if st.transport != *want {
            let msg = format!("could not return to [{}]; the server is in [{}]", short(want), short(&st.transport));
            self.fail(msg.clone());
            return Err(ApiErr::Network(msg));
        }
        Ok(st)
    }

    fn check_status(&mut self, st: &Status) {
        let s = st.transport;
        let live = !s.estop && matches!(s.output, Output::Positioning | Output::Armed);
        self.check(st.hardware_live == live, || {
            format!("[{}] hardware_live is {} but output is {:?}", short(&s), st.hardware_live, s.output)
        });
        let g = &st.guidance;
        let mut texts: Vec<&str> = g.blocked.values().map(|t| t.as_str()).collect();
        texts.push(&g.hint);
        for text in texts {
            for named in transport::actions_named_in(text) {
                self.check(g.allows(named), || format!("[{}] \"{text}\" names {:?}, which is not enabled", short(&s), named));
            }
        }
        self.check(g.primary.is_some_and(|p| g.allows(p)), || {
            format!("[{}] the main button {:?} is not an enabled action", short(&s), g.primary)
        });
        self.check(g.allows(Action::Estop), || format!("[{}] E-Stop is not enabled", short(&s)));
    }

    fn check_watcher(&mut self, s: &TransportState) -> Result<(), ApiErr> {
        let st = self.watcher.status()?;
        self.check(st.guidance.enabled == vec![Action::Estop], || {
            format!("[{}] a watcher is offered {:?}; only E-Stop is expected", short(s), st.guidance.enabled)
        });
        let refused = self.watcher.act(Action::Play, &TransportRequest::default());
        self.check(
            matches!(&refused, Err(ApiErr::Refused { code: 409, msg }) if Some(msg.as_str()) == st.guidance.reason(Action::Play)),
            || format!("[{}] a watcher's Play was not refused with the guidance reason: {refused:?}", short(s)),
        );
        Ok(())
    }

    fn run(&mut self) -> Result<BTreeMap<String, (TransportState, Vec<String>)>, ApiErr> {
        let mut recipes: Vec<(TransportState, Vec<Step>)> = vec![(TransportState::IDLE, Vec::new())];
        let mut queue: VecDeque<usize> = VecDeque::from([0]);
        let mut table: BTreeMap<String, (TransportState, Vec<String>)> = BTreeMap::new();

        while let Some(index) = queue.pop_front() {
            let (state, recipe) = recipes[index].clone();
            let mut row = Vec::new();
            let mut here = self.goto(&recipe, &state)?;
            self.check_status(&here);
            self.check_watcher(&state)?;
            if self.verbose {
                println!("[{}] via {:?}", short(&state), recipe);
            }

            let mut steps: Vec<Step> = Action::ALL.iter().map(|a| Step::Act(*a)).collect();
            steps.push(Step::ArmHeld);
            for step in steps {
                let action = step.action();
                let enabled = here.guidance.allows(action);
                if step == Step::ArmHeld && !enabled {
                    continue;
                }
                if !enabled {
                    let reason = here.guidance.reason(action).unwrap_or("").to_string();
                    let result = self.api.act(action, &Self::request_for(action));
                    self.check(matches!(&result, Err(ApiErr::Refused { code: 409, msg }) if *msg == reason), || {
                        format!(
                            "[{}] {:?} should be refused with \"{reason}\" but the server said {result:?}",
                            short(&state),
                            action
                        )
                    });
                    let after = self.settle()?;
                    self.check(after.transport == state, || {
                        format!(
                            "[{}] a refused {:?} still changed the state to [{}]",
                            short(&state),
                            action,
                            short(&after.transport)
                        )
                    });
                    row.push("--".to_string());
                    continue;
                }
                match self.do_step(step) {
                    Ok(after) => {
                        self.check_status(&after);
                        let allowed = expected_after(&state, action);
                        self.check(allowed.contains(&after.transport), || {
                            format!(
                                "[{}] {:?} led to [{}], which the rules do not allow",
                                short(&state),
                                action,
                                short(&after.transport)
                            )
                        });
                        if !recipes.iter().any(|(s, _)| *s == after.transport) {
                            let mut r = recipe.clone();
                            r.push(step);
                            recipes.push((after.transport, r));
                            queue.push_back(recipes.len() - 1);
                        }
                        if self.verbose {
                            println!("    {:?} -> [{}]", step, short(&after.transport));
                        }
                        if step != Step::ArmHeld {
                            row.push("ok".to_string());
                        }
                    }
                    Err(e) => {
                        self.check(false, || {
                            format!("[{}] {:?} is listed as enabled but the server refused it: {e}", short(&state), action)
                        });
                        if step != Step::ArmHeld {
                            row.push("!!".to_string());
                        }
                    }
                }
                here = self.goto(&recipe, &state)?;
            }
            table.insert(format!("{:?}", sort_key(&state)), (state, row));
        }

        // A watcher's E-Stop must work while the show is live.
        if let Some((state, recipe)) = recipes.iter().find(|(s, _)| s.output == Output::Armed && s.run == Run::Playing).cloned() {
            self.goto(&recipe, &state)?;
            let st = self.watcher.act(Action::Estop, &TransportRequest::default());
            self.check(matches!(&st, Ok(s) if s.transport.estop && !s.hardware_live && s.transport.run == Run::Stopped), || {
                format!("a watcher's E-Stop during a live run did not latch: {st:?}")
            });
        } else {
            self.fail("the probe never reached Armed and Playing".into());
        }
        self.go_idle()?;
        Ok(table)
    }
}

fn die(msg: &str) -> ! {
    eprintln!("ptz-probe: {msg}");
    std::process::exit(2);
}

fn main() {
    let mut server = format!("http://127.0.0.1:{DEFAULT_PORT}");
    let mut key = std::env::var("PTZ_ACCESS_KEY").ok();
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => server = args.next().unwrap_or_else(|| die("--server needs a value")),
            "--access-key" => key = args.next(),
            "--verbose" | "-v" => verbose = true,
            "--help" | "-h" => {
                print!("{HELP}");
                return;
            }
            other => die(&format!("unknown option {other}. Try --help")),
        }
    }

    let id = format!("probe-{}", std::process::id());
    let api = Api::new(&server, &id, key.as_deref());
    let watcher = Api::new(&server, &format!("{id}-watcher"), key.as_deref());

    let start = api.status().unwrap_or_else(|e| die(&format!("{e}\nIs ptz-server running at {server}?")));
    if start.adapters.first().map(|a| a.name.as_str()) != Some("Simulator") {
        die("the probe arms and plays, so it only runs against the simulator rig. Set the rig to Simulator first.");
    }
    if start.transport != TransportState::IDLE {
        die("the show is not idle. Stop, disarm and unlock before probing.");
    }
    let previous_song = start.song.as_ref().map(|s| s.id.clone());
    let or_die = |what: &str, e: ApiErr| -> ! { die(&format!("{what}: {e}")) };

    api.take_lease("ptz-probe", true).unwrap_or_else(|e| or_die("could not take the lease", e));
    let project = api.project().unwrap_or_else(|e| or_die("could not read the project", e));
    let presets = api.presets().unwrap_or_else(|e| or_die("could not read presets", e));

    // A scratch song with one move, committed so Arm is possible.
    let scratch = api.new_song("Probe Scratch", None).unwrap_or_else(|e| or_die("could not create the scratch song", e));
    let song_id = scratch.song.id.clone();
    let mut song = scratch.song;
    if let Some(preset) = presets.first() {
        if let Ok((next, _)) = edit::insert_preset_move(&song, &project, preset, 50) {
            song = next;
        }
    }
    let setup = (|| -> Result<(), ApiErr> {
        api.put_working(&PutWorking { expected_counter: scratch.edit_counter, song })?;
        api.save(&song_id)?;
        api.commit(&song_id)?;
        api.load_song(&song_id)?;
        // Slow positioning, so the Positioning state holds still long enough to test.
        let mut slow = project.clone();
        slow.rig.positioning_speed = 0.0005;
        api.put_project(&slow)?;
        Ok(())
    })();
    if let Err(e) = setup {
        let _ = api.delete_song(&song_id);
        or_die("could not prepare the scratch song", e);
    }

    let mut probe = Probe { api: api.clone(), watcher, song: song_id.clone(), verbose, checks: 0, failures: Vec::new() };
    let started = Instant::now();
    let result = probe.run();

    // Put everything back.
    let _ = probe.go_idle();
    let _ = api.put_project(&project);
    if let Some(prev) = previous_song.filter(|p| *p != song_id) {
        let _ = api.load_song(&prev);
    }
    let _ = api.delete_song(&song_id);
    let _ = api.release_lease();

    match result {
        Ok(table) => {
            let width = 36;
            print!("{:<width$}", "run     time output      rec estop");
            for a in Action::ALL {
                let head = match a {
                    Action::RecordOn => "rec+",
                    Action::RecordOff => "rec-",
                    Action::ResetEstop => "reset",
                    Action::Disarm => "disarm",
                    other => other.key(),
                };
                print!(" {head:<6}");
            }
            println!();
            for (state, row) in table.values() {
                print!("{:<width$}", short(state));
                for cell in row {
                    print!(" {cell:<6}");
                }
                println!();
            }
            println!();
            println!("ok = enabled and accepted   -- = refused with the reason guidance gave   !! = mismatch");
            println!(
                "{} states, {} checks, {} mismatches, {:.1} s",
                table.len(),
                probe.checks,
                probe.failures.len(),
                started.elapsed().as_secs_f64()
            );
        }
        Err(e) => {
            eprintln!("ptz-probe stopped early: {e}");
            if probe.failures.is_empty() {
                probe.failures.push(e.to_string());
            }
        }
    }
    if !probe.failures.is_empty() {
        eprintln!("\n{} mismatch(es):", probe.failures.len());
        for f in &probe.failures {
            eprintln!("  - {f}");
        }
        std::process::exit(1);
    }
}
