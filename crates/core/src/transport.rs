//! Transport rules. The state is five independent fields; this module decides
//! which actions are legal in each combination and what the operator is told.
//! The server enforces it and the app renders it, so they cannot disagree.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Run {
    Stopped,
    Playing,
    WaitingForCode,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TimeMode {
    FreeRun,
    /// Locked to a dropped file that the server itself rolls.
    LockedFile,
    /// Locked to LTC, MTC or Art-Net, which roll on their own.
    LockedExternal,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    Rehearse,
    /// Armed and moving the rig to the pose at the playhead.
    Positioning,
    Armed,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TransportState {
    pub run: Run,
    pub time: TimeMode,
    pub output: Output,
    pub record: bool,
    pub estop: bool,
}

impl TransportState {
    pub const IDLE: TransportState =
        TransportState { run: Run::Stopped, time: TimeMode::FreeRun, output: Output::Rehearse, record: false, estop: false };

    /// True only while the server may command hardware.
    pub fn hardware_live(&self) -> bool {
        !self.estop && matches!(self.output, Output::Positioning | Output::Armed)
    }

    pub fn locked(&self) -> bool {
        self.time != TimeMode::FreeRun
    }

    /// Every combination of the five fields, reachable or not.
    pub fn all() -> Vec<TransportState> {
        let mut out = Vec::new();
        for run in [Run::Stopped, Run::Playing, Run::WaitingForCode] {
            for time in [TimeMode::FreeRun, TimeMode::LockedFile, TimeMode::LockedExternal] {
                for output in [Output::Rehearse, Output::Positioning, Output::Armed] {
                    for record in [false, true] {
                        for estop in [false, true] {
                            out.push(TransportState { run, time, output, record, estop });
                        }
                    }
                }
            }
        }
        out
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Play,
    Stop,
    Cue,
    Seek,
    Lock,
    Unlock,
    Arm,
    Disarm,
    RecordOn,
    RecordOff,
    Estop,
    ResetEstop,
}

impl Action {
    pub const ALL: [Action; 12] = [
        Action::Play,
        Action::Stop,
        Action::Cue,
        Action::Seek,
        Action::Lock,
        Action::Unlock,
        Action::Arm,
        Action::Disarm,
        Action::RecordOn,
        Action::RecordOff,
        Action::Estop,
        Action::ResetEstop,
    ];

    /// Name used in URLs and in the `enabled` list.
    pub fn key(self) -> &'static str {
        match self {
            Action::Play => "play",
            Action::Stop => "stop",
            Action::Cue => "cue",
            Action::Seek => "seek",
            Action::Lock => "lock",
            Action::Unlock => "unlock",
            Action::Arm => "arm",
            Action::Disarm => "disarm",
            Action::RecordOn => "record_on",
            Action::RecordOff => "record_off",
            Action::Estop => "estop",
            Action::ResetEstop => "reset_estop",
        }
    }

    pub fn from_key(key: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.key() == key)
    }

    /// Every way the action is written in operator text.
    pub fn labels(self) -> &'static [&'static str] {
        match self {
            Action::Play => &["Play to code", "Play Live", "Rehearse", "Play"],
            Action::Stop => &["Stop"],
            Action::Cue => &["Cue"],
            Action::Seek => &["Seek"],
            Action::Lock => &["Lock"],
            Action::Unlock => &["Unlock"],
            Action::Arm => &["Arm"],
            Action::Disarm => &["Disarm"],
            Action::RecordOn => &["Record"],
            Action::RecordOff => &["Record off"],
            Action::Estop => &["E-Stop"],
            Action::ResetEstop => &["Reset E-Stop"],
        }
    }
}

/// Facts outside the five fields that gate actions.
#[derive(Clone, PartialEq, Debug)]
pub struct Context {
    pub has_lease: bool,
    pub song_loaded: bool,
    /// A committed revision is loaded and there are no edits since.
    pub committed_clean: bool,
    pub blocking_issues: usize,
    /// Why the rig cannot go live, for example "AutoPod is not calibrated".
    pub adapter_problem: Option<String>,
}

