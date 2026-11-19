//! Opt-in OSC commands for changing one camera's automation state.

use crate::{engine::Shared, http::lock};
use ptz_core::api::{OscConfig, OscStatus};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;

pub(crate) fn bind(address: &str, access_key: Option<&str>) -> Result<UdpSocket, String> {
    let addr: SocketAddr = address.parse().map_err(|_| format!("{address} is not an OSC address like 127.0.0.1:9000"))?;
    if !addr.ip().is_loopback() && access_key.is_none_or(str::is_empty) {
        return Err(format!("Refusing to listen for OSC on {addr} without --access-key"));
    }
    let socket = UdpSocket::bind(addr).map_err(|e| format!("Could not listen for OSC on {addr}: {e}"))?;
    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(socket)
}

pub(crate) fn initial_status(data: &Path, cli: Option<SocketAddr>, has_access_key: bool) -> OscStatus {
    let mut status = OscStatus { has_access_key, ..Default::default() };
    if let Some(addr) = cli {
        status.config = OscConfig { enabled: true, bind: addr };
        status.from_command_line = true;
        return status;
    }
    let path = data.join("osc.json");
    let saved = std::fs::read(&path).and_then(|b| serde_json::from_slice(&b).map_err(std::io::Error::other));
    match saved {
        Ok(config) => status.config = config,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => status.problem = Some(format!("Could not read {}: {e}", path.display())),
    }
    status
}

/// Own one socket at a time. A settings change cancels receive and drops the old socket.
/// A port in use or a network interface still coming up is retried without restarting the show.
pub(crate) async fn follow(
    mut changes: tokio::sync::watch::Receiver<OscConfig>,
    mut initial: Option<UdpSocket>,
    show: Shared,
    access_key: Option<Arc<String>>,
) {
    let key = access_key.as_deref().map(String::as_str);
    loop {
        let config = *changes.borrow_and_update();
        if config.enabled {
            let socket = config
                .validate(key.is_some())
                .and_then(|()| initial.take().map_or_else(|| bind(&config.bind.to_string(), key), Ok))
                .and_then(|socket| tokio::net::UdpSocket::from_std(socket).map_err(|e| e.to_string()));
            let problem = match socket {
                Ok(socket) => {
                    {
                        let mut show = lock(&show);
                        if show.osc.config != config {
                            continue;
                        }
                        let state = OscStatus { listening: Some(config.bind), problem: None, ..show.osc.clone() };
                        show.set_osc_status(state);
                    }
                    tokio::select! {
                        biased;
                        result = changes.changed() => {
                            if result.is_err() { return; }
                            continue;
                        }
                        error = receive(socket, &show, key, config.bind) => error,
                    }
                }
                Err(error) => error,
            };
            {
                let mut show = lock(&show);
                if show.osc.config != config {
                    continue;
                }
                let state = OscStatus { listening: None, problem: Some(problem), ..show.osc.clone() };
                show.set_osc_status(state);
            }
            tokio::select! {
                result = changes.changed() => if result.is_err() { return; },
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
        } else if changes.changed().await.is_err() {
            return;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Set(bool),
    Toggle(u64),
    Report,
}

#[derive(Debug, PartialEq, Eq)]
struct Command<'a> {
    camera: &'a str,
    operation: Operation,
    sequence: u64,
    key: Option<&'a str>,
}

fn osc_string<'a>(packet: &mut &'a [u8]) -> Option<&'a str> {
    let size = packet.iter().position(|&byte| byte == 0)?;
    let (field, rest) = packet.split_at_checked((size + 1).next_multiple_of(4))?;
    if field[size..].iter().any(|&byte| byte != 0) {
        return None;
    }
    *packet = rest;
    std::str::from_utf8(&field[..size]).ok()
}

