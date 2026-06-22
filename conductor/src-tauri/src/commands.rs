//! Tauri command handlers (registration is shared; each agent fills its own bodies).
//!
//! Registered once in `lib.rs`. New commands require a contract amendment in
//! `ipc.rs` + `src/ipc/types.ts` first (IMPLEMENTATION-PLAN.md §5).

use tauri::{AppHandle, State};

use crate::db::DbState;
use crate::ipc::{FocusBlock, FocusDay, HistoryEntry, SessionId, SessionMeta, Summary};
use crate::pty::{DetectionState, PtyState};

/// Default daily focus goal when the user hasn't set one: 2 hours.
const DEFAULT_FOCUS_GOAL_MS: i64 = 2 * 60 * 60 * 1000;
const FOCUS_GOAL_KEY: &str = "focus.daily_goal_ms";

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn create_session(
    app: AppHandle,
    state: State<PtyState>,
    detection: State<DetectionState>,
    db: State<DbState>,
    shell: Option<String>,
    cwd: Option<String>,
) -> Result<SessionId, String> {
    crate::pty::create_session(
        app,
        state.0.clone(),
        detection.0.clone(),
        db.0.clone(),
        shell,
        cwd,
    )
}

#[tauri::command]
pub fn write_stdin(state: State<PtyState>, id: SessionId, data: String) -> Result<(), String> {
    state.0.lock().unwrap().write_stdin(&id, &data)
}

#[tauri::command]
pub fn resize_pty(
    state: State<PtyState>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.0.lock().unwrap().resize(&id, cols, rows)
}

#[tauri::command]
pub fn close_session(state: State<PtyState>, id: SessionId) -> Result<(), String> {
    state.0.lock().unwrap().close(&id)
}

#[tauri::command]
pub fn rename_session(state: State<PtyState>, id: SessionId, name: String) -> Result<(), String> {
    state.0.lock().unwrap().rename_session(&id, name)
}

#[tauri::command]
pub fn list_sessions(state: State<PtyState>) -> Result<Vec<SessionMeta>, String> {
    Ok(state.0.lock().unwrap().list_sessions())
}

/// Whole-app resident memory (KB): this process plus the WKWebView helpers.
#[tauri::command]
pub fn get_app_mem() -> Option<i64> {
    crate::pty::app_rss_kb()
}

/// TEMP diagnostic: surface frontend logs in the dev terminal (stdout we can read).
#[tauri::command]
pub fn dev_log(msg: String) {
    eprintln!("[dev_log] {msg}");
}

#[tauri::command]
pub fn set_session_cap(state: State<PtyState>, cap: usize) -> Result<(), String> {
    state.0.lock().unwrap().set_session_cap(cap);
    Ok(())
}

#[tauri::command]
pub fn set_task_label(
    state: State<PtyState>,
    db: State<DbState>,
    id: SessionId,
    label: String,
) -> Result<(), String> {
    state.0.lock().unwrap().set_task_label(&id, label.clone())?;
    crate::db::set_task_label(&db.0.lock().unwrap(), &id, &label);
    Ok(())
}

#[tauri::command]
pub fn get_wait_threshold(detection: State<DetectionState>) -> u64 {
    detection.0.lock().unwrap().t_wait_secs()
}

#[tauri::command]
pub fn set_wait_threshold(
    detection: State<DetectionState>,
    db: State<DbState>,
    secs: u64,
) -> Result<(), String> {
    detection.0.lock().unwrap().set_t_wait_secs(secs);
    crate::db::set_setting(&db.0.lock().unwrap(), "t_wait_secs", &secs.to_string());
    Ok(())
}

