//! macOS file-permission onboarding — port of the Tauri build's `permissions.rs` +
//! `PermissionsSetup.tsx` (issue #3).
//!
//! A terminal that spawns arbitrary child processes can never adopt App Sandbox, so
//! per-file TCC prompts can't be suppressed with entitlements. Instead the app should
//! hold broad file access up front:
//!   * Full Disk Access — one manual switch in System Settings covers every protected
//!     location for the app AND all its PTY children (macOS attributes their file
//!     access to us as the "responsible process").
//!   * Folder priming — reading ~/Desktop, ~/Documents and ~/Downloads once fires the
//!     three folder prompts back-to-back instead of scattered mid-work.
//!
//! Grants only persist across updates when the app has a stable code signature
//! (see conductor/SIGNING.md).

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Vec2};

use crate::theme;

/// Deep-link into System Settings → Privacy & Security → Full Disk Access.
const FDA_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
const PRIMED_FOLDERS: [&str; 3] = ["Desktop", "Documents", "Downloads"];
/// Re-probe FDA this often while the dialog is up, so the ✓ flips as soon as it lands.
const POLL: Duration = Duration::from_secs(2);

/// Only macOS has TCC; elsewhere there is nothing to onboard.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// True when the app currently has Full Disk Access. Standard probe: the TCC database
/// directory is readable only with FDA, and listing it never triggers a prompt.
pub fn has_full_disk_access() -> bool {
    if !SUPPORTED {
        return true;
    }
    let Ok(home) = std::env::var("HOME") else { return false };
    std::fs::read_dir(format!("{home}/Library/Application Support/com.apple.TCC")).is_ok()
}

/// Open System Settings on the Full Disk Access pane (FDA can't be requested
/// programmatically — the user flips the switch).
pub fn open_fda_settings() {
    let _ = std::process::Command::new("open").arg(FDA_SETTINGS_URL).spawn();
}

/// Read each common folder once so macOS asks for all three now. Returns the folders
/// that are readable afterwards (already granted, or granted just now).
fn prime_folders() -> Vec<String> {
    let Ok(home) = std::env::var("HOME") else { return Vec::new() };
    PRIMED_FOLDERS
        .iter()
        .filter(|d| std::fs::read_dir(format!("{home}/{d}")).is_ok())
        .map(|d| d.to_string())
        .collect()
}

/// The .app bundle this process runs from (`…/Aran Terminal.app`), if any.
pub fn app_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Contents/MacOS/aran → three levels up is the bundle.
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension().is_some_and(|e| e == "app")).then(|| app.to_path_buf())
}

/// The onboarding dialog. Shown on first launch without FDA, and from Settings.
pub struct PermissionsDialog {
    pub open: bool,
    fda: bool,
    probed_at: Option<Instant>,
    settings_opened: bool,
    granted: Vec<String>,
    priming: Option<Receiver<Vec<String>>>,
    /// Set when the user asked to relaunch; the app closes its window after this frame.
    pub relaunch: bool,
}

impl PermissionsDialog {
    pub fn new() -> Self {
        PermissionsDialog {
            open: false,
            fda: has_full_disk_access(),
            probed_at: Some(Instant::now()),
            settings_opened: false,
            granted: Vec::new(),
            priming: None,
            relaunch: false,
        }
    }

    pub fn has_fda(&self) -> bool {
        self.fda
    }

    /// Draw the window if open. Returns true when the user dismissed it this frame.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if !self.open {
            return false;
        }
        if self.probed_at.is_none_or(|t| t.elapsed() >= POLL) {
            self.fda = has_full_disk_access();
            self.probed_at = Some(Instant::now());
        }
        ctx.request_repaint_after(POLL);
        if let Some(rx) = &self.priming {
            if let Ok(g) = rx.try_recv() {
                self.granted = g;
                self.priming = None;
            }
        }

        let th = theme::t();
        let mut dismissed = false;
        egui::Window::new("File access")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(
                    "macOS asks permission for every file dragged into the terminal — one dialog at a \
                     time. Grant access once here and those prompts go away for every tool you run \
                     inside Aran Terminal.",
                );
                ui.add_space(10.0);

                ui.label(RichText::new("Option 1 — Full Disk Access (recommended)").strong());
                ui.label(
                    RichText::new(
                        "One switch covers Desktop, Documents, Downloads, Photos, external drives — \
                         everything, for the app and all terminals inside it.",
                    )
                    .color(th.muted)
                    .size(12.0),
                );
                if self.fda {
                    ui.label(RichText::new("✓ Granted.").color(th.running));
                } else {
                    ui.label(RichText::new("Find “Aran Terminal” in the list and turn it on.").size(12.0));
                    ui.horizontal(|ui| {
                        if ui.button("Open System Settings…").clicked() {
                            self.settings_opened = true;
                            open_fda_settings();
                        }
                        if self.settings_opened && ui.button("Relaunch app").clicked() {
                            self.relaunch = true;
                        }
                    });
                    if self.settings_opened {
                        ui.label(
                            RichText::new("macOS applies Full Disk Access after the app restarts — relaunch once you've flipped the switch.")
                                .color(th.muted)
                                .size(11.5),
                        );
                    }
                }

                ui.add_space(10.0);
                ui.label(RichText::new("Option 2 — Just the common folders").strong());
                ui.label(
                    RichText::new("Prefer something narrower? Answer the three folder prompts now instead of being interrupted later.")
                        .color(th.muted)
                        .size(12.0),
                );
                ui.horizontal(|ui| {
                    let busy = self.priming.is_some();
                    let label = if busy { "Requesting…" } else { "Grant folder access" };
                    if ui.add_enabled(!busy && !self.fda, egui::Button::new(label)).clicked() {
                        let (tx, rx) = mpsc::channel();
                        self.priming = Some(rx);
                        let _ = std::thread::Builder::new().name("prime-folders".into()).spawn(move || {
                            let _ = tx.send(prime_folders());
                        });
                    }
                    for f in PRIMED_FOLDERS {
                        let ok = self.fda || self.granted.iter().any(|g| g == f);
                        let mark = if ok { "✓" } else { "·" };
                        ui.label(RichText::new(format!("{mark} {f}")).color(if ok { th.running } else { th.muted }).size(12.0));
                    }
                });

                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let txt = if self.fda { "Done" } else { "Skip for now" };
                    if ui.button(txt).clicked() {
                        self.open = false;
                        dismissed = true;
                    }
                });
            });
        dismissed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_never_panics() {
        // Whatever the grant state, probing must be side-effect free and total.
        let _ = has_full_disk_access();
        let _ = app_bundle();
    }
}
