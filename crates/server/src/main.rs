use ptz_server::engine::{AudioChoice, Config};
use ptz_server::{audio, http, runlog};
use std::path::PathBuf;

const HELP: &str = "\
ptz-server: the PTZ Programmer show server

Usage: ptz-server [options]

  --data <dir>          Data folder (default ~/PTZProgrammer)
  --bind <addr:port>    Address to listen on (default 127.0.0.1:8787)
  --access-key <key>    Require this key on every request. Needed for any
                        address other than loopback.
  --no-seed             Do not create the example songs in an empty folder
  --audio-device <name> Play through the output whose name contains this text
                        (default: the system output)
  --no-audio            Do not open an audio device; run silent
  --list-audio          List audio devices and exit
  --selftest-audio      Play a tone on the left, then the right, and exit
  --verify-log          Check the run log's hash chain and exit
  --version, --help
";

fn default_data() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("PTZProgrammer")
}

fn fail(msg: &str) -> ! {
    eprintln!("ptz-server: {msg}");
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut data = default_data();
    let mut bind = format!("127.0.0.1:{}", ptz_core::api::DEFAULT_PORT);
    let mut access_key = std::env::var("PTZ_ACCESS_KEY").ok();
    let mut seed = true;
    let mut verify_log = false;
    let mut audio_choice = AudioChoice::Default;
    let mut list_audio = false;
    let mut selftest_audio = false;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| fail(&format!("{name} needs a value")));
        match arg.as_str() {
            "--data" => data = PathBuf::from(value("--data")),
            "--bind" => bind = value("--bind"),
            "--access-key" => access_key = Some(value("--access-key")),
            "--no-seed" => seed = false,
            "--verify-log" => verify_log = true,
            "--audio-device" => audio_choice = AudioChoice::Named(value("--audio-device")),
            "--no-audio" => audio_choice = AudioChoice::Off,
            "--list-audio" => list_audio = true,
            "--selftest-audio" => selftest_audio = true,
            "--version" => {
                println!("ptz-server {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return;
            }
            other => fail(&format!("unknown option {other}. Try --help")),
        }
    }

    if verify_log {
        match runlog::verify(&data.join("log")) {
            Ok(n) => println!("Run log intact: {n} records in {}", data.join("log").display()),
            Err(e) => {
                eprintln!("Run log is NOT intact: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if list_audio {
        match audio::device::list() {
            Ok((outputs, inputs)) => {
                println!("Outputs:");
                outputs.iter().for_each(|n| println!("  {n}"));
                println!("Inputs:");
                inputs.iter().for_each(|n| println!("  {n}"));
            }
            Err(e) => {
                eprintln!("No audio devices: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if selftest_audio {
        match audio::selftest(audio_choice) {
            Ok(report) => print!("{report}"),
            Err(e) => {
                eprintln!("Audio self-test did not play: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut cfg = Config::new(&data);
    cfg.seed = seed;
    cfg.audio = audio_choice;
    match http::start(&cfg, &bind, access_key) {
        Ok(running) => {
            // One write, errors ignored: a launcher may read the first line and close the pipe.
            use std::io::Write;
            let banner = format!(
                "ptz-server {} listening on {}\ndata folder: {}\n",
                env!("CARGO_PKG_VERSION"),
                running.url(),
                data.display()
            );
            let mut out = std::io::stdout();
            let _ = out.write_all(banner.as_bytes());
            let _ = out.flush();
            running.wait();
        }
        Err(e) => fail(&e),
    }
}