impl Context {
    /// A lease holder with a committed, clean, feasible song and a ready rig.
    pub fn ready() -> Context {
        Context { has_lease: true, song_loaded: true, committed_clean: true, blocking_issues: 0, adapter_problem: None }
    }
}

/// Why an action is refused. `names` lists the actions the text mentions; each
/// of them must be enabled in the same state.
#[derive(Clone, PartialEq, Debug)]
pub struct Blocked {
    pub text: String,
    pub names: Vec<Action>,
}

fn blocked(text: impl Into<String>, names: &[Action]) -> Result<(), Blocked> {
    Err(Blocked { text: text.into(), names: names.to_vec() })
}

/// Is `action` legal in this state?
pub fn check(s: &TransportState, ctx: &Context, action: Action) -> Result<(), Blocked> {
    use Action::*;
    if action == Estop {
        return Ok(());
    }
    if !ctx.has_lease {
        return blocked("Take the edit lease to control the show", &[]);
    }
    if !ctx.song_loaded {
        return blocked("Create or load a song first", &[]);
    }
    if s.estop {
        return match action {
            ResetEstop => Ok(()),
            _ => blocked("Reset E-Stop to continue", &[ResetEstop]),
        };
    }
    let positioning = s.output == Output::Positioning;
    match action {
        Estop => Ok(()),
        ResetEstop => blocked("Nothing to reset", &[]),
        Play => match s.run {
            Run::Playing => blocked("Already playing", &[]),
            Run::WaitingForCode => blocked("Already waiting for code", &[]),
            Run::Stopped if positioning => blocked("The rig is still moving to the playhead. Wait, or Disarm", &[Disarm]),
            Run::Stopped => Ok(()),
        },
        Stop => match s.run {
            Run::Stopped => blocked("Already stopped", &[]),
            _ => Ok(()),
        },
        Cue | Seek => {
            if s.time == TimeMode::LockedExternal {
                blocked("Unlock to cue", &[Unlock])
            } else if action == Seek && s.record && s.run != Run::Stopped {
                blocked("Stop to move the playhead while recording", &[Stop])
            } else {
                Ok(())
            }
        }
        Lock => {
            if s.locked() {
                blocked("Already locked. Unlock to choose another source", &[Unlock])
            } else if s.run != Run::Stopped {
                blocked("Stop before locking", &[Stop])
            } else if positioning {
                blocked("The rig is still moving to the playhead. Wait, or Disarm", &[Disarm])
            } else {
                Ok(())
            }
        }
        Unlock => {
            if s.locked() {
                Ok(())
            } else {
                blocked("Not locked to code", &[])
            }
        }
        Arm => {
            if s.output == Output::Armed {
                blocked("Already armed", &[])
            } else if positioning {
                blocked("Already arming: the rig is moving to the playhead", &[])
            } else if s.run != Run::Stopped {
                blocked("Stop before arming", &[Stop])
            } else if s.record {
                blocked("Turn Record off before arming", &[RecordOff])
            } else if !ctx.committed_clean {
                blocked("Commit the song first", &[])
            } else if ctx.blocking_issues > 0 {
                let n = ctx.blocking_issues;
                blocked(format!("Fix or accept {n} timeline issue{} first", if n == 1 { "" } else { "s" }), &[])
            } else if let Some(problem) = &ctx.adapter_problem {
                blocked(problem.clone(), &[])
            } else {
                Ok(())
            }
        }
        Disarm => {
            if s.output == Output::Rehearse {
                blocked("Not armed", &[])
            } else {
                Ok(())
            }
        }
        RecordOn => {
            if s.record {
                blocked("Already recording", &[])
            } else if s.output != Output::Rehearse {
                blocked("Disarm to record", &[Disarm])
            } else {
                Ok(())
            }
        }
        RecordOff => {
            if s.record {
                Ok(())
            } else {
                blocked("Not recording", &[])
            }
        }
    }
}

