//! Protocol fakes: a UDP responder that speaks the FR7's VISCA framing over a
//! simulated head (with the digest-protected web interface beside it), and an
//! HTTP responder for the AutoPod+'s commands.
//!
//! The adapters are developed and regression-tested against these. They prove
//! framing, sequencing, retries and the control loop. They do not prove the
//! real units behave the same.

use super::autopod;
use super::cgi;
use super::visca::{self, Command, Packet, Turn};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Request {
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    /// A query value, with `%xx` decoded.
    pub fn query(&self, name: &str) -> Option<String> {
        let query = self.target.split_once('?')?.1;
        query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| percent_decode(v))
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub struct Response {
    pub code: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Framed as the AutoPod+ RX frames its replies, not as a modern web server does.
    pub as_leg_box: bool,
}

impl Response {
    pub fn text(body: impl Into<String>) -> Response {
        Response {
            code: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: body.into().into_bytes(),
            as_leg_box: false,
        }
    }

    pub fn status(code: u16) -> Response {
        Response { code, headers: Vec::new(), body: Vec::new(), as_leg_box: false }
    }

    /// HTTP/1.0, no length, `Connection: close`, `Content-Type` last, body ending CRLF.
    pub fn leg_box(code: u16, body: &str) -> Response {
        Response { code, headers: Vec::new(), body: format!("{body}\r\n").into_bytes(), as_leg_box: true }
    }

    fn head(&self) -> String {
        let reason = match self.code {
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "OK",
        };
        if self.as_leg_box {
            return format!(
                "HTTP/1.0 {} {reason}\r\nServer: AutoPod+/1.7\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n",
                self.code
            );
        }
        let mut head = format!("HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n", self.code, self.body.len());
        for (k, v) in &self.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head + "\r\n"
    }
}

fn answer(stream: TcpStream, handler: &dyn Fn(&Request) -> Option<Response>) -> Option<()> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    // A handler that returns nothing hangs up without a word: a device that has gone away.
    let resp = handler(&Request { target, headers })?;
    let mut stream = stream;
    stream.write_all(resp.head().as_bytes()).ok()?;
    stream.write_all(&resp.body).ok()?;
    stream.flush().ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = reader.read(&mut [0u8; 16]);
    Some(())
}

/// A one-thread HTTP server: enough for a device's web interface.
fn serve(listener: TcpListener, quit: Arc<AtomicBool>, handler: impl Fn(&Request) -> Option<Response> + Send + 'static) {
    listener.set_nonblocking(true).expect("nonblocking listener");
    std::thread::Builder::new()
        .name("fake-http".into())
        .spawn(move || {
            while !quit.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = answer(stream, &handler);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(1)),
                }
            }
        })
        .expect("fake http thread");
}

/// A preset stored in the fake camera.
#[derive(Clone, PartialEq, Debug)]
pub struct FakePreset {
    pub number: u32,
    pub name: String,
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
    pub focus: f64,
}

/// One message the fake camera received.
#[derive(Clone, PartialEq, Debug)]
pub struct Seen {
    pub kind: u16,
    pub payload: Vec<u8>,
}

pub const COUNTS_PER_DEGREE: f64 = 236.0;
const PAN_RANGE: (f64, f64) = (-170.0, 170.0);
const TILT_RANGE: (f64, f64) = (-30.0, 195.0);
/// Zoom runs end to end in three seconds, focus in two.
const ZOOM_PER_S: f64 = visca::ZOOM_TELE as f64 / 3.0;
const FOCUS_PER_S: f64 = 65535.0 / 2.0;

