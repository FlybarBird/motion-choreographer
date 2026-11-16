use ptz_server::engine::{AudioChoice, Config};
use ptz_server::{audio, http, panel_host, runlog};
use std::path::PathBuf;

const HELP: &str = "\
ptz-server: the Motion Choreographer show server

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
  --panel <address>     Run the show from a SKAARHOJ panel at this address as
                        well (Raw Panel mode, port 9923 unless given). The map
                        is panel.json in the data folder, if there is one
  --panel-listen <addr:port>
                        The same, for a panel that dials in: wait for it here
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
    let mut panel: Option<String> = None;
    let mut panel_listen: Option<String> = None;
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
            "--panel" => panel = Some(value("--panel")),
            "--panel-listen" => panel_listen = Some(value("--panel-listen")),
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

    // A map that cannot be read stops the server here, before it owns the show.
    let panel_map = (panel.is_some() || panel_listen.is_some())
        .then(|| ptz_panel::map::PanelMap::load(&data.join("panel.json")).unwrap_or_else(|e| fail(&e)));

    let mut cfg = Config::new(&data);
    cfg.seed = seed;
    cfg.audio = audio_choice;
    match http::start(&cfg, &bind, access_key.clone()) {
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
            // The panel is one more station of this server, reached like any other. It has no
            // part in stopping the server: the rigs are stopped before `wait` returns.
            let reach = panel_host::Reach::new(running.addr, access_key, data.clone());
            let station = panel_map.map(|map| {
                let mut options = ptz_panel::Options::new(&reach.server);
                options.access_key = reach.access_key.clone();
                options.panel = panel.as_deref().map(ptz_panel::with_port);
                options.listen = panel_listen;
                options.map = map;
                ptz_panel::start(options)
            });
            let station = match station.transpose() {
                Ok(station) => station,
                Err(e) => {
                    drop(running);
                    fail(&e)
                }
            };
            if let Some(station) = &station {
                let mut line = match (&panel, station.listening) {
                    (Some(addr), None) => format!("panel: connecting to {}\n", ptz_panel::with_port(addr)),
                    (Some(addr), Some(on)) => {
                        format!("panel: connecting to {}, and waiting for one to dial in on {on}\n", ptz_panel::with_port(addr))
                    }
                    (None, Some(on)) => format!("panel: waiting for one to dial in on {on}\n"),
                    (None, None) => String::new(),
                };
                let file = reach.map_file();
                if file.exists() {
                    line.push_str(&format!("panel map: {}\n", file.display()));
                }
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
                // The app shows where the panel is, and that it is changed here and not in Show settings.
                let address = match (&panel, station.listening) {
                    (Some(addr), _) => ptz_panel::with_port(addr),
                    (None, Some(on)) => format!("a panel that dials in on {on}"),
                    (None, None) => String::new(),
                };
                http::lock(&running.show).set_panel_host(Some(ptz_core::api::PanelHost {
                    address,
                    from_command_line: true,
                    problem: None,
                }));
            }
            // With no panel asked for here, the one in Show settings is run, and follows its settings.
            let follower = station.is_none().then(|| panel_host::follow(running.show.clone(), reach));
            running.wait();
            // The panel is told the controller has stopped, but a controller that is slow
            // to stop does not keep the process alive.
            let (done, stopped) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                if let Some(mut station) = station {
                    station.stop();
                }
                if let Some(mut follower) = follower {
                    follower.stop();
                }
                let _ = done.send(());
            });
            let _ = stopped.recv_timeout(std::time::Duration::from_secs(1));
        }
        Err(e) => fail(&e),
    }
}
