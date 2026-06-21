//! Native notifications (PRD §6.5). The "needs you" notification is the hero nudge.
//!
//! Prototype path: macOS `osascript display notification` — works immediately in `tauri
//! dev`, is macOS-native, and respects Focus / Do Not Disturb. TODO before the signed
//! beta: migrate to `tauri-plugin-notification` (no shell-escaping, richer actions).

use std::process::Command;

pub fn notify(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        applescript_quote(body),
        applescript_quote(title),
    );
    // Fire-and-forget so we never block the detection path.
    let _ = Command::new("osascript").arg("-e").arg(script).spawn();
}

fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
