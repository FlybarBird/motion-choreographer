use ptz_panel::codec::{Color, Lit, Text, Trigger};
use ptz_panel::fake::FakePanel;
use ptz_panel::map::{Needs, PanelMap, Role};
use ptz_panel::tool::Session;
use ptz_panel::topology::{self, Input, Topology};
use ptz_panel::{with_port, Options, DEFAULT_STATION, RAW_PANEL_PORT};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

const HELP: &str = "\
ptz-panel: run PTZ Programmer from a SKAARHOJ panel over the Raw Panel protocol

Usage:
  ptz-panel --panel <address> [options]     connect to the panel and run
  ptz-panel --listen <addr:port> [options]  wait for a panel that dials in
  ptz-panel map [--map <file>] [--panel <address>]
                                            print the control map; with --panel,
                                            check it against that panel
  ptz-panel learn --panel <address> [--map <file>] [role ...]
                                            bind controls by pressing them, and
                                            write the map file
  ptz-panel fake [--listen <addr:port>]     a stand-in PTZ Pro in this terminal

Options:
  --server <url>        The show server (default http://127.0.0.1:8787)
  --access-key <key>    The server's access key (or set PTZ_ACCESS_KEY)
  --station <name>      The name the panel holds the edit lease under
                        (default Panel)
  --panel <address>     The panel, in Raw Panel mode. Port 9923 unless given
  --listen <addr:port>  Wait here for a panel set to dial in
  --map <file>          The control map (default: panel.json in this folder
                        if there is one, otherwise the PTZ Pro layout)
  --lease-grace <s>     Seconds a lost panel keeps the edit lease (default 30)
  --version, --help

The server can also run the panel itself: ptz-server --panel <address>.
";

const FAKE_HELP: &str = "\
Type a command and Enter. Controls are named by their number; `controls` lists them.
  press N           press and let go
  hold N SECONDS    keep it down that long
  down N, up N      one at a time, for Shift and for holds you time yourself
  turn N STEPS      an encoder; negative is counter-clockwise
  joy N VALUE       a joystick axis, -500 to 500; 0 is rest
  show              everything the panel shows now
  controls          every control and its number
  silent on|off     stop answering without closing the connection, and answer again
  drop              close the connection
  off, on           power the panel off and on
  help, quit
";

fn fail(msg: &str) -> ! {
    eprintln!("ptz-panel: {msg}");
    std::process::exit(2);
}

#[derive(Default)]
struct Args {
    server: Option<String>,
    access_key: Option<String>,
    station: Option<String>,
    panel: Option<String>,
    listen: Option<String>,
    map: Option<PathBuf>,
    lease_grace: Option<f64>,
    words: Vec<String>,
}

/// Read the options a command takes. Anything else is refused by name.
fn parse(command: &str, allowed: &[&str], words_allowed: bool, args: impl Iterator<Item = String>) -> Args {
    let mut out = Args::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            print!("{HELP}");
            std::process::exit(0);
        }
        if arg == "--version" {
            println!("ptz-panel {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        if !arg.starts_with("--") {
            if !words_allowed {
                fail(&format!("{arg} is not understood here. Try --help"));
            }
            out.words.push(arg);
            continue;
        }
        if !allowed.contains(&arg.as_str()) {
            let of = if command.is_empty() { String::new() } else { format!(" of ptz-panel {command}") };
            fail(&format!("{arg} is not an option{of}. Try --help"));
        }
        let value = args.next().unwrap_or_else(|| fail(&format!("{arg} needs a value")));
        match arg.as_str() {
            "--server" => out.server = Some(value),
            "--access-key" => out.access_key = Some(value),
            "--station" => out.station = Some(value),
            "--panel" => out.panel = Some(with_port(&value)),
            "--listen" => out.listen = Some(value),
            "--map" => out.map = Some(PathBuf::from(value)),
            _ => match value.parse::<f64>() {
                Ok(s) if s.is_finite() && s >= 0.0 => out.lease_grace = Some(s),
                _ => fail("--lease-grace needs a number of seconds, 0 or more"),
            },
        }
    }
    out
}