fn decode(mut packet: &[u8]) -> Option<Command<'_>> {
    let address = osc_string(&mut packet)?;
    let (camera, action) = address.strip_prefix("/ptz/camera/")?.split_once('/')?;
    if camera.is_empty() || camera.len() > 32 || !camera.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    let tags = osc_string(&mut packet)?.strip_prefix(',')?;
    let (number, mut tags) = tags.split_at_checked(1)?;
    let sequence = match number {
        "i" => i64::from(i32::from_be_bytes(packet.split_off(..4)?.try_into().ok()?)),
        "h" => i64::from_be_bytes(packet.split_off(..8)?.try_into().ok()?),
        _ => return None,
    };
    let operation = match action {
        "arm" => Operation::Set(true),
        "disarm" => Operation::Set(false),
        "report" => Operation::Report,
        "toggle" => {
            tags = tags.strip_prefix('s')?;
            let revision = osc_string(&mut packet)?;
            if revision.len() != 16 {
                return None;
            }
            Operation::Toggle(u64::from_str_radix(revision, 16).ok()?)
        }
        _ => return None,
    };
    if sequence <= 0 {
        return None;
    }
    let key = match tags {
        "s" => Some(osc_string(&mut packet)?),
        "" => None,
        _ => return None,
    };
    packet.is_empty().then_some(Command { camera, operation, sequence: sequence as u64, key })
}

fn osc_field(packet: &mut Vec<u8>, value: &str) {
    packet.extend_from_slice(value.as_bytes());
    packet.resize((packet.len() + 1).next_multiple_of(4), 0);
}

fn report_packet(camera: &str, nonce: u64, (revision, armed, mode): (u64, bool, &str)) -> Vec<u8> {
    let mut packet = Vec::new();
    osc_field(&mut packet, &format!("/ptz/camera/{camera}/state"));
    osc_field(&mut packet, ",s");
    // A printable, line-terminated argument also works with Reactor's UDP Status matcher.
    osc_field(&mut packet, &format!("MC1:{camera}:{nonce}:{revision:016x}:{}:{mode}\n", u8::from(armed)));
    packet
}

