//! Aran Terminal — native build. Pure Rust: egui (OpenGL) for the UI,
//! alacritty_terminal for the screen grid, no webview and no JavaScript.
//!
//! Detection, persistence, IPC types and shell integration are shared with the
//! Tauri build by path, so there is a single copy of that logic.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(dead_code)]
#[path = "../../conductor/src-tauri/src/ipc.rs"]
mod ipc;
#[allow(dead_code)]
#[path = "../../conductor/src-tauri/src/db.rs"]
mod db;
#[allow(dead_code, unused_imports)]
#[path = "../../conductor/src-tauri/src/detection/mod.rs"]
mod detection;
#[path = "../../conductor/src-tauri/src/shell_integration.rs"]
mod shell_integration;
#[path = "../../conductor/src-tauri/src/notify.rs"]
mod notify;

mod app;
mod fonts;
mod tasks;
mod timefmt;
mod session;
mod term_view;
mod theme;

use std::sync::{Arc, Mutex};

use eframe::egui;
use session::{lock, Core, SessionManager};

/// Same bundle identifier as the Tauri app; a separate DB file so both builds can run
/// side by side without closing each other's sessions on startup.
const APP_ID: &str = "studio.gearup.conductor";

fn open_db() -> db::DbState {
    let path = std::env::var("HOME").ok().map(|h| {
        let dir = std::path::PathBuf::from(h).join("Library/Application Support").join(APP_ID);
        let _ = std::fs::create_dir_all(&dir);
        dir.join("native.db")
    });
    match path.as_ref().map(|p| db::DbState::open(p)) {
        Some(Ok(db)) => db,
        other => {
            if let Some(Err(e)) = other {
                eprintln!("aran: DB open failed ({e}); using in-memory store");
            }
            match db::DbState::memory() {
                Ok(db) => db,
                Err(e) => {
                    eprintln!("aran: cannot open any database: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn read_shell_history(max: usize) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut cmds = Vec::new();
    for rel in [".zsh_history", ".bash_history"] {
        if let Ok(bytes) = std::fs::read(format!("{home}/{rel}")) {
            for line in String::from_utf8_lossy(&bytes).lines() {
                // Strip zsh extended-history prefix `: <ts>:<dur>;cmd`.
                let cmd = line
                    .strip_prefix(": ")
                    .and_then(|r| r.find(';').map(|i| &r[i + 1..]))
                    .unwrap_or(line)
                    .trim();
                if !cmd.is_empty() {
                    cmds.push(cmd.to_string());
                }
            }
        }
    }
    if cmds.len() > max {
        cmds = cmds.split_off(cmds.len() - max);
    }
    cmds
}

/// The same "A" icon as the Tauri app (shown in the Dock and app switcher).
fn app_icon() -> Arc<egui::IconData> {
    let png = include_bytes!("../../conductor/src-tauri/icons/icon.png");
    Arc::new(eframe::icon_data::from_png_bytes(png).unwrap_or_default())
}

fn main() -> eframe::Result {
    let db = open_db().0;
    let det = Arc::new(Mutex::new(detection::DetectionEngine::default()));
    {
        let conn = lock(&db);
        db::reconcile_orphans(&conn);
        if !db::is_seeded(&conn) {
            let hist = read_shell_history(1000);
            if !hist.is_empty() {
                db::seed_history(&conn, &hist);
            }
        }
        if let Some(secs) = db::get_setting(&conn, "t_wait_secs").and_then(|v| v.parse::<u64>().ok()) {
            lock(&det).set_t_wait_secs(secs);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aran Terminal")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([480.0, 300.0])
            .with_icon(app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Aran Terminal",
        options,
        Box::new(move |cc| {
            let core = Core {
                mgr: Arc::new(Mutex::new(SessionManager::default())),
                det,
                db,
                ctx: cc.egui_ctx.clone(),
            };
            core.spawn_ticker();
            core.spawn_mem_poller();
            Ok(Box::new(app::AranApp::new(cc, core)))
        }),
    )
}