struct Head {
    /// Degrees, left and up positive, whatever sign the counts carry.
    pan: f64,
    tilt: f64,
    pan_speed: f64,
    tilt_speed: f64,
    going_to: Option<(f64, f64, f64, f64)>,
    zoom: f64,
    zoom_target: Option<f64>,
    focus: f64,
    focus_target: Option<f64>,
    focus_manual: bool,
    at: Instant,
    last_seq: Option<u32>,
    silent: bool,
    speed_factor: f64,
    pan_sign: f64,
    tilt_sign: f64,
    first_after_reset: u32,
    drop_next: u32,
    presets: Vec<FakePreset>,
    picture: Vec<u8>,
    seen: Vec<Seen>,
    web_requests: Vec<String>,
}

impl Head {
    fn advance(&mut self, now: Instant) {
        let dt = now.duration_since(self.at).as_secs_f64();
        self.at = now;
        let toward = |at: f64, to: f64, step: f64| at + (to - at).clamp(-step, step);
        if let Some((pan, tilt, pan_speed, tilt_speed)) = self.going_to {
            self.pan = toward(self.pan, pan, pan_speed * self.speed_factor * dt);
            self.tilt = toward(self.tilt, tilt, tilt_speed * self.speed_factor * dt);
            if self.pan == pan && self.tilt == tilt {
                self.going_to = None;
            }
        } else {
            self.pan += self.pan_speed * self.speed_factor * dt;
            self.tilt += self.tilt_speed * self.speed_factor * dt;
        }
        self.pan = self.pan.clamp(PAN_RANGE.0, PAN_RANGE.1);
        self.tilt = self.tilt.clamp(TILT_RANGE.0, TILT_RANGE.1);
        if let Some(z) = self.zoom_target {
            self.zoom = toward(self.zoom, z, ZOOM_PER_S * dt);
        }
        if let Some(f) = self.focus_target {
            self.focus = toward(self.focus, f, FOCUS_PER_S * dt);
        }
    }

    fn counts(&self) -> (i32, i32) {
        (
            (self.pan * self.pan_sign * COUNTS_PER_DEGREE).round() as i32,
            (self.tilt * self.tilt_sign * COUNTS_PER_DEGREE).round() as i32,
        )
    }

    /// Act on one command and return the replies to send, in order.
    fn handle(&mut self, kind: u16, payload: &[u8]) -> Vec<Vec<u8>> {
        let table = visca::speed_table();
        let speed = |step: u8, turn: Turn| match turn {
            Turn::Plus => table[(step.clamp(1, visca::TOP_STEP) - 1) as usize],
            Turn::Minus => -table[(step.clamp(1, visca::TOP_STEP) - 1) as usize],
            Turn::Hold => 0.0,
        };
        let Some(cmd) = visca::parse_command(payload) else {
            return vec![visca::error(0, 0x02)];
        };
        let inquiry = matches!(cmd, Command::PanTiltInquiry | Command::ZoomInquiry | Command::FocusInquiry);
        if inquiry != (kind == visca::TYPE_INQUIRY) {
            return vec![visca::error(0, 0x02)];
        }
        let done = vec![visca::ack(1), visca::completion(1)];
        match cmd {
            Command::PanTiltInquiry => {
                let (pan, tilt) = self.counts();
                vec![visca::pan_tilt_reply(pan, tilt)]
            }
            Command::ZoomInquiry => vec![visca::value_reply(self.zoom.round() as u16)],
            Command::FocusInquiry => vec![visca::value_reply(self.focus.round() as u16)],
            Command::Drive { pan_step, tilt_step, pan, tilt } => {
                self.going_to = None;
                self.pan_speed = speed(pan_step, pan);
                self.tilt_speed = speed(tilt_step, tilt);
                done
            }
            Command::Absolute { pan_step, tilt_step, pan, tilt } => {
                self.pan_speed = 0.0;
                self.tilt_speed = 0.0;
                self.going_to = Some((
                    pan as f64 / COUNTS_PER_DEGREE * self.pan_sign,
                    tilt as f64 / COUNTS_PER_DEGREE * self.tilt_sign,
                    speed(pan_step, Turn::Plus),
                    speed(tilt_step, Turn::Plus),
                ));
                done
            }
            Command::ZoomDirect(z) => {
                self.zoom_target = Some(z as f64);
                done
            }
            Command::ZoomStop => {
                self.zoom_target = None;
                done
            }
            Command::FocusDirect(f) => {
                if !self.focus_manual {
                    // Sony: not executable while focus is automatic.
                    return vec![visca::error(1, 0x41)];
                }
                self.focus_target = Some(f as f64);
                done
            }
            Command::FocusStop => {
                self.focus_target = None;
                done
            }
            Command::FocusManual => {
                self.focus_manual = true;
                done
            }
            Command::Recall(number) => {
                let Some(p) = self.presets.iter().find(|p| p.number == number).cloned() else {
                    return vec![visca::error(1, 0x41)];
                };
                let top = table[table.len() - 1];
                self.pan_speed = 0.0;
                self.tilt_speed = 0.0;
                self.going_to = Some((p.pan, p.tilt, top, top));
                self.zoom_target = Some(p.zoom * visca::ZOOM_TELE as f64);
                self.focus_target = Some(p.focus * 65535.0);
                done
            }
        }
    }
}

