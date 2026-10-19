//! Over-the-air updates. Checks the latest GitHub Release, and on macOS downloads the
//! universal `.zip`, swaps the running `.app` bundle in place and relaunches.
//!
//! Uses the system `curl`, `ditto` and `codesign` (always present on macOS), so no
//! HTTP/TLS crates are linked. Every argument is a separate argv entry (no shell).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use serde::Deserialize;

use crate::session::lock;

const REPO: &str = "arunkarnann/aran-terminal";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Re-check this often while the app stays open.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(String),
    /// New bundle is in place at this path: relaunch to finish.
    Installed(PathBuf),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    /// macOS universal .zip, when the release has one.
    pub zip_url: Option<String>,
    pub notes: String,
}

#[derive(Deserialize)]
struct GhRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

#[derive(Clone)]
pub struct Updater {
    status: Arc<Mutex<Status>>,
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: egui::Context) -> Self {
        Updater { status: Arc::new(Mutex::new(Status::Idle)), ctx }
    }

    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    fn set(&self, s: Status) {
        *lock(&self.status) = s;
        self.ctx.request_repaint();
    }

    pub fn dismiss(&self) {
        self.set(Status::Idle);
    }

    /// Check shortly after launch, then every few hours.
    pub fn start_background_checks(&self) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            loop {
                // Don't clobber an offer the user is looking at or an install in flight.
                if matches!(me.status(), Status::Idle | Status::UpToDate | Status::Failed(_)) {
                    me.check_now_blocking(false);
                }
                std::thread::sleep(RECHECK);
            }
        });
    }

    pub fn check_now(&self) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || me.check_now_blocking(true));
    }

    fn check_now_blocking(&self, manual: bool) {
        if manual {
            self.set(Status::Checking);
        }
        match fetch_latest() {
            Ok(r) if is_newer(&r.version, CURRENT) => self.set(Status::Available(r)),
            Ok(_) => self.set(if manual { Status::UpToDate } else { Status::Idle }),
            // Background checks fail quietly (offline, rate limit).
            Err(e) => self.set(if manual { Status::Failed(e) } else { Status::Idle }),
        }
    }

    /// Download + swap the bundle (macOS). Elsewhere, open the release page.
    pub fn install(&self, r: Release) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            me.set(Status::Installing(format!("Downloading {}…", r.version)));
            match install_macos(&r, &me) {
                Ok(path) => me.set(Status::Installed(path)),
                Err(e) => me.set(Status::Failed(e)),
            }
        });
    }
}

/// Start the freshly installed app once this process has quit.
pub fn relaunch(app: &Path) {
    let _ = Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 1; /usr/bin/open -n \"$0\"")
        .arg(app)
        .spawn();
}

pub fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = Command::new(opener).arg(url).spawn();
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-A", "AranTerminal-updater"])
        .arg(&url)
        .output()
        .map_err(|e| format!("curl not available: {e}"))?;
    if !out.status.success() {
        return Err("Couldn't reach GitHub to check for updates.".into());
    }
    parse_release(&out.stdout)
}

fn parse_release(json: &[u8]) -> Result<Release, String> {
    let r: GhRelease = serde_json::from_slice(json).map_err(|e| format!("Unexpected release data: {e}"))?;
    let version = r.tag_name.trim_start_matches('v').to_string();
    if parse_version(&version).is_none() {
        return Err(format!("Unexpected release tag {:?}", r.tag_name));
    }
    let zip_url = r
        .assets
        .iter()
        .find(|a| a.name.ends_with("_universal.zip"))
        .map(|a| a.browser_download_url.clone());
    Ok(Release { version, page_url: r.html_url, zip_url, notes: r.body.unwrap_or_default() })
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// The `.app` this binary runs from (`…/Aran Terminal.app/Contents/MacOS/aran`).
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{what} failed: {}", err.trim().lines().last().unwrap_or("")))
    }
}

fn install_macos(r: &Release, me: &Updater) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        open_url(&r.page_url);
        return Err("Automatic install is macOS-only — the download page is open.".into());
    }
    let zip_url = r.zip_url.as_deref().ok_or("This release has no macOS .zip.")?;
    let Some(bundle) = running_bundle() else {
        open_url(&r.page_url);
        return Err("Not running from an installed .app — the download page is open.".into());
    };
    let parent = bundle.parent().ok_or("Can't locate the app folder.")?.to_path_buf();
    // Stage next to the bundle so the final swap is a same-volume rename.
    let stage = parent.join(".aran-update");
    let _ = std::fs::remove_dir_all(&stage);
    if std::fs::create_dir_all(&stage).is_err() {
        open_url(&r.page_url);
        return Err(format!("No permission to write to {} — the download page is open.", parent.display()));
    }
    let result = (|| {
        let zip = stage.join("update.zip");
        run(
            Command::new("curl").args(["-fsSL", "--max-time", "600", "-A", "AranTerminal-updater", "-o"]).arg(&zip).arg(zip_url),
            "Download",
        )?;
        me.set(Status::Installing("Installing…".into()));
        let unpacked = stage.join("x");
        run(Command::new("ditto").args(["-x", "-k"]).arg(&zip).arg(&unpacked), "Unzip")?;
        let new_app = std::fs::read_dir(&unpacked)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("The download has no .app inside.")?;
        let plist = std::fs::read_to_string(new_app.join("Contents/Info.plist")).map_err(|e| e.to_string())?;
        if !plist_has_version(&plist, &r.version) {
            return Err(format!("The download isn't version {}.", r.version));
        }
        if !new_app.join("Contents/MacOS/aran").is_file() {
            return Err("The download is missing the app binary.".into());
        }
        run(Command::new("codesign").args(["--verify", "--deep"]).arg(&new_app), "Signature check")?;

        // Swap: old → backup, new → place; roll back if the second step fails.
        let backup = stage.join("previous.app");
        std::fs::rename(&bundle, &backup).map_err(|e| format!("Couldn't replace the app: {e}"))?;
        if let Err(e) = std::fs::rename(&new_app, &bundle) {
            let _ = std::fs::rename(&backup, &bundle);
            return Err(format!("Couldn't replace the app: {e}"));
        }
        let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&bundle).output();
        Ok(bundle.clone())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

fn plist_has_version(plist: &str, version: &str) -> bool {
    plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)
        .and_then(|rest| rest.split("<string>").nth(1))
        .and_then(|s| s.split("</string>").next())
        .is_some_and(|v| v.trim() == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("3.0.1", "3.0.0"));
        assert!(is_newer("v3.1.0", "3.0.9"));
        assert!(!is_newer("3.0.0", "3.0.0"));
        assert!(!is_newer("2.9.0", "3.0.0"));
        assert!(!is_newer("garbage", "3.0.0"));
        assert_eq!(parse_version("3.2"), Some((3, 2, 0)));
    }

    #[test]
    fn parses_release() {
        let json = br#"{"tag_name":"v3.0.1","html_url":"https://x/r","body":"notes","assets":[
            {"name":"Aran-Terminal_3.0.1_universal.dmg","browser_download_url":"https://x/d"},
            {"name":"Aran-Terminal_3.0.1_universal.zip","browser_download_url":"https://x/z"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "3.0.1");
        assert_eq!(r.zip_url.as_deref(), Some("https://x/z"));
        assert!(parse_release(br#"{"tag_name":"nightly"}"#).is_err());
    }

    #[test]
    fn reads_plist_version() {
        let p = "<key>CFBundleShortVersionString</key>\n    <string>3.0.1</string>";
        assert!(plist_has_version(p, "3.0.1"));
        assert!(!plist_has_version(p, "3.0.2"));
    }
}
