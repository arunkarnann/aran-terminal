//! Native notifications (PRD §6.5). The "needs you" notification is the hero nudge.
//!
//! Prototype path: macOS `osascript display notification`, Linux `notify-send`
//! (libnotify) — both work immediately in `tauri dev`, are OS-native, and respect
//! Focus / Do Not Disturb. TODO before the signed beta: migrate to
//! `tauri-plugin-notification` (no shell-escaping, richer actions, one code path).

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

pub fn notify(title: &str, body: &str) {
    // Fire-and-forget so we never block the detection path.
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(body),
            applescript_quote(title),
        );
        let _ = Command::new("osascript").arg("-e").arg(script).spawn();
    }

    // notify-send ships with libnotify and is present on essentially every
    // desktop Linux install (and is a no-op error we swallow if it isn't).
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send").arg(title).arg(body).spawn();
    }

    // Other platforms (e.g. Windows): no-op for now. Silence unused-arg warnings.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, body);
    }
}

#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