/// A stand-in FR7 on loopback: VISCA on a UDP port and the web interface on a TCP port.
pub struct FakeFr7 {
    pub visca: SocketAddr,
    pub http: SocketAddr,
    pub user: String,
    pub password: String,
    head: Arc<Mutex<Head>>,
    quit: Arc<AtomicBool>,
}

/// A picture that starts like a JPEG. The fake has no camera to take one with.
pub const FAKE_PICTURE: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9];

impl FakeFr7 {
    pub fn start() -> FakeFr7 {
        Self::start_on("127.0.0.1:0", "127.0.0.1:0").expect("loopback ports")
    }

    pub fn start_on(visca_bind: &str, http_bind: &str) -> std::io::Result<FakeFr7> {
        let socket = UdpSocket::bind(visca_bind)?;
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        let listener = TcpListener::bind(http_bind)?;
        let (visca_addr, http_addr) = (socket.local_addr()?, listener.local_addr()?);
        let head = Arc::new(Mutex::new(Head {
            pan: 0.0,
            tilt: 0.0,
            pan_speed: 0.0,
            tilt_speed: 0.0,
            going_to: None,
            zoom: 0.0,
            zoom_target: None,
            focus: 32768.0,
            focus_target: None,
            focus_manual: false,
            at: Instant::now(),
            last_seq: None,
            silent: false,
            speed_factor: 1.0,
            pan_sign: 1.0,
            tilt_sign: 1.0,
            first_after_reset: 1,
            drop_next: 0,
            presets: vec![
                FakePreset { number: 1, name: "Wide".into(), pan: 0.0, tilt: 2.0, zoom: 0.05, focus: 0.45 },
                FakePreset { number: 2, name: "Lectern".into(), pan: -24.0, tilt: 1.5, zoom: 0.62, focus: 0.5 },
                FakePreset { number: 3, name: "Two-Shot".into(), pan: 18.0, tilt: 0.5, zoom: 0.38, focus: 0.48 },
            ],
            picture: FAKE_PICTURE.to_vec(),
            seen: Vec::new(),
            web_requests: Vec::new(),
        }));
        let quit = Arc::new(AtomicBool::new(false));
        let (user, password) = ("admin".to_string(), "fr7-password".to_string());

        let (shared, stop) = (head.clone(), quit.clone());
        std::thread::Builder::new()
            .name("fake-fr7".into())
            .spawn(move || {
                let mut buf = [0u8; 128];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
                    let Some(packet) = Packet::parse(&buf[..n]) else { continue };
                    let mut head = shared.lock().unwrap_or_else(|e| e.into_inner());
                    head.advance(Instant::now());
                    if head.silent {
                        continue;
                    }
                    if head.drop_next > 0 {
                        head.drop_next -= 1;
                        continue;
                    }
                    let send = |kind: u16, payload: &[u8]| {
                        let _ = socket.send_to(&Packet::new(kind, packet.seq, payload).to_bytes(), from);
                    };
                    if packet.kind == visca::TYPE_CONTROL {
                        if packet.payload == visca::RESET {
                            head.last_seq = None;
                            send(visca::TYPE_CONTROL_REPLY, &visca::RESET);
                        }
                        continue;
                    }
                    // A repeat of the last message is a resend after a lost reply: answer it again.
                    let repeat = head.last_seq == Some(packet.seq);
                    let expected = head.last_seq.map(|s| s.wrapping_add(1)).unwrap_or(head.first_after_reset);
                    if !repeat && packet.seq != expected {
                        send(visca::TYPE_CONTROL_REPLY, &visca::BAD_SEQUENCE);
                        continue;
                    }
                    head.last_seq = Some(packet.seq);
                    if !repeat {
                        head.seen.push(Seen { kind: packet.kind, payload: packet.payload.clone() });
                    }
                    for reply in head.handle(packet.kind, &packet.payload) {
                        send(visca::TYPE_REPLY, &reply);
                    }
                }
            })
            .expect("fake fr7 thread");

