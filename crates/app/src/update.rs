//! Updates. The app asks the update service what the newest release is, and when there is a newer one
//! than this, offers it. Installing is one click and the Mac's password: the installer is downloaded,
//! checked, run, and the app and the show server on this computer reopen as the new version.
//!
//! Nothing the service says is taken on trust. A release is described by a small file that was signed
//! on the machine that made it, with a key that never leaves that machine; the description names the
//! installer's size and SHA-256. The app checks the signature with the public half of the key, which
//! is built into it, and the installer against the description, before anything is run as an
//! administrator. Only a version newer than this one is ever installed.
//!
//! A build made with `cargo build` knows no update service and never looks for one: the service's
//! address and key are given to the build by `scripts/release.sh`.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const SERVICE: Option<&str> = option_env!("PTZ_UPDATE_URL");
const KEY: Option<&str> = option_env!("PTZ_UPDATE_KEY");
/// The public half of the key releases are signed with, as hex.
const SIGNED_BY: &str = include_str!("../update-signing.pub");
/// How often the service is asked while the app is open.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The flag the app is reopened with after an update, to start the show server it stopped.
pub const START_SERVER: &str = "--start-server";

/// What the installers are made for, as the update service names it.
pub fn platform() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-x86_64")
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Installer {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// A release, as its signed description gives it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Release {
    pub version: String,
    pub platform: String,
    pub installer: Installer,
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
struct Answer {
    manifest: String,
    signature: String,
}

fn version_parts(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok().filter(|_| !p.is_empty() && p.len() <= 9));
    let found = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(found)
}

/// Is `candidate` a later version than `than`? Anything that is not three numbers is not.
pub fn newer(candidate: &str, than: &str) -> bool {
    matches!((version_parts(candidate), version_parts(than)), (Some(a), Some(b)) if a > b)
}

fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != 2 * N || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

const NOT_SIGNED: &str = "The update is not signed by the key this app trusts, so it was not installed";

/// The release in the service's answer, if it is signed by `signed_by` and is for `platform`.
pub fn read_release(answer: &str, signed_by: &str, platform: &str) -> Result<Release, String> {
    let unreadable = || "The update service's answer could not be read".to_string();
    let answer: Answer = serde_json::from_str(answer).map_err(|_| unreadable())?;
    let key = from_hex::<32>(signed_by).and_then(|k| VerifyingKey::from_bytes(&k).ok());
    let signature = from_hex::<64>(&answer.signature).map(|s| Signature::from_bytes(&s));
    let (Some(key), Some(signature)) = (key, signature) else { return Err(NOT_SIGNED.into()) };
    key.verify_strict(answer.manifest.as_bytes(), &signature).map_err(|_| NOT_SIGNED.to_string())?;

    let release: Release = serde_json::from_str(&answer.manifest).map_err(|_| unreadable())?;
    if release.platform != platform {
        return Err(format!("The update is for {}, and this computer is {platform}", release.platform));
    }
    let name = &release.installer.name;
    let plain = name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) && !name.starts_with('.');
    if version_parts(&release.version).is_none()
        || !plain
        || !name.ends_with(".pkg")
        || from_hex::<32>(&release.installer.sha256).is_none()
    {
        return Err(unreadable());
    }
    Ok(release)
}

/// Is the file at `path` the installer the release describes, byte for byte?
pub fn check_installer(path: &Path, installer: &Installer) -> Result<(), String> {
    let damaged = || "The download is not the installer that was published, so it was not installed. Try again".to_string();
    let mut file = std::fs::File::open(path).map_err(|e| format!("The download could not be read: {e}"))?;
    if file.metadata().map(|m| m.len()).ok() != Some(installer.size) {
        return Err(damaged());
    }
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|e| format!("The download could not be read: {e}"))?;
    let found: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !found.eq_ignore_ascii_case(installer.sha256.trim()) {
        return Err(damaged());
    }
    Ok(())
}

