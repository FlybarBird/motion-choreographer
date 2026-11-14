//! On-site bring-up of the FR7 and AutoPod+. Talks to the devices directly, not
//! through the show server: stop the server, or set its rig to Simulator, first.

use ptz_core::client::Api;
use ptz_server::bringup::{self, Operator, Options, Report, Step, Unattended};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use ptz_server::hw::fr7::Fr7Config;
use ptz_server::hw::{visca, with_port};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const HELP: &str = "\
ptz-bringup: measure the real rig and write the values the project needs

Usage:
  ptz-bringup --fr7 <host[:port]> --autopod <host[:port]> [options]
  ptz-bringup --apply <report.json> [--server http://127.0.0.1:8787] [--access-key <key>]
  ptz-bringup fakes [--fr7 127.0.0.1:52381] [--fr7-http 127.0.0.1:8088] [--autopod 127.0.0.1:8089]
                    [--picture <file.jpg>]

Checklist options:
  --fr7 <host[:port]>      The FR7's address. VISCA over IP is UDP port 52381.
  --fr7-http <port>        Port of the FR7's web interface (default 80)
  --user <name>            Web interface login, for preset names and pictures
  --password <password>    (or the PTZ_FR7_PASSWORD environment variable)
  --autopod <host[:port]>  The AutoPod leg box's address (default port 80)
  --only <steps>           Comma-separated steps to run, in this order:
                           connect, moves, timing, speeds, tracking, autopod, estop
  --from <report.json>     Start from an earlier report's findings, to repeat one step
  --report <file>          Where to write the report (default bringup-<date>.json)
  --quick                  Shorter runs: five speed steps, not all 24
  --yes                    Do not ask before moving, and take every default.
                           The height map is skipped: it needs someone to measure.

Every step that moves the rig says what it will do and asks first. It moves
from wherever the rig stands and brings it back. Keep a hand on the camera's
own stop: this tool is what measures how long a stop takes.

--apply takes the edit lease, writes the report's values into the server's
project and releases the lease. Top speeds are only ever lowered.

`fakes` runs stand-ins for both devices on this machine, to try the software
against. The fake FR7's web login is admin / fr7-password. --picture gives it
a JPEG to serve as every preset's picture.
";

fn fail(msg: &str) -> ! {
    eprintln!("ptz-bringup: {msg}");
    std::process::exit(2);
}

struct Console;

impl Console {
    fn ask(prompt: &str) -> String {
        print!("{prompt} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            // No terminal to answer: treat it as a refusal, never as consent to move.
            println!();
            return "n".into();
        }
        line.trim().to_string()
    }
}

impl Operator for Console {
    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn confirm(&mut self, question: &str) -> bool {
        loop {
            match Console::ask(&format!("  {question} [y/n]")).to_lowercase().as_str() {
                "y" | "yes" => return true,
                "n" | "no" => return false,
                _ => println!("  Answer y or n."),
            }
        }
    }

    fn number(&mut self, question: &str, default: Option<f64>) -> Option<f64> {
        loop {
            let hint = match default {
                Some(d) => format!("[{d}]"),
                None => "[Enter to skip]".to_string(),
            };
            let answer = Console::ask(&format!("  {question} {hint}"));
            if answer.is_empty() || answer == "n" {
                return default;
            }
            match answer.parse::<f64>() {
                Ok(v) if v.is_finite() => return Some(v),
                _ => println!("  That is not a number."),
            }
        }
    }
}

fn apply(path: &Path, server: &str, key: Option<&str>) {
    let report = Report::load(path).unwrap_or_else(|e| fail(&e));
    let api = Api::new(server, "ptz-bringup", key);
    let mut project = api.project().unwrap_or_else(|e| fail(&format!("{server}: {}", e.message())));
    let changes = report.settings.apply(&mut project);
    if changes.is_empty() {
        println!("The project already holds every value in {}.", path.display());
        return;
    }
    if let Err(e) = api.take_lease("ptz-bringup", false) {
        fail(&format!("could not take the edit lease: {}", e.message()));
    }
    let result = api.put_project(&project);
    let _ = api.release_lease();
    match result {
        Ok(_) => {
            println!("Applied {} to the project at {server}:", path.display());
            for c in changes {
                println!("  {c}");
            }
            println!("The rig kind is not changed. Switch it to Hardware in the app's rig settings when the rig is ready.");
        }
        Err(e) => fail(&format!("the server refused the project: {}", e.message())),
    }
}

fn fakes(mut args: impl Iterator<Item = String>) {
    let (mut visca_at, mut http_at, mut pod_at) =
        (format!("127.0.0.1:{}", visca::PORT), "127.0.0.1:8088".to_string(), "127.0.0.1:8089".to_string());
    let mut picture = None::<PathBuf>;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(&format!("{name} needs a value")));
        match arg.as_str() {
            "--fr7" => visca_at = value("--fr7"),
            "--fr7-http" => http_at = value("--fr7-http"),
            "--autopod" => pod_at = value("--autopod"),
            "--picture" => picture = Some(PathBuf::from(value("--picture"))),
            other => fail(&format!("unknown option {other} for fakes")),
        }
    }
    let fr7 =
        FakeFr7::start_on(&visca_at, &http_at).unwrap_or_else(|e| fail(&format!("fake FR7 on {visca_at} and {http_at}: {e}")));
    if let Some(path) = picture {
        fr7.set_picture(std::fs::read(&path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display()))));
    }
    let pod = FakeAutoPod::start_on(&pod_at).unwrap_or_else(|e| fail(&format!("fake AutoPod+ on {pod_at}: {e}")));
    println!(
        "Fake FR7:     VISCA over IP on {} (UDP), web interface on {} (login {} / {})",
        fr7.visca, fr7.http, fr7.user, fr7.password
    );
    println!("Fake AutoPod+: {} (calibrated; height map in raw counts: {:?})", pod.addr, FakeAutoPod::height_map());
    println!("These are stand-ins, not a rig. Ctrl-C to stop.\n");
    let mut shown = String::new();
    loop {
        let (pan, tilt, zoom, focus) = fr7.pose();
        let line =
            format!("pan {pan:7.2}  tilt {tilt:6.2}  zoom {zoom:.3}  focus {focus:.3}  elevation {:6.1} mm", pod.height_mm());
        if line != shown {
            println!("{line}");
            shown = line;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("fakes") {
        args.next();
        return fakes(args);
    }
    let (mut fr7_host, mut fr7_http, mut user, mut autopod) = (None::<String>, 80u16, String::new(), None::<String>);
    let mut password = std::env::var("PTZ_FR7_PASSWORD").unwrap_or_default();
    let (mut steps, mut quick, mut yes) = (Vec::new(), false, false);
    let (mut report_path, mut from, mut apply_path) = (None::<PathBuf>, None::<PathBuf>, None::<PathBuf>);
    let mut server = format!("http://127.0.0.1:{}", ptz_core::api::DEFAULT_PORT);
    let mut key = std::env::var("PTZ_ACCESS_KEY").ok();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(&format!("{name} needs a value")));
        match arg.as_str() {
            "--fr7" => fr7_host = Some(value("--fr7")),
            "--fr7-http" => fr7_http = value("--fr7-http").parse().unwrap_or_else(|_| fail("--fr7-http needs a port number")),
            "--user" => user = value("--user"),
            "--password" => password = value("--password"),
            "--autopod" => autopod = Some(value("--autopod")),
            "--only" => {
                for name in value("--only").split(',') {
                    steps.push(Step::parse(name).unwrap_or_else(|| fail(&format!("there is no step called {name}"))));
                }
            }
            "--from" => from = Some(PathBuf::from(value("--from"))),
            "--report" => report_path = Some(PathBuf::from(value("--report"))),
            "--quick" => quick = true,
            "--yes" => yes = true,
            "--apply" => apply_path = Some(PathBuf::from(value("--apply"))),
            "--server" => server = value("--server"),
            "--access-key" => key = Some(value("--access-key")),
            "--version" => {
                println!("ptz-bringup {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return;
            }
            other => fail(&format!("unknown option {other}; see --help")),
        }
    }
    if let Some(path) = apply_path {
        return apply(&path, &server, key.as_deref());
    }
    if fr7_host.is_none() && autopod.is_none() {
        print!("{HELP}");
        std::process::exit(2);
    }

    let start = match &from {
        Some(path) => Report::load(path).unwrap_or_else(|e| fail(&e)).settings,
        None => Default::default(),
    };
    let fr7 = fr7_host.map(|host| {
        let host = host.trim().to_string();
        let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host).to_string();
        Fr7Config {
            visca: with_port(&host, visca::PORT),
            http: format!("{name}:{fr7_http}"),
            user,
            password,
            counts_per_degree: 236.0,
            pan_sign: 1.0,
            tilt_sign: 1.0,
            absolute_moves: false,
            speed_steps: Vec::new(),
        }
    });
    let opts = Options { fr7, autopod: autopod.map(|h| with_port(h.trim(), 80)), steps, quick, start };

    println!("PTZ Programmer bring-up. The show server must not be driving this rig while this runs.");
    let report = if yes { bringup::run(&opts, &mut Unattended { echo: true }) } else { bringup::run(&opts, &mut Console) };
    print!("{}", report.summary());
    let path = report_path.unwrap_or_else(|| PathBuf::from(format!("bringup-{}.json", &report.made_at[..10])));
    match report.save(&path) {
        Ok(()) => {
            println!("\nReport written to {}.", path.display());
            println!("To put these values into the project: ptz-bringup --apply {} --server {server}", path.display());
        }
        Err(e) => fail(&e),
    }
    if report.failed() {
        std::process::exit(1);
    }
}