        let (shared, auth_user, auth_password) = (head.clone(), user.clone(), password.clone());
        serve(listener, quit.clone(), move |req| {
            let mut head = shared.lock().unwrap_or_else(|e| e.into_inner());
            if head.silent {
                return None;
            }
            let nonce = "5f2a9c1d7e3b4a60";
            let challenge = cgi::Challenge { realm: "FR7".into(), nonce: nonce.into(), qop: Some("auth".into()), opaque: None };
            let signed_in = req.header("Authorization").and_then(|h| h.strip_prefix("Digest")).is_some_and(|h| {
                let f = cgi::auth_fields(h);
                let get = |name: &str| f.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or("");
                let nc = u32::from_str_radix(get("nc"), 16).unwrap_or(0);
                get("username") == auth_user
                    && get("uri") == req.target
                    && get("nonce") == nonce
                    && get("response")
                        == cgi::digest_response(&auth_user, &auth_password, &challenge, &req.target, nc, get("cnonce"))
            });
            if !signed_in {
                let offer = format!("Digest realm=\"FR7\", nonce=\"{nonce}\", qop=\"auth\", algorithm=MD5");
                return Some(Response {
                    code: 401,
                    headers: vec![("WWW-Authenticate".into(), offer)],
                    body: Vec::new(),
                    as_leg_box: false,
                });
            }
            head.web_requests.push(req.target.clone());
            head.advance(Instant::now());
            match (req.path(), req.query("inq").as_deref()) {
                ("/command/inquiry.cgi", Some("presetposition")) => {
                    let names: Vec<String> = head.presets.iter().map(|p| format!("{},{}", p.number, p.name)).collect();
                    Some(Response::text(format!("PresetNum={}&PresetName={}\r\n", head.presets.len(), names.join(","))))
                }
                ("/command/inquiry.cgi", Some("ptzf")) => {
                    let (pan, tilt) = head.counts();
                    let (zoom, focus) = (head.zoom.round() as u32, head.focus.round() as u32);
                    Some(Response::text(format!(
                        "AbsolutePTZF={:05x},{:05x},{zoom:04x},{focus:04x}\r\n",
                        pan as u32 & 0xF_FFFF,
                        tilt as u32 & 0xF_FFFF
                    )))
                }
                (path, _) => {
                    let number = path
                        .strip_prefix("/preset/presetimg")
                        .and_then(|p| p.strip_suffix(".jpg"))
                        .and_then(|n| n.parse::<u32>().ok());
                    match number.filter(|n| head.presets.iter().any(|p| p.number == *n)) {
                        Some(_) => Some(Response {
                            code: 200,
                            headers: vec![("Content-Type".into(), "image/jpeg".into())],
                            body: head.picture.clone(),
                            as_leg_box: false,
                        }),
                        None => Some(Response::status(404)),
                    }
                }
            }
        });