/// Where the updater has got to.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// This build does not update itself, and why.
    Off(&'static str),
    /// Nothing has been asked yet.
    Idle,
    Checking,
    /// The service was asked and this is the newest version.
    Newest,
    /// The service could not be asked, or its answer was no good.
    NotChecked(String),
    Available(Release),
    Downloading(Release, u64),
    /// The installer is running, or waiting for the Mac's password.
    Installing(Release),
    /// The new version is on the disk. It runs once the app is reopened.
    Installed(Release),
    Failed(Release, String),
}

struct Inner {
    step: Step,
    open: bool,
    asked: Option<Instant>,
    /// An install was asked for, and the app is to reopen by itself when it is done.
    reopen: bool,
}

/// The app's one updater. Clones share it.
#[derive(Clone)]
pub struct Updater {
    inner: Arc<Mutex<Inner>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Updater {
    fn with(step: Step, repaint: Arc<dyn Fn() + Send + Sync>) -> Updater {
        Updater { inner: Arc::new(Mutex::new(Inner { step, open: false, asked: None, reopen: false })), repaint }
    }

    /// An updater that never looks for anything: for the tests, and for a station that is not the window.
    pub fn off() -> Updater {
        Updater::with(Step::Off("This copy was not made by a release, so it does not update itself."), Arc::new(|| {}))
    }

    /// The updater of this build.
    pub fn of_this_build(repaint: Arc<dyn Fn() + Send + Sync>) -> Updater {
        let step = match (SERVICE, KEY, platform()) {
            (Some(_), Some(_), Some(_)) => Step::Idle,
            (_, _, None) => Step::Off("Updates are installed by hand on this kind of computer."),
            _ => Step::Off("This copy was not made by a release, so it does not update itself."),
        };
        Updater::with(step, repaint)
    }

    pub fn step(&self) -> Step {
        self.inner.lock().unwrap().step.clone()
    }

    /// The newer version there is to install, while it is on offer.
    pub fn offered(&self) -> Option<String> {
        match self.step() {
            Step::Available(r) | Step::Failed(r, _) => Some(r.version),
            _ => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.inner.lock().unwrap().open
    }

    pub fn open(&self, open: bool) {
        self.inner.lock().unwrap().open = open;
    }

    /// True once, when an install that was asked for has finished: the moment to reopen the app.
    pub fn take_reopen(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.step {
            Step::Installed(_) => std::mem::take(&mut inner.reopen),
            Step::Downloading(..) | Step::Installing(_) => false,
            _ => {
                inner.reopen = false;
                false
            }
        }
    }

    /// Put the updater at a step, as if it had got there.
    #[cfg(test)]
    pub fn set(&self, step: Step) {
        self.inner.lock().unwrap().step = step;
    }

    fn to(&self, step: Step) {
        self.inner.lock().unwrap().step = step;
        (self.repaint)();
    }

    /// Called every frame: asks the service when the app opens, and now and then after that.
    pub fn tick(&self) {
        let due = {
            let inner = self.inner.lock().unwrap();
            matches!(inner.step, Step::Idle | Step::Newest | Step::NotChecked(_))
                && inner.asked.is_none_or(|at| at.elapsed() >= EVERY)
        };
        if due {
            self.check();
        }
    }

    /// Ask the service now.
    pub fn check(&self) {
        let (Some(service), Some(key), Some(platform)) = (SERVICE, KEY, platform()) else { return };
        {
            let mut inner = self.inner.lock().unwrap();
            if !matches!(inner.step, Step::Idle | Step::Newest | Step::NotChecked(_) | Step::Available(_) | Step::Failed(..)) {
                return;
            }
            inner.step = Step::Checking;
            inner.asked = Some(Instant::now());
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let found = ask(&format!("{service}/v1/latest/{platform}"), key).and_then(|a| read_release(&a, SIGNED_BY, platform));
            this.to(match found {
                Ok(release) if newer(&release.version, VERSION) => Step::Available(release),
                Ok(_) => Step::Newest,
                Err(why) => Step::NotChecked(why),
            });
        });
    }

    /// Download the release on offer, check it, and run its installer as an administrator.
    pub fn install(&self) {
        let (Some(service), Some(key)) = (SERVICE, KEY) else { return };
        let release = {
            let mut inner = self.inner.lock().unwrap();
            let (Step::Available(release) | Step::Failed(release, _)) = inner.step.clone() else { return };
            inner.step = Step::Downloading(release.clone(), 0);
            inner.reopen = true;
            release
        };
        let this = self.clone();
        std::thread::spawn(move || {
            let done = (|| {
                let installer = download(service, key, &release, |got| this.to(Step::Downloading(release.clone(), got)))?;
                check_installer(&installer, &release.installer)?;
                this.to(Step::Installing(release.clone()));
                let ran = run_installer(&installer, &release.version);
                let _ = std::fs::remove_file(&installer);
                ran
            })();
            this.to(match done {
                Ok(()) => Step::Installed(release),
                Err(why) => Step::Failed(release, why),
            });
        });
    }
}

const CURL: &str = "/usr/bin/curl";
const NO_ANSWER: &str = "The update service could not be reached. Check this computer's connection to the internet";

/// Run curl with the key given on its standard input, so that it is not in the list of processes.
fn curl(args: &[&str], key: Option<&str>) -> Result<std::process::Child, String> {
    let mut child = Command::new(CURL)
        .args(["--silent", "--show-error", "--config", "-"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{CURL} could not be run: {e}"))?;
    let config = key.map(|k| format!("header = \"x-ptz-key: {k}\"\n")).unwrap_or_default();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    Ok(child)
}

/// What the service says at `url`: the body of a 200, or its reason for anything else.
fn ask(url: &str, key: &str) -> Result<String, String> {
    let out = curl(&["--max-time", "20", "--write-out", "\n%{http_code}", url], Some(key))?
        .wait_with_output()
        .map_err(|_| NO_ANSWER.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", ""));
    match status.trim() {
        "200" => Ok(body.to_string()),
        "" | "000" => Err(NO_ANSWER.into()),
        status => Err(said(body).unwrap_or_else(|| format!("The update service answered {status}"))),
    }
}

/// The reason in one of the service's refusals.
fn said(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Problem {
        error: String,
    }
    serde_json::from_str::<Problem>(body).ok().map(|p| p.error)
}

/// Download the release's installer, telling `progress` how much has arrived. The service answers
/// with an address at the host of the files, which is fetched without the key.
fn download(service: &str, key: &str, release: &Release, progress: impl Fn(u64)) -> Result<PathBuf, String> {
    let dir = dirs::cache_dir().ok_or("This computer has no folder for downloads")?.join("PTZ Programmer").join("updates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} could not be made: {e}", dir.display()))?;
    let to = dir.join(&release.installer.name);

    let url = format!("{service}/v1/download/{}/{}", release.version, release.installer.name);
    let out = curl(&["--max-time", "20", "--write-out", "\n%{http_code} %{redirect_url}", &url], Some(key))?
        .wait_with_output()
        .map_err(|_| NO_ANSWER.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, last) = text.rsplit_once('\n').unwrap_or(("", ""));
    let at = match last.trim().split_once(' ') {
        Some(("302", at)) if at.starts_with("https://") => at.to_string(),
        _ if last.trim().is_empty() || last.starts_with("000") => return Err(NO_ANSWER.into()),
        _ => return Err(said(body).unwrap_or_else(|| "The update service did not say where the installer is".into())),
    };

    let path = to.to_string_lossy().to_string();
    let mut child = curl(&["--location", "--max-time", "900", "--output", &path, "--write-out", "%{http_code}", &at], None)?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()),
            Ok(None) => {
                progress(std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0));
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => break Err(e),
        }
    };
    match status.as_deref() {
        Ok("200") => Ok(to),
        _ => {
            let _ = std::fs::remove_file(&to);
            Err("The installer could not be downloaded. Check this computer's connection to the internet and try again".into())
        }
    }
}

