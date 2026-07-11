//! macOS permission helpers (issue #3).
//!
//! A terminal that spawns arbitrary child processes (Claude Code, node, python…)
//! can never adopt App Sandbox, so per-file TCC prompts can't be suppressed with
//! entitlements. Instead the app itself should hold broad file access up front:
//!
//!   * Full Disk Access — one manual grant in System Settings covers every
//!     protected location for the app AND all its PTY children (macOS attributes
//!     their file access to us as the "responsible process").
//!   * Folder priming — reading ~/Desktop, ~/Documents and ~/Downloads once at
//!     onboarding fires the three folder prompts back-to-back, instead of
//!     scattering them mid-work one file at a time.
//!
//! Grants only persist across updates when the app has a stable code signature
//! (see SIGNING.md) — ad-hoc signatures change every build and reset TCC.

use tauri_plugin_opener::OpenerExt;

/// Deep-link into System Settings → Privacy & Security → Full Disk Access.
const FDA_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// True when the app currently has Full Disk Access.
///
/// Standard probe: the TCC database directory is readable only with FDA.
/// Listing it never triggers a permission prompt — it either works or fails.
#[tauri::command]
pub fn check_full_disk_access() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    std::fs::read_dir(format!("{home}/Library/Application Support/com.apple.TCC")).is_ok()
}

/// Open System Settings on the Full Disk Access pane. FDA cannot be requested
/// programmatically — the user has to flip the switch themselves.
#[tauri::command]
pub fn open_full_disk_access_settings(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(FDA_SETTINGS_URL, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Trigger the Desktop/Documents/Downloads TCC prompts up front by reading each
/// folder once. Returns the folder names that are currently readable (already
/// granted, or granted just now). Denials are the user's choice — not an error.
#[tauri::command]
pub async fn prime_folder_permissions() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Ok(home) = std::env::var("HOME") else {
            return Vec::new();
        };
        ["Desktop", "Documents", "Downloads"]
            .iter()
            .filter(|dir| std::fs::read_dir(format!("{home}/{dir}")).is_ok())
            .map(|dir| dir.to_string())
            .collect()
    })
    .await
    .unwrap_or_default()
}