        Ok(FakeFr7 { visca: visca_addr, http: http_addr, user, password, head, quit })
    }

    fn head(&self) -> std::sync::MutexGuard<'_, Head> {
        let mut head = self.head.lock().unwrap_or_else(|e| e.into_inner());
        head.advance(Instant::now());
        head
    }

    /// Pan and tilt in degrees, zoom and focus from 0 to 1.
    pub fn pose(&self) -> (f64, f64, f64, f64) {
        let h = self.head();
        (h.pan, h.tilt, h.zoom / visca::ZOOM_TELE as f64, h.focus / 65535.0)
    }

    pub fn place(&self, pan: f64, tilt: f64, zoom: f64, focus: f64) {
        let mut h = self.head();
        (h.pan, h.tilt, h.zoom, h.focus) = (pan, tilt, zoom * visca::ZOOM_TELE as f64, focus * 65535.0);
        (h.pan_speed, h.tilt_speed, h.going_to, h.zoom_target, h.focus_target) = (0.0, 0.0, None, None, None);
    }

    pub fn is_moving(&self) -> bool {
        let h = self.head();
        h.pan_speed != 0.0 || h.tilt_speed != 0.0 || h.going_to.is_some()
    }

    /// The picture served for every preset, in place of the stub.
    pub fn set_picture(&self, jpeg: Vec<u8>) {
        self.head().picture = jpeg;
    }

    /// Stop answering, like a pulled cable. The head keeps doing what it was last told.
    pub fn set_silent(&self, silent: bool) {
        self.head().silent = silent;
    }

    /// Move at this share of every commanded speed: a head that cannot keep up.
    pub fn set_speed_factor(&self, factor: f64) {
        self.head().speed_factor = factor;
    }

    /// Count positions the other way round on an axis.
    pub fn set_count_signs(&self, pan: f64, tilt: f64) {
        let mut h = self.head();
        (h.pan_sign, h.tilt_sign) = (pan, tilt);
    }

    /// Expect sequence number zero, not one, on the first message after a reset.
    pub fn expect_zero_after_reset(&self) {
        self.head().first_after_reset = 0;
    }

    /// Lose the next `n` packets without answering.
    pub fn drop_next(&self, n: u32) {
        self.head().drop_next = n;
    }

    pub fn set_presets(&self, presets: Vec<FakePreset>) {
        self.head().presets = presets;
    }

    /// Every message received since the last `clear_seen`, resends left out.
    pub fn seen(&self) -> Vec<Seen> {
        self.head().seen.clone()
    }

    /// The command payloads received, inquiries left out.
    pub fn commands(&self) -> Vec<Vec<u8>> {
        self.seen().into_iter().filter(|s| s.kind == visca::TYPE_COMMAND).map(|s| s.payload).collect()
    }

    pub fn web_requests(&self) -> Vec<String> {
        self.head().web_requests.clone()
    }

    pub fn clear_seen(&self) {
        let mut h = self.head();
        h.seen.clear();
        h.web_requests.clear();
    }
}

impl Drop for FakeFr7 {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// Raw position the fake pedestal reports at its lowest height, and counts per millimetre.
pub const POD_RAW_ZERO: f64 = 4096.0;
pub const POD_RAW_PER_MM: f64 = 60.0;
pub const POD_TOP_MM_S: f64 = 43.0;
pub const POD_TRAVEL_MM: f64 = 670.0;

struct Column {
    mm: f64,
    speed: f64,
    going_to: Option<f64>,
    at: Instant,
    calibration: &'static str,
    silent: bool,
    speed_factor: f64,
    seen: Vec<String>,
    model: String,
    firmware: String,
    power: bool,
    /// Commands starting with this are refused with ER1, as a unit without that feature would.
    refuses: Option<String>,
}

impl Column {
    fn advance(&mut self, now: Instant) {
        let dt = now.duration_since(self.at).as_secs_f64();
        self.at = now;
        match self.going_to {
            Some(to) => {
                let step = POD_TOP_MM_S * self.speed_factor * dt;
                self.mm += (to - self.mm).clamp(-step, step);
                if self.mm == to {
                    self.going_to = None;
                }
            }
            None => self.mm += self.speed * self.speed_factor * dt,
        }
        self.mm = self.mm.clamp(0.0, POD_TRAVEL_MM);
    }