async fn receive(socket: tokio::net::UdpSocket, show: &Shared, key: Option<&str>, address: SocketAddr) -> String {
    // Receive the whole datagram. Some platforms report truncation as a socket error.
    let mut buffer = vec![0; 65_535];
    loop {
        let (size, peer) = match socket.recv_from(&mut buffer).await {
            Ok(received) => received,
            Err(e) => return format!("OSC receive on {address} failed: {e}"),
        };
        let Some(command) = decode(&buffer[..size]).filter(|command| command.key == key) else { continue };
        let report = {
            let mut show = lock(show);
            // A settings write takes this same lock and invalidates listening before it returns.
            if show.osc.listening != Some(address) {
                continue;
            }
            show.heard_osc(peer);
            match command.operation {
                Operation::Report => show.camera_osc_report(command.camera).ok(),
                op => {
                    if let Err(e) = show.camera_osc_control(command.camera, op, command.sequence) {
                        eprintln!("ptz-server: OSC command for {} refused ({peer}): {}", command.camera, e.msg);
                    }
                    None
                }
            }
        };
        if let Some(report) = report {
            let packet = report_packet(command.camera, command.sequence, report);
            if let Err(e) = socket.send_to(&packet, peer).await {
                eprintln!("ptz-server: OSC report for {} to {peer} failed: {e}", command.camera);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{AudioChoice, Config};
    use ptz_core::api::{LogRecord, Status};
    use ptz_core::camera::CameraConfig;
    use ptz_core::client::Api;
    use ptz_core::transport::{Action, Output, Run};
    use std::time::{Duration, Instant};

    fn packet(address: &str, sequence: i32, key: Option<&str>) -> Vec<u8> {
        let mut packet = Vec::new();
        osc_field(&mut packet, address);
        osc_field(&mut packet, if key.is_some() { ",is" } else { ",i" });
        packet.extend_from_slice(&sequence.to_be_bytes());
        if let Some(key) = key {
            osc_field(&mut packet, key);
        }
        packet
    }

    #[test]
    fn reads_both_sequence_types_and_access_key() {
        assert_eq!(
            decode(&packet("/ptz/camera/cam1/disarm", 42, Some("show-key"))),
            Some(Command { camera: "cam1", operation: Operation::Set(false), sequence: 42, key: Some("show-key") })
        );
        let mut packet = Vec::new();
        osc_field(&mut packet, "/ptz/camera/cam-12/arm");
        osc_field(&mut packet, ",h");
        packet.extend_from_slice(&i64::MAX.to_be_bytes());
        assert_eq!(
            decode(&packet),
            Some(Command { camera: "cam-12", operation: Operation::Set(true), sequence: i64::MAX as u64, key: None })
        );
    }

    #[test]
    fn rejects_malformed_and_unsequenced_commands() {
        let good = packet("/ptz/camera/cam1/disarm", 1, Some("key"));
        for end in 0..good.len() {
            assert!(decode(&good[..end]).is_none(), "truncated at byte {end}");
        }
        for address in ["/ptz/camera//arm", "/ptz/camera/*/arm", "/ptz/camera/cam1/toggle", "/ptz/camera/cam1/arm/"] {
            assert!(decode(&packet(address, 1, None)).is_none(), "{address}");
        }
        assert!(decode(&packet("/ptz/camera/cam1/arm", 0, None)).is_none());
        assert!(decode(&packet("/ptz/camera/cam1/arm", -1, None)).is_none());
        let mut unsequenced = Vec::new();
        osc_field(&mut unsequenced, "/ptz/camera/cam1/arm");
        osc_field(&mut unsequenced, ",");
        assert!(decode(&unsequenced).is_none());
        let mut extra = good.clone();
        extra.extend_from_slice(&[0; 4]);
        assert!(decode(&extra).is_none());
        let mut bad_padding = good;
        bad_padding[23] = 1;
        assert!(decode(&bad_padding).is_none());
    }

    #[test]
    fn settings_restore_the_listener_and_retry_a_busy_port_without_stopping_the_show() {
        let dir = std::env::temp_dir().join(format!("ptz-osc-settings-{}", std::process::id()));
        let mut cfg = Config::new(&dir);
        cfg.audio = AudioChoice::Off;
        let start = || crate::http::start(&cfg, "127.0.0.1:0", Some("test".into())).unwrap();
        let server = start();
        let api = Api::new(&server.url(), "operator", Some("test"));
        let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
        let config = OscConfig { enabled: true, bind: occupied.local_addr().unwrap() };
        assert!(api.put::<_, OscStatus>("/osc", &config).is_err());
        api.take_lease("operator", false).unwrap();
        let wait = |api: &Api, ready: fn(&OscStatus) -> bool| {
            let until = Instant::now() + Duration::from_secs(6);
            loop {
                let s = api.status().unwrap().osc.unwrap();
                if ready(&s) {
                    return s;
                }
                assert!(Instant::now() < until, "OSC status: {s:?}");
                std::thread::sleep(Duration::from_millis(15));
            }
        };
        let report = || {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            socket.set_read_timeout(Some(Duration::from_millis(350))).unwrap();
            socket.send_to(&packet("/ptz/camera/cam1/report", 71, Some("test")), config.bind).unwrap();
            let mut bytes = [0; 256];
            socket.recv(&mut bytes).ok().is_some_and(|n| bytes[..n].starts_with(b"/ptz/camera/cam1/state\0"))
        };
        api.put::<_, OscStatus>("/osc", &config).unwrap();
        assert!(wait(&api, |s| s.problem.is_some()).listening.is_none());
        drop(occupied);
        assert_eq!(wait(&api, |s| s.listening.is_some()).listening, Some(config.bind));
        assert!(report());
        let last = api.status().unwrap().osc.unwrap().last_packet.unwrap();
        assert!(last.sender.ip().is_loopback() && last.seconds_ago < 2);
        drop(server);

        let server = start();
        let api = Api::new(&server.url(), "operator", Some("test"));
        assert_eq!(wait(&api, |s| s.listening.is_some()).config, config);
        assert!(report());
        api.take_lease("operator", false).unwrap();
        let off = OscConfig { enabled: false, ..config };
        api.put::<_, OscStatus>("/osc", &off).unwrap();
        assert!(!report());
        drop(server);
        let server = start();
        let state = Api::new(&server.url(), "operator", Some("test")).status().unwrap().osc.unwrap();
        assert_eq!(state.config, off);
        assert!(state.listening.is_none());
        assert!(!report());
        drop(server);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn command_line_override_and_missing_key_keep_the_saved_lan_listener_closed() {
        let dir = std::env::temp_dir().join(format!("ptz-osc-override-{}", std::process::id()));
        let mut cfg = Config::new(&dir);
        cfg.audio = AudioChoice::Off;
        let saved = OscConfig { enabled: true, bind: "0.0.0.0:9000".parse().unwrap() };
        crate::storage::write_json(&dir.join("osc.json"), &saved).unwrap();
        let server = crate::http::start_with_osc(&cfg, "127.0.0.1:0", None, Some("127.0.0.1:0")).unwrap();
        let api = Api::new(&server.url(), "test", None);
        api.take_lease("test", false).unwrap();
        assert!(api.status().unwrap().osc.unwrap().from_command_line);
        assert!(api.put::<_, OscStatus>("/osc", &OscConfig::default()).is_err());
        drop(server);
        let server = crate::http::start(&cfg, "127.0.0.1:0", None).unwrap();
        let api = Api::new(&server.url(), "test", None);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let state = api.status().unwrap().osc.unwrap();
            assert_eq!(state.config, saved);
            assert!(state.listening.is_none());
            if let Some(why) = state.problem {
                assert!(why.contains("access key"));
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(server);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_and_toggles_confirm_the_same_revision_once() {
        let folder = std::env::temp_dir().join(format!("ptz-osc-report-{}", std::process::id()));
        let mut cfg = Config::new(&folder);
        cfg.audio = AudioChoice::Off;
        let server = crate::http::start_with_osc(&cfg, "127.0.0.1:0", Some("test".into()), Some("127.0.0.1:0")).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(server.osc_addr.unwrap()).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let report = |nonce| {
            socket.send(&packet("/ptz/camera/cam1/report", nonce, Some("test"))).unwrap();
            let mut buf = [0; 256];
            let size = socket.recv(&mut buf).unwrap();
            let mut fields = &buf[..size];
            assert_eq!(osc_string(&mut fields), Some("/ptz/camera/cam1/state"));
            assert_eq!(osc_string(&mut fields), Some(",s"));
            let args: Vec<_> = osc_string(&mut fields).unwrap().trim_end().split(':').collect();
            assert!(fields.is_empty());
            assert_eq!(&args[..3], ["MC1", "cam1", &nonce.to_string()]);
            assert_eq!(args[3].len(), 16);
            (args[3].to_owned(), args[4] == "1")
        };
        let toggle = |sequence: i32, revision: &str| {
            let mut msg = Vec::new();
            osc_field(&mut msg, "/ptz/camera/cam1/toggle");
            osc_field(&mut msg, ",iss");
            msg.extend_from_slice(&sequence.to_be_bytes());
            osc_field(&mut msg, revision);
            osc_field(&mut msg, "test");
            socket.send(&msg).unwrap();
        };
        let (first, armed) = report(1000);
        assert!(armed);
        toggle(10, &first);
        toggle(10, &first);
        let (second, armed) = report(1001);
        assert!(!armed);
        assert_ne!(first, second);
        socket.send(&packet("/ptz/camera/cam1/arm", 11, Some("test"))).unwrap();
        toggle(12, &first);
        let (third, armed) = report(1002);
        assert!(armed);
        assert_ne!(first, third);
        toggle(9, &third);
        assert_eq!(report(1003), (third.clone(), true));
        toggle(13, &third);
        assert!(!report(1004).1);
        drop(server);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn udp_commands_are_authenticated_ordered_and_independent_during_live_playback() {
        let error = bind("0.0.0.0:0", None).unwrap_err();
        assert!(error.contains("without --access-key"), "{error}");

        let folder = std::env::temp_dir().join(format!("ptz-test-osc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let mut config = Config::new(&folder);
        config.seed = false;
        config.audio = AudioChoice::Off;
        let key = "show-key";
        let server = crate::http::start_with_osc(&config, "127.0.0.1:0", Some(key.into()), Some("127.0.0.1:0")).unwrap();
        let api = Api::new(&server.url(), "test", Some(key));
        let wait_for = |done: fn(&Status) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let status = api.status().unwrap();
                if done(&status) {
                    break status;
                }
                assert!(Instant::now() < deadline, "last status: {status:?}");
                std::thread::sleep(Duration::from_millis(15));
            }
        };
        api.take_lease("Test operator", false).unwrap();
        let mut project = api.project().unwrap();
        project.cameras.push(CameraConfig::new("cam2", "Camera 2"));
        api.put_project(&project).unwrap();
        let song = api.new_song("OSC test", None).unwrap().song.id;
        api.load_song(&song).unwrap();
        api.save(&song).unwrap();
        api.commit(&song).unwrap();
        api.act(Action::Arm, &Default::default()).unwrap();
        wait_for(|s| s.transport.output == Output::Armed);
        let start = api.act(Action::Play, &Default::default()).unwrap();
        assert_eq!(start.transport.run, Run::Playing);

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dest = server.osc_addr.unwrap();
        let send = |address, sequence, key| {
            sender.send_to(&packet(address, sequence, key), dest).unwrap();
        };
        let arm1 = "/ptz/camera/cam1/arm";
        let disarm1 = "/ptz/camera/cam1/disarm";
        let arm2 = "/ptz/camera/cam2/arm";
        let disarm2 = "/ptz/camera/cam2/disarm";
        sender.send_to(&[0; 2048], dest).unwrap();
        send(disarm1, 99, Some("wrong-key"));
        send(disarm1, 100, None);
        send(disarm2, 1, Some(key));
        assert!(wait_for(|s| !s.camera("cam2").unwrap().armed).first().armed);

        send(disarm1, 20, Some(key));
        send(disarm1, 20, Some(key));
        send(arm1, 19, Some(key));
        send(arm1, 20, Some(key));
        send(disarm1, 21, Some(key));
        send(arm2, 2, Some(key));
        let disarmed = wait_for(|s| s.camera("cam2").unwrap().armed);
        assert!(!disarmed.first().armed);
        assert_eq!((disarmed.transport.output, disarmed.transport.run), (Output::Armed, Run::Playing));

        send(arm1, 22, Some(key));
        send(arm1, 22, Some(key));
        send(disarm1, 21, Some(key));
        send(disarm2, 3, Some(key));
        let rearmed = wait_for(|s| !s.camera("cam2").unwrap().armed && s.frame > 0);
        assert!(rearmed.first().armed);
        assert_eq!((rearmed.transport.output, rearmed.transport.run), (Output::Armed, Run::Playing));
        let records: Vec<LogRecord> = api.get("/log?since=0").unwrap();
        let transitions: Vec<_> = records
            .iter()
            .filter(|record| record.kind == "camera_arm" && record.data["camera"] == "cam1")
            .map(|record| record.data["armed"].as_bool().unwrap())
            .collect();
        assert_eq!(transitions, [false, true]);
        drop(server);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