/// State after a legal action. `external_lock` only matters for `Lock`.
pub fn apply(s: &TransportState, action: Action, external_lock: bool) -> TransportState {
    let mut n = *s;
    match action {
        Action::Play => {
            n.run = if s.locked() { Run::WaitingForCode } else { Run::Playing };
        }
        Action::Stop => {
            n.run = Run::Stopped;
            n.record = false;
        }
        Action::Cue => {
            n.run = Run::Stopped;
            n.record = false;
            if s.output == Output::Armed {
                n.output = Output::Positioning;
            }
        }
        Action::Seek => {
            if s.output == Output::Armed {
                n.output = Output::Positioning;
            }
        }
        Action::Lock => {
            n.time = if external_lock { TimeMode::LockedExternal } else { TimeMode::LockedFile };
        }
        Action::Unlock => {
            n.time = TimeMode::FreeRun;
            if s.run == Run::WaitingForCode {
                n.run = Run::Stopped;
            }
        }
        Action::Arm => n.output = Output::Positioning,
        Action::Disarm => n.output = Output::Rehearse,
        Action::RecordOn => n.record = true,
        Action::RecordOff => n.record = false,
        Action::Estop => {
            n.estop = true;
            n.run = Run::Stopped;
            n.output = Output::Rehearse;
            n.record = false;
        }
        Action::ResetEstop => n.estop = false,
    }
    n
}

/// Things that happen without an operator action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// The rig reached the pose at the playhead.
    RigArrived,
    /// Code entered the song while waiting.
    CodeEntered,
    /// Code stopped or left the song while playing locked.
    CodeLost,
    /// Code jumped while armed; the rig has to travel to the new pose.
    Relocated,
    /// Free-run or file playback reached the end of the song.
    SongEnded,
    /// Tracking, limit or adapter fault while live.
    Fault,
}

impl Event {
    pub const ALL: [Event; 6] =
        [Event::RigArrived, Event::CodeEntered, Event::CodeLost, Event::Relocated, Event::SongEnded, Event::Fault];
}

pub fn on_event(s: &TransportState, event: Event) -> TransportState {
    let mut n = *s;
    if s.estop {
        return n;
    }
    match event {
        Event::RigArrived => {
            if s.output == Output::Positioning {
                n.output = Output::Armed;
            }
        }
        Event::CodeEntered => {
            if s.run == Run::WaitingForCode {
                n.run = Run::Playing;
            }
        }
        Event::CodeLost => {
            if s.run == Run::Playing && s.locked() {
                n.run = Run::WaitingForCode;
            }
        }
        Event::Relocated => {
            if s.output == Output::Armed {
                n.output = Output::Positioning;
            }
        }
        Event::SongEnded => {
            if s.run == Run::Playing && s.time != TimeMode::LockedExternal {
                n.run = Run::Stopped;
                n.record = false;
            }
        }
        Event::Fault => {
            if s.output != Output::Rehearse {
                n.output = Output::Rehearse;
                n.run = Run::Stopped;
            }
        }
    }
    n
}

/// What the app renders: the enabled actions, the reason for each blocked one,
/// the main button and a one-line hint.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Guidance {
    pub enabled: Vec<Action>,
    pub blocked: BTreeMap<Action, String>,
    pub primary: Option<Action>,
    pub primary_label: String,
    pub hint: String,
    /// Actions that `hint` or a blocked reason mentions. Not sent over the wire.
    #[serde(skip)]
    pub named: Vec<Action>,
}

impl Guidance {
    pub fn allows(&self, action: Action) -> bool {
        self.enabled.contains(&action)
    }

    pub fn reason(&self, action: Action) -> Option<&str> {
        self.blocked.get(&action).map(|s| s.as_str())
    }
}