    fn raw(&self) -> u16 {
        (POD_RAW_ZERO + self.mm * POD_RAW_PER_MM).round() as u16
    }
}

/// A stand-in AutoPod+ leg box (RX) on loopback, answering as RX v3.49e does.
pub struct FakeAutoPod {
    pub addr: SocketAddr,
    column: Arc<Mutex<Column>>,
    quit: Arc<AtomicBool>,
}

impl FakeAutoPod {
    pub fn start() -> FakeAutoPod {
        Self::start_on("127.0.0.1:0").expect("loopback port")
    }

    pub fn start_on(bind: &str) -> std::io::Result<FakeAutoPod> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let column = Arc::new(Mutex::new(Column {
            mm: 0.0,
            speed: 0.0,
            going_to: None,
            at: Instant::now(),
            calibration: "rER00",
            silent: false,
            speed_factor: 1.0,
            seen: Vec::new(),
            model: autopod::MODEL.into(),
            firmware: "V3.49e".into(),
            power: true,
            refuses: None,
        }));
        let quit = Arc::new(AtomicBool::new(false));
        let shared = column.clone();
        serve(listener, quit.clone(), move |req| {
            let mut col = shared.lock().unwrap_or_else(|e| e.into_inner());
            col.advance(Instant::now());
            if col.silent {
                return None;
            }
            let position = |col: &Column| format!("aPC8000{:04X}", col.raw());
            match req.path() {
                "/cgi-bin/aw_ptz" | "/cgi-bin/aw_cam" => {}
                "/live/camdata.html" => {
                    let power = if col.power { "p1" } else { "p0" };
                    let lines = [
                        format!("OID:{}", col.model),
                        "TITLE:Stand-in".into(),
                        power.into(),
                        col.calibration.into(),
                        position(&col),
                    ];
                    return Some(Response::leg_box(200, &lines.join("\r\n")));
                }
                // RX v3.49e has no get_state, so it is not a heartbeat.
                _ => return Some(Response::leg_box(404, "Not Found")),
            }
            let cmd = req.query("cmd").unwrap_or_default();
            col.seen.push(cmd.clone());
            let digits = |text: &str, n: usize| text.len() == n && text.bytes().all(|b| b.is_ascii_digit());
            let refused = col.refuses.as_deref().is_some_and(|prefix| cmd.starts_with(prefix));
            let pedal = |col: &mut Column, tt: &str| {
                let tt: f64 = tt.parse().unwrap_or(50.0);
                col.going_to = None;
                // 00 is full down and 99 full up, as the TX sends them.
                col.speed = (tt - 50.0) / if tt < 50.0 { 50.0 } else { 49.0 } * POD_TOP_MM_S;
            };
            let body = if refused {
                format!("ER1:{}", cmd.trim_start_matches('#'))
            } else if cmd == autopod::POSITION_QUERY {
                position(&col)
            } else if cmd == autopod::CALIBRATION_QUERY {
                col.calibration.to_string()
            } else if cmd == autopod::MODEL_QUERY {
                format!("OID:{}", col.model)
            } else if cmd == autopod::FIRMWARE_QUERY {
                col.firmware.clone()
            } else if matches!(cmd.as_str(), "#O" | "#O0" | "#O1") {
                if cmd.len() == 3 {
                    col.power = cmd.ends_with('1');
                    if !col.power {
                        (col.speed, col.going_to) = (0.0, None);
                    }
                }
                format!("p{}", col.power as u8)
            } else if let Some(tt) = cmd.strip_prefix("#T").filter(|t| digits(t, 2)) {
                if col.power {
                    pedal(&mut col, tt);
                }
                format!("tS{tt}")
            } else if let Some(speeds) = cmd.strip_prefix("#PTS").filter(|s| digits(s, 4)) {
                if col.power {
                    pedal(&mut col, &speeds[2..]);
                }
                format!("pTS{speeds}")
            } else if let Some(hex) =
                cmd.strip_prefix("#APC").filter(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                let raw = u16::from_str_radix(&hex[4..], 16).unwrap_or(0);
                if col.power {
                    col.speed = 0.0;
                    col.going_to = Some(((raw as f64 - POD_RAW_ZERO) / POD_RAW_PER_MM).clamp(0.0, POD_TRAVEL_MM));
                }
                format!("aPC{}", hex.to_uppercase())
            } else if cmd == "#D0" || cmd == "#D1" {
                format!("d{}0", &cmd[2..])
            } else if cmd == "#AXI" {
                "axiAAA".to_string()
            } else {
                format!("ER1:{}", cmd.trim_start_matches('#'))
            };
            Some(Response::leg_box(200, &body))
        });
        Ok(FakeAutoPod { addr, column, quit })
    }