/// Where the installers put the show server.
const SERVER_HOME: &str = "/usr/local/ptz-programmer/bin/ptz-server";

/// Run the installer as an administrator: macOS asks for the password. A station that has no show
/// server is not given one.
fn run_installer(installer: &Path, version: &str) -> Result<(), String> {
    let mut script = String::from("/usr/sbin/installer -pkg \" & quoted form of (item 1 of argv) & \"");
    if !Path::new(SERVER_HOME).is_file() {
        let choices = installer.with_extension("choices.xml");
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\"><array><dict>",
            "<key>attributeSetting</key><integer>0</integer>",
            "<key>choiceAttribute</key><string>selected</string>",
            "<key>choiceIdentifier</key><string>server</string>",
            "</dict></array></plist>\n"
        );
        std::fs::write(&choices, xml).map_err(|e| format!("The installer could not be prepared: {e}"))?;
        script.push_str(" -applyChoiceChangesXML \" & quoted form of (item 3 of argv) & \"");
    }
    script.push_str(" -target /");
    let prompt = format!("{} is installing version {version}.", crate::brand::NAME);
    let out = Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv"])
        .args(["-e", &format!("do shell script \"{script}\" with prompt (item 2 of argv) with administrator privileges")])
        .args(["-e", "end run", "--"])
        .arg(installer)
        .arg(&prompt)
        .arg(installer.with_extension("choices.xml"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("The installer could not be run: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    if said.contains("-128") || said.contains("User canceled") {
        return Err("The password was not given, so nothing was installed".into());
    }
    let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it gave no reason").trim().to_string();
    Err(format!("The installer did not finish: {last}"))
}

/// The app bundle this is running from, when it is one.
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().nth(3).filter(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Can the app reopen itself? A copy run from a build folder cannot.
pub fn can_reopen() -> bool {
    cfg!(target_os = "macos") && bundle().is_some()
}

/// Leave something behind that waits for this app to close and opens it again. With `server` it
/// first stops the show server on this computer, which is still the old version, and the app is
/// told to start the new one. The caller closes the window.
pub fn reopen_after_exit(server: bool) -> Result<(), String> {
    let bundle = bundle().ok_or("This copy is not an installed app, so it cannot reopen itself")?;
    let script = concat!(
        "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done\n",
        "if [ \"$2\" = server ]; then\n",
        "  /usr/bin/pkill -x ptz-server\n",
        "  n=0; while /usr/bin/pgrep -x ptz-server >/dev/null && [ $n -lt 40 ]; do /bin/sleep 0.25; n=$((n+1)); done\n",
        "  exec /usr/bin/open -n \"$3\" --args \"$4\"\n",
        "fi\n",
        "exec /usr/bin/open -n \"$3\"\n",
    );
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", script, "ptz-reopen"])
        .arg(std::process::id().to_string())
        .arg(if server { "server" } else { "app" })
        .arg(bundle)
        .arg(START_SERVER)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so it outlives the app it is waiting for.
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("The app could not arrange to reopen: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn signed(manifest: &str, key: &SigningKey) -> String {
        serde_json::json!({ "manifest": manifest, "signature": hex(&key.sign(manifest.as_bytes()).to_bytes()) }).to_string()
    }

    fn manifest(version: &str, platform: &str, name: &str) -> String {
        serde_json::json!({
            "version": version,
            "platform": platform,
            "installer": { "name": name, "size": 11, "sha256": hex(&Sha256::digest(b"hello world")) },
            "notes": "Scroll bars.",
            "published": "2026-11-15T00:00:00Z",
        })
        .to_string()
    }

    #[test]
    fn only_a_release_signed_by_the_trusted_key_and_made_for_this_computer_is_read() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let trusted = hex(&key.verifying_key().to_bytes());
        let good = manifest("0.1.2", "macos-arm64", "Motion-Choreographer-0.1.2-macos-arm64.pkg");

        let release = read_release(&signed(&good, &key), &trusted, "macos-arm64").unwrap();
        assert_eq!((release.version.as_str(), release.installer.size, release.notes.as_str()), ("0.1.2", 11, "Scroll bars."));
        // The file the release tool writes has a newline after the key.
        assert!(read_release(&signed(&good, &key), &format!("{trusted}\n"), "macos-arm64").is_ok());

        // Signed by somebody else, changed after it was signed, or not signed at all.
        let other = SigningKey::from_bytes(&[8; 32]);
        assert_eq!(read_release(&signed(&good, &other), &trusted, "macos-arm64"), Err(NOT_SIGNED.into()));
        let changed = signed(&good, &key).replace("0.1.2", "0.1.3");
        assert_eq!(read_release(&changed, &trusted, "macos-arm64"), Err(NOT_SIGNED.into()));
        let unsigned = serde_json::json!({ "manifest": good, "signature": "" }).to_string();
        assert_eq!(read_release(&unsigned, &trusted, "macos-arm64"), Err(NOT_SIGNED.into()));
        assert_eq!(read_release(&signed(&good, &key), &"0".repeat(64), "macos-arm64"), Err(NOT_SIGNED.into()));

        // Properly signed, but not something to install here.
        let said = read_release(&signed(&good, &key), &trusted, "macos-x86_64").unwrap_err();
        assert!(said.contains("is for macos-arm64"), "{said}");
        for (version, name) in
            [("0.1", "a.pkg"), ("0.1.2", "../a.pkg"), ("0.1.2", "a b.pkg"), ("0.1.2", "a.sh"), ("0.1.2", ".pkg")]
        {
            let odd = manifest(version, "macos-arm64", name);
            assert!(read_release(&signed(&odd, &key), &trusted, "macos-arm64").is_err(), "{version} {name}");
        }
        assert!(read_release("<html>", &trusted, "macos-arm64").is_err());
    }

    #[test]
    fn the_key_built_into_the_app_is_a_key() {
        let key = from_hex::<32>(SIGNED_BY).expect("crates/app/update-signing.pub holds 64 hex digits");
        assert!(VerifyingKey::from_bytes(&key).is_ok());
    }

    #[test]
    fn only_a_later_version_is_newer() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(newer("0.2.0", "0.1.12"));
        for (candidate, than) in [
            ("0.1.1", "0.1.1"),
            ("0.1.0", "0.1.1"),
            ("0.1", "0.0.1"),
            ("v0.2.0", "0.1.0"),
            ("0.2.0-beta", "0.1.0"),
            ("", "0.1.0"),
        ] {
            assert!(!newer(candidate, than), "{candidate} over {than}");
        }
    }

    #[test]
    fn a_download_is_the_published_installer_or_it_is_not_installed() {
        let dir = std::env::temp_dir().join(format!("ptz-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("installer.pkg");
        std::fs::write(&path, b"hello world").unwrap();
        let installer = Installer { name: "installer.pkg".into(), size: 11, sha256: hex(&Sha256::digest(b"hello world")) };
        assert_eq!(check_installer(&path, &installer), Ok(()));
        assert_eq!(check_installer(&path, &Installer { sha256: installer.sha256.to_uppercase(), ..installer.clone() }), Ok(()));

        // The same length with one byte changed; cut short; and a file that is not there.
        std::fs::write(&path, b"hello_world").unwrap();
        assert!(check_installer(&path, &installer).unwrap_err().contains("not the installer that was published"));
        std::fs::write(&path, b"hello").unwrap();
        assert!(check_installer(&path, &installer).is_err());
        assert!(check_installer(&dir.join("missing.pkg"), &installer).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_was_not_released_never_looks_for_updates() {
        let updater = Updater::off();
        updater.tick();
        updater.check();
        updater.install();
        assert!(matches!(updater.step(), Step::Off(_)));
        assert_eq!(updater.offered(), None);
    }
}