pub fn guidance(s: &TransportState, ctx: &Context) -> Guidance {
    let mut g = Guidance::default();
    for action in Action::ALL {
        match check(s, ctx, action) {
            Ok(()) => g.enabled.push(action),
            Err(b) => {
                g.named.extend(b.names);
                g.blocked.insert(action, b.text);
            }
        }
    }

    let (primary, label, hint, names): (Action, &str, String, &[Action]) = if !ctx.has_lease {
        (Action::Estop, "E-Stop", "Watching. Take the edit lease to control the show.".into(), &[])
    } else if !ctx.song_loaded {
        (Action::Estop, "E-Stop", "Create or load a song to begin.".into(), &[])
    } else if s.estop {
        (
            Action::ResetEstop,
            "Reset E-Stop",
            "E-Stop is latched and output is held. Reset E-Stop when it is safe.".into(),
            &[Action::Estop, Action::ResetEstop],
        )
    } else if s.run != Run::Stopped {
        let hint = match (s.run, s.output, s.record) {
            (Run::WaitingForCode, _, _) => "Waiting for code to enter the song.",
            (_, Output::Rehearse, true) => "Recording the rig onto the enabled axes.",
            (_, Output::Rehearse, false) => "Rehearsing. Hardware does not move.",
            (_, Output::Positioning, _) => "Live. The rig is catching up with the playhead.",
            (_, Output::Armed, _) => "Live. Hardware is following the timeline.",
        };
        (Action::Stop, "Stop", hint.into(), &[])
    } else {
        match s.output {
            Output::Positioning => {
                (Action::Disarm, "Disarm", "Arming: the rig is moving to the pose at the playhead.".into(), &[])
            }
            Output::Armed => {
                if s.locked() {
                    (
                        Action::Play,
                        "Play to code",
                        "Armed. Play to code moves the rig when code enters the song.".into(),
                        &[Action::Play],
                    )
                } else {
                    (Action::Play, "Play Live", "Armed. Play Live moves the rig.".into(), &[Action::Play])
                }
            }
            Output::Rehearse => {
                if s.locked() {
                    (
                        Action::Play,
                        "Play to code",
                        "Play to code follows the locked source. Hardware does not move.".into(),
                        &[Action::Play],
                    )
                } else {
                    (
                        Action::Play,
                        "Rehearse",
                        "Rehearse plays audio and the monitor. Hardware does not move.".into(),
                        &[Action::Play],
                    )
                }
            }
        }
    };
    g.primary = Some(primary);
    g.primary_label = label.into();
    g.hint = hint;
    g.named.extend_from_slice(names);
    g.named.sort();
    g.named.dedup();
    g
}