/// The map file to use and whether it was asked for by name.
fn map_path(args: &Args) -> (PathBuf, bool) {
    match &args.map {
        Some(path) => (path.clone(), true),
        None => (PathBuf::from("panel.json"), false),
    }
}

fn load_map(args: &Args) -> (PanelMap, String) {
    let (path, named) = map_path(args);
    if named && !path.exists() {
        fail(&format!("{} does not exist. Leave --map out to use the PTZ Pro layout", path.display()));
    }
    let map = PanelMap::load(&path).unwrap_or_else(|e| fail(&e));
    let from = if path.exists() {
        format!("the PTZ Pro layout with the changes in {}", path.display())
    } else {
        "the PTZ Pro layout".to_string()
    };
    (map, from)
}

fn run(args: Args) {
    let (map, from) = load_map(&args);
    for p in map.problems(None) {
        eprintln!("ptz-panel: map: {}", p.text);
    }
    let server = args.server.unwrap_or_else(|| format!("http://127.0.0.1:{}", ptz_core::api::DEFAULT_PORT));
    let mut options = Options::new(&server);
    options.access_key = args.access_key.or_else(|| std::env::var("PTZ_ACCESS_KEY").ok()).filter(|k| !k.is_empty());
    options.station = args.station.unwrap_or_else(|| DEFAULT_STATION.to_string());
    options.panel = args.panel.clone();
    options.listen = args.listen.clone();
    options.map = map;
    if let Some(s) = args.lease_grace {
        options.lease_grace = Duration::from_secs_f64(s);
    }
    let station = options.station.trim().to_string();
    match ptz_panel::start(options) {
        Ok(running) => {
            println!("ptz-panel {}: station \"{station}\" on the show server at {server}", env!("CARGO_PKG_VERSION"));
            if let Some(panel) = &args.panel {
                println!("panel: connecting to {panel}");
            }
            if let Some(addr) = running.listening {
                println!("panel: waiting for one to dial in on {addr}");
            }
            println!("control map: {from}");
            told_to_stop();
            // The lease is given back and the panel told, but a controller that is slow
            // to stop does not keep the process alive.
            let (done, stopped) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut running = running;
                running.stop();
                let _ = done.send(());
            });
            let _ = stopped.recv_timeout(Duration::from_secs(2));
        }
        Err(e) => fail(&e),
    }
}

/// Block until Ctrl-C, or the signals `kill` and a closing terminal send.
fn told_to_stop() {
    let forever = || loop {
        std::thread::park();
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_io().build() else { forever() };
    let heard = rt.block_on(async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            if let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) {
                return tokio::select! {
                    r = tokio::signal::ctrl_c() => r.is_ok(),
                    _ = term.recv() => true,
                    _ = hup.recv() => true,
                };
            }
        }
        tokio::signal::ctrl_c().await.is_ok()
    });
    if !heard {
        forever()
    }
}

fn describe_panel(s: &Session, addr: &str) -> String {
    let what = match (s.model.is_empty(), s.name.is_empty()) {
        (false, false) => format!("{} \"{}\"", s.model, s.name),
        (false, true) => s.model.clone(),
        _ => "a panel that did not give its model".to_string(),
    };
    format!("{what} at {addr}")
}

fn map_command(args: Args) {
    let (map, from) = load_map(&args);
    let (topology, against) = match &args.panel {
        Some(addr) => {
            let s = Session::open(addr, Duration::from_secs(5)).unwrap_or_else(|e| fail(&e));
            let against = describe_panel(&s, addr);
            match s.topology {
                Some(t) => (t, against),
                None => fail(&format!(
                    "{against} answered but did not describe its controls, so the map cannot be checked against it"
                )),
            }
        }
        None => (
            topology::ptz_pro(),
            "the PTZ Pro's published layout. No panel was asked: add --panel to check a real one".to_string(),
        ),
    };
    println!("Control map: {from}");
    println!("Checked against: {against}\n");
    print!("{}", map.table(Some(&topology)));
    let problems = map.problems(Some(&topology));
    if problems.is_empty() {
        println!("\nThe map fits this panel.");
        return;
    }
    println!("\n{} problem(s):", problems.len());
    for p in &problems {
        println!("  {}", p.text);
    }
    println!("Fix them in the map file, or run ptz-panel learn.");
    std::process::exit(1);
}

fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn how_to_show(role: Role) -> &'static str {
    match role {
        Role::JoyPan => "push the joystick fully to the right",
        Role::JoyTilt => "push the joystick fully up",
        Role::JoyZoom => "twist the joystick fully clockwise",
        Role::ZoomRocker => "press the zoom-in end of the rocker fully",
        _ => match role.needs() {
            Needs::Encoder => "turn it",
            _ => "press it",
        },
    }
}

fn label_of(topology: Option<&Topology>, hwc: u32) -> String {
    match topology.and_then(|t| t.get(hwc)) {
        Some(c) => format!("component {hwc}, {} ({})", c.label, c.input.word()),
        None => format!("component {hwc}"),
    }
}

fn set_inverted(map: &mut PanelMap, role: Role, inverted: bool) {
    let axes = [Role::JoyPan, Role::JoyTilt, Role::JoyZoom, Role::ZoomRocker];
    let list: Vec<String> =
        axes.into_iter().filter(|r| if *r == role { inverted } else { map.inverted(*r) }).map(|r| r.key()).collect();
    if let Err(e) = map.apply_json(&serde_json::json!({ "invert": list }).to_string()) {
        fail(&e);
    }
}

fn learn(args: Args) {
    let Some(addr) = args.panel.clone() else { fail("learn needs the panel's address: ptz-panel learn --panel <address>") };
    let (path, _) = map_path(&args);
    let mut map = PanelMap::load(&path).unwrap_or_else(|e| fail(&e));
    let roles: Vec<Role> = if args.words.is_empty() {
        Role::all().into_iter().filter(|r| r.needs() != Needs::Display).collect()
    } else {
        args.words
            .iter()
            .map(|w| match Role::from_key(w) {
                Some(r) if r.needs() != Needs::Display => r,
                Some(_) => fail(&format!("{w} is a display. Displays cannot be pressed: set them in {} by hand", path.display())),
                None => fail(&format!("there is no role \"{w}\". Run ptz-panel map for the list")),
            })
            .collect()
    };
    let mut s = Session::open(&addr, Duration::from_secs(5)).unwrap_or_else(|e| fail(&e));
    println!("Connected to {}.", describe_panel(&s, &addr));
    println!("For each role, do what is asked on the panel. Or type Enter to keep it as it is, - to unbind it, q to stop.\n");
    let _ = s.clear();
    let typed = stdin_lines();
    let mut stopped = false;
    'roles: for role in roles {
        let now = match map.hwc(role) {
            Some(hwc) => format!("now {}", label_of(s.topology.as_ref(), hwc)),
            None => "not bound".to_string(),
        };
        println!("{}: {}. To bind it, {} ({now})", role.key(), role.describe(), how_to_show(role));
        if let Err(e) = s.drain() {
            fail(&e);
        }
        loop {
            match typed.try_recv() {
                Ok(line) => match line.trim() {
                    "" => {
                        println!("  kept");
                        continue 'roles;
                    }
                    "-" => {
                        map.bind(role, None);
                        println!("  unbound");
                        continue 'roles;
                    }
                    "q" => {
                        stopped = true;
                        break 'roles;
                    }
                    other => println!("  \"{other}\" is not understood. Enter keeps it, - unbinds it, q stops"),
                },
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    stopped = true;
                    break 'roles;
                }
            }
            let Some((hwc, trigger)) = s.next_event(Duration::from_millis(100)).unwrap_or_else(|e| fail(&e)) else { continue };
            let fits = match (role.needs(), &trigger) {
                (Needs::Button, Trigger::Down | Trigger::Press) => Some(false),
                (Needs::Encoder, Trigger::Enc(_)) => Some(false),
                (Needs::Axis, Trigger::Speed(v)) if v.abs() >= 300 => Some(*v < 0),
                _ => None,
            };
            let Some(inverted) = fits else { continue };
            map.bind(role, Some(hwc));
            if role.needs() == Needs::Axis {
                set_inverted(&mut map, role, inverted);
            }
            println!("  bound to {}{}", label_of(s.topology.as_ref(), hwc), if inverted { ", inverted" } else { "" });
            let text = Text { title: "Bound".into(), label: true, line1: role.key(), ..Text::default() };
            let _ = s.show(hwc, Lit::On, Color::Green, &text);
            break;
        }
    }
    let _ = s.clear();
    if let Err(e) = std::fs::write(&path, map.to_json()) {
        fail(&format!("{} could not be written: {e}", path.display()));
    }
    println!("\n{}Wrote {}.", if stopped { "Stopped. " } else { "" }, path.display());
    let problems = map.problems(s.topology.as_ref());
    if problems.is_empty() {
        println!("The map fits this panel.");
    } else {
        println!("{} problem(s) remain:", problems.len());
        for p in &problems {
            println!("  {}", p.text);
        }
        std::process::exit(1);
    }
}

