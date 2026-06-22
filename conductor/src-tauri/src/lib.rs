//! Conductor — terminal focus dashboard. App entry / wiring.
//!
//! Module map (IMPLEMENTATION-PLAN.md §5 ownership):
//!   ipc                — FROZEN contract (shared)
//!   pty                — terminal core (Agent A) + detection wiring
//!   detection          — detection engine (Phase 3, the moat)
//!   shell_integration  — OSC 133 injection
//!   notify             — native notifications
//!   commands           — Tauri handlers (registration shared; bodies per-agent)

pub mod cmdhelp;
pub mod commands;
pub mod db;
pub mod detection;
pub mod git;
pub mod ipc;
pub mod notify;
pub mod pty;
pub mod shell_integration;

use db::DbState;
use pty::{DetectionState, PtyState};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(PtyState::new())
        .manage(DetectionState::new())
        .setup(|app| {
            // Open the local DB (fall back to in-memory if the data dir is unwritable).
            let db = open_db(app);
            app.manage(db);

            let det = app.state::<DetectionState>().0.clone();
            let mgr = app.state::<PtyState>().0.clone();
            let dbc = app.state::<DbState>().0.clone();

            // Reconcile sessions left open from a prior run (quit/crash without cleanup).
            {
                let conn = dbc.lock().unwrap();
                db::reconcile_orphans(&conn);
            }

            // Auto-learn: import the user's existing shell history once, so suggestions
            // work from the first keystroke instead of waiting to build history in-app.
            {
                let conn = dbc.lock().unwrap();
                if !db::is_seeded(&conn) {
                    let hist = read_shell_history(1000);
                    if !hist.is_empty() {
                        db::seed_history(&conn, &hist);
                    }
                }
            }

            // Restore the user's tuned WAITING threshold, if any.
            if let Some(secs) = db::get_setting(&dbc.lock().unwrap(), "t_wait_secs")
                .and_then(|v| v.parse::<u64>().ok())
            {
                det.lock().unwrap().set_t_wait_secs(secs);
            }

            // Start the app-wide detection ticker (drives the prompt-quiet heuristic).
            pty::spawn_ticker(app.handle().clone(), det, mgr.clone(), dbc);
            // Poll per-session memory usage.
            pty::spawn_mem_poller(mgr);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::write_stdin,
            commands::resize_pty,
            commands::close_session,
            commands::rename_session,
            commands::list_sessions,
            commands::get_app_mem,
            commands::dev_log,
            commands::set_session_cap,
            commands::set_task_label,
            commands::get_wait_threshold,
            commands::set_wait_threshold,
            commands::get_command_history,
            commands::get_command_suggestions,
            commands::get_daily_summary,
            commands::reset_stats,
            commands::start_focus_block,
            commands::complete_focus_block,
            commands::abandon_focus_block,
            commands::extend_focus_block,
            commands::get_active_focus_block,
            commands::get_focus_day,
            commands::get_focus_goal,
            commands::set_focus_goal,
            commands::set_notification_focus,
            commands::git_repos,
            commands::git_status,
            commands::git_log,
            commands::git_show,
            commands::git_diff,
            commands::command_help,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Read the most recent `max` command lines from the user's shell history files.
fn read_shell_history(max: usize) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut cmds = Vec::new();
    for rel in [".zsh_history", ".bash_history"] {
        if let Ok(bytes) = std::fs::read(format!("{home}/{rel}")) {
            for line in String::from_utf8_lossy(&bytes).lines() {
                let cmd = parse_hist_line(line);
                if !cmd.is_empty() {
                    cmds.push(cmd);
                }
            }
        }
    }
    if cmds.len() > max {
        cmds = cmds.split_off(cmds.len() - max);
    }
    cmds
}

/// Strip the zsh extended-history prefix (`: <ts>:<dur>;cmd`) if present.
fn parse_hist_line(line: &str) -> String {
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some(idx) = rest.find(';') {
            return rest[idx + 1..].trim().to_string();
        }
    }
    line.trim().to_string()
}

/// Open the persistent DB under the app data dir; fall back to in-memory on any failure
/// so the app never refuses to start over a storage problem.
fn open_db(app: &tauri::App) -> DbState {
    let path = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| {
            let _ = std::fs::create_dir_all(&dir);
            dir.join("conductor.db")
        });
    match path.as_ref().map(|p| DbState::open(p)) {
        Some(Ok(db)) => db,
        other => {
            if let Some(Err(e)) = other {
                eprintln!("conductor: DB open failed ({e}); using in-memory store");
            }
            DbState::memory().expect("in-memory sqlite")
        }
    }
}