/// Actions whose labels appear as whole words in `text`. Longer labels win, so
/// "Reset E-Stop" is not also counted as "E-Stop" or "Stop".
pub fn actions_named_in(text: &str) -> Vec<Action> {
    let mut labels: Vec<(&str, Action)> = Action::ALL.into_iter().flat_map(|a| a.labels().iter().map(move |l| (*l, a))).collect();
    labels.sort_by_key(|(l, _)| std::cmp::Reverse(l.len()));
    let mut text = text.to_string();
    let mut found = Vec::new();
    let word = |c: char| c.is_alphanumeric() || c == '-';
    for (label, action) in labels {
        let mut from = 0;
        while let Some(i) = text[from..].find(label) {
            let at = from + i;
            let end = at + label.len();
            let before = text[..at].chars().next_back().is_some_and(word);
            let after = text[end..].chars().next().is_some_and(word);
            if !before && !after {
                found.push(action);
                text.replace_range(at..end, &" ".repeat(label.len()));
            }
            from = end;
        }
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    fn contexts() -> Vec<Context> {
        let mut out = Vec::new();
        for has_lease in [true, false] {
            for song_loaded in [true, false] {
                for committed_clean in [true, false] {
                    for blocking_issues in [0, 1, 3] {
                        for adapter_problem in [None, Some("AutoPod is not calibrated".to_string())] {
                            out.push(Context { has_lease, song_loaded, committed_clean, blocking_issues, adapter_problem });
                        }
                    }
                }
            }
        }
        out
    }

    /// States reachable from idle through legal actions and events.
    fn reachable() -> HashSet<TransportState> {
        let ctx = Context::ready();
        let mut seen = HashSet::from([TransportState::IDLE]);
        let mut queue = VecDeque::from([TransportState::IDLE]);
        while let Some(s) = queue.pop_front() {
            let mut next = Vec::new();
            for a in Action::ALL {
                if check(&s, &ctx, a).is_ok() {
                    next.push(apply(&s, a, false));
                    next.push(apply(&s, a, true));
                }
            }
            for e in Event::ALL {
                next.push(on_event(&s, e));
            }
            for n in next {
                if seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        seen
    }

    #[test]
    fn guidance_names_only_enabled_actions_in_every_state() {
        for s in TransportState::all() {
            for ctx in contexts() {
                let g = guidance(&s, &ctx);
                let mut texts: Vec<&str> = g.blocked.values().map(|t| t.as_str()).collect();
                texts.push(&g.hint);
                let mut from_text = Vec::new();
                for t in texts {
                    from_text.extend(actions_named_in(t));
                }
                from_text.sort();
                from_text.dedup();
                assert_eq!(from_text, g.named, "declared names differ from text in {s:?}");
                for a in &g.named {
                    assert!(g.allows(*a), "{s:?} {ctx:?}: guidance names {a:?}, which is disabled");
                }
                let primary = g.primary.unwrap();
                assert!(g.allows(primary), "{s:?}: primary {primary:?} is disabled");
                assert!(actions_named_in(&g.primary_label).iter().all(|a| g.allows(*a)));
                assert_eq!(g.enabled.len() + g.blocked.len(), Action::ALL.len());
            }
        }
    }

    #[test]
    fn idle_is_reachable_from_every_state_without_estop() {
        let states = reachable();
        assert!(states.len() > 20);
        for ctx in contexts().into_iter().filter(|c| c.has_lease && c.song_loaded) {
            for start in &states {
                let mut seen = HashSet::from([*start]);
                let mut queue = VecDeque::from([*start]);
                let mut found = *start == TransportState::IDLE;
                while let Some(s) = queue.pop_front() {
                    if found {
                        break;
                    }
                    for a in Action::ALL {
                        if a == Action::Estop || check(&s, &ctx, a).is_err() {
                            continue;
                        }
                        for ext in [false, true] {
                            let n = apply(&s, a, ext);
                            if n == TransportState::IDLE {
                                found = true;
                            }
                            if seen.insert(n) {
                                queue.push_back(n);
                            }
                        }
                    }
                }
                assert!(found, "dead end: no way to idle from {start:?} with {ctx:?}");
            }
        }
    }

    #[test]
    fn every_non_idle_state_has_a_way_forward() {
        // Stronger than reachability: some enabled action other than E-Stop
        // always changes the state.
        let ctx = Context::ready();
        for s in reachable() {
            let moves = Action::ALL
                .into_iter()
                .filter(|a| *a != Action::Estop && check(&s, &ctx, *a).is_ok())
                .any(|a| apply(&s, a, false) != s);
            assert!(moves, "{s:?} has no action that changes anything");
        }
    }

    #[test]
    fn hardware_live_only_when_positioning_or_armed() {
        for s in TransportState::all() {
            let expect = !s.estop && matches!(s.output, Output::Positioning | Output::Armed);
            assert_eq!(s.hardware_live(), expect);
        }
        // No reachable state is live with E-Stop latched or Record on.
        for s in reachable() {
            if s.estop {
                assert_eq!(s.output, Output::Rehearse);
                assert_eq!(s.run, Run::Stopped);
            }
            if s.record {
                assert_eq!(s.output, Output::Rehearse, "recording while live in {s:?}");
            }
        }
    }

    #[test]
    fn rehearse_and_arm_paths() {
        let ctx = Context::ready();
        let s = TransportState::IDLE;
        assert_eq!(guidance(&s, &ctx).primary_label, "Rehearse");
        let playing = apply(&s, Action::Play, false);
        assert_eq!(playing.run, Run::Playing);
        assert!(!playing.hardware_live());
        assert!(check(&playing, &ctx, Action::Arm).is_err());

        let arming = apply(&s, Action::Arm, false);
        assert_eq!(arming.output, Output::Positioning);
        assert!(arming.hardware_live());
        assert!(check(&arming, &ctx, Action::Play).is_err());
        assert!(check(&arming, &ctx, Action::Disarm).is_ok());
        let armed = on_event(&arming, Event::RigArrived);
        assert_eq!(guidance(&armed, &ctx).primary_label, "Play Live");
        let live = apply(&armed, Action::Play, false);
        assert!(check(&live, &ctx, Action::Disarm).is_ok());
        assert!(check(&live, &ctx, Action::RecordOn).is_err());
        let paused = apply(&live, Action::Stop, false);
        assert_eq!(paused.output, Output::Armed);
        assert_eq!(apply(&paused, Action::Disarm, false), TransportState::IDLE);
    }

    #[test]
    fn arm_is_refused_for_each_missing_precondition() {
        let s = TransportState::IDLE;
        let reason = |ctx: Context| check(&s, &ctx, Action::Arm).unwrap_err().text;
        assert!(reason(Context { has_lease: false, ..Context::ready() }).contains("lease"));
        assert!(reason(Context { committed_clean: false, ..Context::ready() }).contains("Commit"));
        assert!(reason(Context { blocking_issues: 2, ..Context::ready() }).contains("2 timeline issues"));
        assert!(reason(Context { adapter_problem: Some("AutoPod is not calibrated".into()), ..Context::ready() })
            .contains("calibrated"));
        let rec = apply(&s, Action::RecordOn, false);
        assert!(check(&rec, &Context::ready(), Action::Arm).unwrap_err().text.contains("Record off"));
        let latched = apply(&s, Action::Estop, false);
        assert!(check(&latched, &Context::ready(), Action::Arm).unwrap_err().text.contains("Reset E-Stop"));
    }

    #[test]
    fn lock_play_to_code_and_record_while_locked() {
        let ctx = Context::ready();
        let locked = apply(&TransportState::IDLE, Action::Lock, true);
        assert_eq!(locked.time, TimeMode::LockedExternal);
        assert_eq!(check(&locked, &ctx, Action::Cue).unwrap_err().text, "Unlock to cue");
        assert_eq!(guidance(&locked, &ctx).primary_label, "Play to code");
        let waiting = apply(&locked, Action::Play, true);
        assert_eq!(waiting.run, Run::WaitingForCode);
        let rolling = on_event(&waiting, Event::CodeEntered);
        assert_eq!(rolling.run, Run::Playing);
        let rec = apply(&rolling, Action::RecordOn, true);
        assert!(rec.record && !rec.hardware_live());
        // Code stops: playback waits and resumes when code returns.
        let held = on_event(&rolling, Event::CodeLost);
        assert_eq!(held.run, Run::WaitingForCode);
        assert_eq!(on_event(&held, Event::CodeEntered).run, Run::Playing);
        // Unlock while playing continues in free-run.
        let free = apply(&rolling, Action::Unlock, false);
        assert_eq!((free.run, free.time), (Run::Playing, TimeMode::FreeRun));

        let file = apply(&TransportState::IDLE, Action::Lock, false);
        assert!(check(&file, &ctx, Action::Cue).is_ok());
    }

    #[test]
    fn estop_always_works_and_latches() {
        for s in TransportState::all() {
            for ctx in contexts() {
                assert!(check(&s, &ctx, Action::Estop).is_ok());
                let n = apply(&s, Action::Estop, false);
                assert!(n.estop && !n.hardware_live());
                if ctx.has_lease && ctx.song_loaded {
                    for a in Action::ALL {
                        let ok = check(&n, &ctx, a).is_ok();
                        assert_eq!(ok, matches!(a, Action::Estop | Action::ResetEstop), "{a:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn label_matching_respects_word_boundaries() {
        assert_eq!(actions_named_in("Reset E-Stop to continue"), vec![Action::ResetEstop]);
        assert_eq!(actions_named_in("Unlock to cue"), vec![Action::Unlock]);
        assert_eq!(actions_named_in("Disarm to record"), vec![Action::Disarm]);
        assert_eq!(actions_named_in("Turn Record off before arming"), vec![Action::RecordOff]);
        assert_eq!(actions_named_in("Rehearsing. Hardware does not move."), vec![]);
        assert_eq!(actions_named_in("Stop, then Play Live"), vec![Action::Play, Action::Stop]);
    }
}