fn lit_word(lit: Lit) -> &'static str {
    match lit {
        Lit::Off => "off",
        Lit::Dim => "dim",
        Lit::On => "on",
        Lit::Blink => "blinking",
    }
}

fn color_word(color: Option<Color>) -> &'static str {
    match color {
        None | Some(Color::Default) => "",
        Some(Color::White) => "white",
        Some(Color::Red) => "red",
        Some(Color::Amber) => "amber",
        Some(Color::Blue) => "blue",
        Some(Color::Green) => "green",
    }
}

fn cell_line(panel: &FakePanel, hwc: u32, cell: &ptz_panel::fake::Cell) -> String {
    let label = panel.topology().get(hwc).map(|c| c.label.clone()).unwrap_or_default();
    let light = format!("{} {}", lit_word(cell.lit), color_word(cell.color));
    let t = &cell.text;
    let text: Vec<&str> = [t.title.as_str(), t.line1.as_str(), t.line2.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    format!("  [{hwc:>3}] {label:<10} {:<14} {}", light.trim(), text.join(" | "))
}

fn number(word: Option<&str>, what: &str) -> Result<i64, String> {
    word.and_then(|w| w.parse::<i64>().ok()).ok_or_else(|| format!("{what} is missing or is not a number"))
}

fn control(panel: &FakePanel, word: Option<&str>, wants: &str) -> Result<u32, String> {
    let n = number(word, "The control's number")?;
    let found = u32::try_from(n).ok().and_then(|n| panel.topology().get(n).map(|c| (n, c.input)));
    let Some((hwc, input)) = found else {
        return Err(format!("The panel has no control {n}. Type controls for the list"));
    };
    let fits = match wants {
        "button" => matches!(input, Input::Button | Input::Encoder { push: true }),
        "encoder" => matches!(input, Input::Encoder { .. }),
        _ => matches!(input, Input::Intensity),
    };
    if fits {
        Ok(hwc)
    } else {
        Err(format!("{} is not a {wants}", panel.label(hwc)))
    }
}

fn fake_command(panel: &FakePanel, line: &str) -> Result<bool, String> {
    let mut words = line.split_whitespace();
    let Some(command) = words.next() else { return Ok(true) };
    match command {
        "press" => panel.press(control(panel, words.next(), "button")?),
        "down" => panel.down(control(panel, words.next(), "button")?),
        "up" => panel.up(control(panel, words.next(), "button")?),
        "hold" => {
            let hwc = control(panel, words.next(), "button")?;
            let seconds = words.next().and_then(|w| w.parse::<f64>().ok()).filter(|s| (0.0..=60.0).contains(s));
            let seconds = seconds.ok_or("Give the seconds to hold it, up to 60")?;
            panel.hold(hwc, Duration::from_secs_f64(seconds));
        }
        "turn" => {
            let hwc = control(panel, words.next(), "encoder")?;
            panel.turn(hwc, number(words.next(), "The number of steps")?.clamp(-100, 100) as i32);
        }
        "joy" => {
            let hwc = control(panel, words.next(), "joystick axis")?;
            panel.deflect(hwc, number(words.next(), "The value")?.clamp(-500, 500) as i32);
        }
        "show" => {
            for (hwc, cell) in panel.cells() {
                println!("{}", cell_line(panel, hwc, &cell));
            }
        }
        "controls" => {
            for (hwc, label, input) in panel.controls() {
                println!("  [{hwc:>3}] {label:<10} {}", input.word());
            }
        }
        "silent" => match words.next() {
            Some("on") => panel.set_silent(true),
            Some("off") => panel.set_silent(false),
            _ => return Err("silent on, or silent off".into()),
        },
        "drop" => panel.drop_connections(),
        "off" => panel.power_off(),
        "on" => panel.power_on(),
        "help" => print!("{FAKE_HELP}"),
        "quit" | "exit" | "q" => return Ok(false),
        other => return Err(format!("\"{other}\" is not a command. Type help")),
    }
    Ok(true)
}

fn fake(args: Args) {
    let listen = args.listen.unwrap_or_else(|| format!("127.0.0.1:{RAW_PANEL_PORT}"));
    let panel = FakePanel::start_on(&listen).unwrap_or_else(|e| fail(&format!("Cannot listen on {listen}: {e}")));
    println!(
        "A stand-in PTZ Pro is listening on {}. It is not a real panel: it shows what it is sent and sends what you type.",
        panel.addr
    );
    println!("Connect to it with: ptz-panel --panel {}   or   ptz-server --panel {}\n", panel.addr, panel.addr);
    print!("{FAKE_HELP}");
    let typed = stdin_lines();
    let mut shown: BTreeMap<u32, ptz_panel::fake::Cell> = BTreeMap::new();
    let mut seen = u64::MAX;
    let mut connected = 0;
    loop {
        match typed.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => match fake_command(&panel, &line) {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => println!("{e}"),
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = panel.connections();
        if now != connected {
            println!("{}", if now > 0 { "-- a system connected" } else { "-- the system is gone" });
            connected = now;
        }
        if panel.changes() != seen {
            seen = panel.changes();
            let cells = panel.cells();
            for (hwc, cell) in &cells {
                if shown.get(hwc) != Some(cell) {
                    println!("{}", cell_line(&panel, *hwc, cell));
                }
            }
            for hwc in shown.keys().filter(|h| !cells.contains_key(h)) {
                println!("  [{hwc:>3}] cleared");
            }
            shown = cells;
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let command = match args.peek().map(String::as_str) {
        Some("map" | "learn" | "fake") => args.next().unwrap_or_default(),
        _ => String::new(),
    };
    match command.as_str() {
        "map" => map_command(parse("map", &["--map", "--panel"], false, args)),
        "learn" => learn(parse("learn", &["--map", "--panel"], true, args)),
        "fake" => fake(parse("fake", &["--listen"], false, args)),
        _ => {
            let all = ["--server", "--access-key", "--station", "--panel", "--listen", "--map", "--lease-grace"];
            let parsed = parse("", &all, false, args);
            if parsed.panel.is_none() && parsed.listen.is_none() {
                eprint!("{HELP}");
                fail("give the panel's address with --panel, or an address to wait on with --listen");
            }
            run(parsed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_read_and_the_map_defaults_to_panel_json_here() {
        let words = ["--server", "http://10.0.0.5:8787", "--panel", "10.0.0.9", "--station", "Stage left", "--lease-grace", "5"];
        let all = ["--server", "--access-key", "--station", "--panel", "--listen", "--map", "--lease-grace"];
        let a = parse("", &all, false, words.iter().map(|s| s.to_string()));
        assert_eq!(a.server.as_deref(), Some("http://10.0.0.5:8787"));
        assert_eq!(a.panel.as_deref(), Some("10.0.0.9:9923"));
        assert_eq!(a.station.as_deref(), Some("Stage left"));
        assert_eq!(a.lease_grace, Some(5.0));
        let (path, named) = map_path(&a);
        assert_eq!((path, named), (PathBuf::from("panel.json"), false));
    }
}