#[tauri::command]
pub fn get_command_history(
    db: State<DbState>,
    search: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<HistoryEntry>, String> {
    Ok(crate::db::command_history(
        &db.0.lock().unwrap(),
        search.as_deref().filter(|s| !s.is_empty()),
        limit.unwrap_or(200),
    ))
}

#[tauri::command]
pub fn get_command_suggestions(
    db: State<DbState>,
    prefix: String,
    cwd: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<String>, String> {
    Ok(crate::db::command_suggestions(
        &db.0.lock().unwrap(),
        &prefix,
        cwd.as_deref(),
        limit.unwrap_or(1),
    ))
}

// ---- Focus View (deep-work timers) ----

/// Start a focus (or break) block. The backend stamps `started_at` so the timer is
/// drift-free and survives reloads. Starting a focus block points notifications at the
/// session (muting the others); a break clears that focus.
#[tauri::command]
pub fn start_focus_block(
    state: State<PtyState>,
    db: State<DbState>,
    session_id: Option<SessionId>,
    planned_ms: i64,
    task_label: Option<String>,
    kind: Option<String>,
) -> Result<FocusBlock, String> {
    let kind = kind.unwrap_or_else(|| "focus".to_string());
    let id = uuid::Uuid::new_v4().to_string();
    let started_at = now_ms();
    {
        let conn = db.0.lock().unwrap();
        crate::db::insert_focus_block(
            &conn,
            &id,
            session_id.as_deref(),
            task_label.as_deref(),
            &kind,
            started_at,
            planned_ms.max(0),
        );
    }
    // A focus block mutes other sessions; a break restores normal notifications.
    let focus = if kind == "focus" { session_id.clone() } else { None };
    let drained = state.0.lock().unwrap().set_notification_focus(focus);
    fire_catch_up(&drained);

    crate::db::get_focus_block(&db.0.lock().unwrap(), &id)
        .ok_or_else(|| "focus block vanished".to_string())
}

/// Finish a block normally. If it was the active focus, restore notifications and surface
/// any agents that went WAITING while you were heads-down.
#[tauri::command]
pub fn complete_focus_block(state: State<PtyState>, db: State<DbState>, id: String) {
    crate::db::end_focus_block(&db.0.lock().unwrap(), &id, now_ms(), "completed");
    let drained = state.0.lock().unwrap().set_notification_focus(None);
    fire_catch_up(&drained);
}

/// Stop a block early (abandon). Same notification cleanup as completion.
#[tauri::command]
pub fn abandon_focus_block(state: State<PtyState>, db: State<DbState>, id: String) {
    crate::db::end_focus_block(&db.0.lock().unwrap(), &id, now_ms(), "abandoned");
    let drained = state.0.lock().unwrap().set_notification_focus(None);
    fire_catch_up(&drained);
}

/// Lengthen an active block (the "+5 min" control).
#[tauri::command]
pub fn extend_focus_block(db: State<DbState>, id: String, add_ms: i64) {
    crate::db::extend_focus_block(&db.0.lock().unwrap(), &id, add_ms.max(0));
}

/// The single active block (rehydrates the Focus view on reload).
#[tauri::command]
pub fn get_active_focus_block(db: State<DbState>) -> Option<FocusBlock> {
    crate::db::active_focus_block(&db.0.lock().unwrap())
}

/// Daily focus rollup + streak. `day_start` is the caller's local-midnight epoch ms.
#[tauri::command]
pub fn get_focus_day(db: State<DbState>, day_start: i64) -> FocusDay {
    let conn = db.0.lock().unwrap();
    let goal = focus_goal(&conn);
    crate::db::focus_day(&conn, day_start, goal)
}

#[tauri::command]
pub fn get_focus_goal(db: State<DbState>) -> i64 {
    focus_goal(&db.0.lock().unwrap())
}

#[tauri::command]
pub fn set_focus_goal(db: State<DbState>, ms: i64) {
    crate::db::set_setting(&db.0.lock().unwrap(), FOCUS_GOAL_KEY, &ms.max(0).to_string());
}

/// Mute notifications for every session except `session_id` (None restores normal
/// notifications and fires a catch-up for anything queued while muted).
#[tauri::command]
pub fn set_notification_focus(state: State<PtyState>, session_id: Option<SessionId>) {
    let drained = state.0.lock().unwrap().set_notification_focus(session_id);
    fire_catch_up(&drained);
}

fn focus_goal(conn: &rusqlite::Connection) -> i64 {
    crate::db::get_setting(conn, FOCUS_GOAL_KEY)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_FOCUS_GOAL_MS)
}

/// Surface agents that went WAITING while a focus block muted them.
fn fire_catch_up(labels: &[String]) {
    if labels.is_empty() {
        return;
    }
    let body = if labels.len() == 1 {
        format!("{} was waiting while you focused", labels[0])
    } else {
        format!("{} sessions were waiting while you focused", labels.len())
    };
    crate::notify::notify("Aran Terminal", &body);
}

#[tauri::command]
pub fn get_daily_summary(
    state: State<PtyState>,
    db: State<DbState>,
    since: i64,
    until: i64,
) -> Result<Summary, String> {
    let overrides = state.0.lock().unwrap().override_count() as i64;
    Ok(crate::db::daily_summary(
        &db.0.lock().unwrap(),
        since,
        until,
        overrides,
    ))
}

#[tauri::command]
pub fn reset_stats(db: State<DbState>) {
    crate::db::reset_stats(&db.0.lock().unwrap());
}

// ---- Git inspection (active session's working directory) ----

#[tauri::command]
pub fn git_repos(cwd: String) -> Result<Vec<crate::git::GitRepo>, String> {
    Ok(crate::git::repos(&cwd))
}

#[tauri::command]
pub fn git_status(cwd: String) -> Result<crate::git::GitStatus, String> {
    Ok(crate::git::status(&cwd))
}

#[tauri::command]
pub fn git_log(cwd: String, limit: Option<usize>) -> Result<Vec<crate::git::GitCommit>, String> {
    Ok(crate::git::log(&cwd, limit.unwrap_or(50)))
}

#[tauri::command]
pub fn git_show(cwd: String, hash: String) -> Result<String, String> {
    crate::git::show(&cwd, &hash)
}

#[tauri::command]
pub fn git_diff(cwd: String, path: String, staged: bool) -> Result<String, String> {
    crate::git::diff_file(&cwd, &path, staged)
}

/// Run `<command> --help` and parse its flags for the prompt's flag palette.
#[tauri::command]
pub fn command_help(cwd: String, command: String) -> crate::cmdhelp::CommandHelp {
    crate::cmdhelp::command_help(&cwd, &command)
}

// ---- Git write actions ----

#[tauri::command]
pub fn git_stage(cwd: String, paths: Vec<String>) -> Result<(), String> {
    crate::git::stage(&cwd, &paths)
}

#[tauri::command]
pub fn git_unstage(cwd: String, paths: Vec<String>) -> Result<(), String> {
    crate::git::unstage(&cwd, &paths)
}

#[tauri::command]
pub fn git_commit(cwd: String, message: String, all: bool) -> Result<String, String> {
    crate::git::commit(&cwd, &message, all)
}

#[tauri::command]
pub async fn git_fetch(cwd: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::git::fetch(&cwd))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_pull(cwd: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::git::pull(&cwd))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_push(cwd: String, set_upstream: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::git::push(&cwd, set_upstream))
        .await
        .map_err(|e| e.to_string())?
}