    fn column(&self) -> std::sync::MutexGuard<'_, Column> {
        let mut col = self.column.lock().unwrap_or_else(|e| e.into_inner());
        col.advance(Instant::now());
        col
    }

    pub fn height_mm(&self) -> f64 {
        self.column().mm
    }

    pub fn place(&self, mm: f64) {
        let mut col = self.column();
        (col.mm, col.speed, col.going_to) = (mm, 0.0, None);
    }

    pub fn is_moving(&self) -> bool {
        let col = self.column();
        col.speed != 0.0 || col.going_to.is_some()
    }

    /// What `#RER` answers: `rER00` calibrated, `rER53` uncalibrated, `rER42` calibrating, `rER03` motor error.
    pub fn set_calibration(&self, answer: &'static str) {
        self.column().calibration = answer;
    }

    pub fn set_silent(&self, silent: bool) {
        self.column().silent = silent;
    }

    pub fn set_speed_factor(&self, factor: f64) {
        self.column().speed_factor = factor;
    }

    /// On, or in standby, where the column answers but does not move.
    pub fn set_power(&self, on: bool) {
        let mut col = self.column();
        col.power = on;
        if !on {
            (col.speed, col.going_to) = (0.0, None);
        }
    }

    /// What `QID` and `#V?` answer, without the `OID:` prefix.
    pub fn set_identity(&self, model: &str, firmware: &str) {
        let mut col = self.column();
        (col.model, col.firmware) = (model.to_string(), firmware.to_string());
    }

    /// Refuse every command that starts with `prefix` with ER1; `None` accepts them again.
    pub fn refuse(&self, prefix: Option<&str>) {
        self.column().refuses = prefix.map(str::to_string);
    }

    /// Every command received since the last `clear_seen`, in order.
    pub fn seen(&self) -> Vec<String> {
        self.column().seen.clone()
    }

    /// The commands in `seen` that are not questions: the ones that could move or change the unit.
    pub fn orders(&self) -> Vec<String> {
        let questions =
            [autopod::POSITION_QUERY, autopod::CALIBRATION_QUERY, autopod::MODEL_QUERY, autopod::FIRMWARE_QUERY, "#O"];
        self.seen().into_iter().filter(|c| !questions.contains(&c.as_str())).collect()
    }

    pub fn clear_seen(&self) {
        self.column().seen.clear();
    }

    /// The height map an operator would measure on this column.
    pub fn height_map() -> Vec<ptz_core::pose::CalPoint> {
        [0.0, 300.0, 600.0]
            .into_iter()
            .map(|mm| ptz_core::pose::CalPoint { raw: POD_RAW_ZERO + mm * POD_RAW_PER_MM, mm })
            .collect()
    }
}

impl Drop for FakeAutoPod {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}
