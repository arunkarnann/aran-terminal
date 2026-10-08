//! Tauri command handlers (registration is shared; each agent fills its own bodies).
//!
//! Registered once in `lib.rs`. New commands require a contract amendment in
//! `ipc.rs` + `src/ipc/types.ts` first (IMPLEMENTATION-PLAN.md §5).

use tauri::{AppHandle, State};

use base64::Engine;
use crate::db::DbState;
use crate::ipc::{FocusBlock, FocusDay, HistoryEntry, SessionId, SessionMeta, SessionSnapshot, SessionSnapshotWithScrollback, Summary, Task, TaskEvent, GhAccount, GhItem, GhProjectList, GhRepoInfo, GhSource, GhSourceInput};
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

// ---- Session restore (PRD §8.1) ----

/// Persist a session snapshot (metadata + serialized xterm.js scrollback).
/// Called periodically by the frontend and on app close.
#[tauri::command]
pub fn save_session_snapshot(
    db: State<DbState>,
    snapshot: SessionSnapshot,
    scrollback_base64: Option<String>,
) -> Result<(), String> {
    let scrollback = scrollback_base64
        .and_then(|b64| base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD, b64.as_bytes(),
        ).ok());
    crate::db::save_snapshot(
        &db.0.lock().unwrap(),
        &snapshot,
        scrollback.as_deref(),
        now_ms(),
    );
    Ok(())
}

/// Load all open snapshots (tabs to restore on launch).
#[tauri::command]
pub fn load_session_snapshots(
    db: State<DbState>,
) -> Result<Vec<SessionSnapshotWithScrollback>, String> {
    let rows = crate::db::load_snapshots(&db.0.lock().unwrap());
    Ok(rows
        .into_iter()
        .map(|(snap, sb)| {
            let scrollback_base64 = sb.map(|bytes| {
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            });
            SessionSnapshotWithScrollback {
                snapshot: snap,
                scrollback_base64,
            }
        })
        .collect())
}

/// Load recently closed snapshots (for Cmd+Shift+T reopen).
#[tauri::command]
pub fn load_closed_snapshots(
    db: State<DbState>,
    limit: Option<i64>,
) -> Result<Vec<SessionSnapshotWithScrollback>, String> {
    let rows = crate::db::load_closed_snapshots(&db.0.lock().unwrap(), limit.unwrap_or(10));
    Ok(rows
        .into_iter()
        .map(|(snap, sb)| {
            let scrollback_base64 = sb.map(|bytes| {
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            });
            SessionSnapshotWithScrollback {
                snapshot: snap,
                scrollback_base64,
            }
        })
        .collect())
}

/// Mark a snapshot as closed (tab closed by user).
#[tauri::command]
pub fn close_session_snapshot(db: State<DbState>, session_id: SessionId) -> Result<(), String> {
    crate::db::close_snapshot(&db.0.lock().unwrap(), &session_id, now_ms());
    Ok(())
}

/// Reopen a closed snapshot (Cmd+Shift+T) so it appears in load_snapshots again.
#[tauri::command]
pub fn reopen_session_snapshot(db: State<DbState>, session_id: SessionId) -> Result<(), String> {
    crate::db::reopen_snapshot(&db.0.lock().unwrap(), &session_id);
    Ok(())
}

/// Delete a snapshot entirely.
#[tauri::command]
pub fn delete_session_snapshot(db: State<DbState>, session_id: SessionId) -> Result<(), String> {
    crate::db::delete_snapshot(&db.0.lock().unwrap(), &session_id);
    Ok(())
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

// ---- Git: branches / remotes / stash / discard / merge (issue #6) ----

#[tauri::command]
pub fn git_branches(cwd: String) -> Result<Vec<crate::git::GitBranch>, String> {
    Ok(crate::git::branches(&cwd))
}

#[tauri::command]
pub fn git_checkout(cwd: String, branch: String, create: bool) -> Result<String, String> {
    crate::git::checkout(&cwd, &branch, create)
}

#[tauri::command]
pub fn git_create_branch(cwd: String, name: String, source: Option<String>) -> Result<String, String> {
    crate::git::create_branch(&cwd, &name, source.as_deref())
}

#[tauri::command]
pub fn git_delete_branch(cwd: String, name: String, force: bool) -> Result<String, String> {
    crate::git::delete_branch(&cwd, &name, force)
}

#[tauri::command]
pub fn git_remotes(cwd: String) -> Result<Vec<crate::git::GitRemote>, String> {
    Ok(crate::git::remotes(&cwd))
}

#[tauri::command]
pub fn git_stashes(cwd: String) -> Result<Vec<crate::git::GitStash>, String> {
    Ok(crate::git::stashes(&cwd))
}

#[tauri::command]
pub fn git_stash_push(cwd: String, message: Option<String>, include_untracked: bool) -> Result<String, String> {
    crate::git::stash_push(&cwd, message.as_deref(), include_untracked)
}

#[tauri::command]
pub fn git_stash_pop(cwd: String) -> Result<String, String> {
    crate::git::stash_pop(&cwd)
}

#[tauri::command]
pub fn git_stash_apply(cwd: String, index: usize) -> Result<String, String> {
    crate::git::stash_apply(&cwd, index)
}

#[tauri::command]
pub fn git_stash_drop(cwd: String, index: usize) -> Result<String, String> {
    crate::git::stash_drop(&cwd, index)
}

#[tauri::command]
pub fn git_discard(cwd: String, paths: Vec<String>, untracked: Vec<String>) -> Result<(), String> {
    crate::git::discard(&cwd, &paths, &untracked)
}

#[tauri::command]
pub fn git_discard_all(cwd: String) -> Result<(), String> {
    crate::git::discard_all(&cwd)
}

#[tauri::command]
pub fn git_merge(cwd: String, branch: String) -> Result<crate::git::GitOpResult, String> {
    crate::git::merge(&cwd, &branch)
}

#[tauri::command]
pub fn git_abort_merge(cwd: String) -> Result<String, String> {
    crate::git::abort_merge(&cwd)
}

#[tauri::command]
pub fn git_merge_in_progress(cwd: String) -> Result<bool, String> {
    Ok(crate::git::merge_in_progress(&cwd))
}

#[tauri::command]
pub fn git_cherry_pick(cwd: String, hash: String) -> Result<crate::git::GitOpResult, String> {
    crate::git::cherry_pick(&cwd, &hash)
}

// ---- Tasks sidebar ----

const TASK_STATUSES: &[&str] = &["backlog", "todo", "in_progress", "review", "blocked", "done"];

fn validate_task(t: &Task) -> Result<(), String> {
    if t.title.trim().is_empty() {
        return Err("task title is empty".into());
    }
    if !TASK_STATUSES.contains(&t.status.as_str()) {
        return Err(format!("unknown task status: {}", t.status));
    }
    Ok(())
}

/// All open tasks plus those finished at/after `done_since` (epoch ms, chosen by the
/// frontend so "recently done" follows the local day).
#[tauri::command]
pub fn list_tasks(db: State<DbState>, done_since: i64) -> Result<Vec<Task>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::list_tasks(&conn, done_since).map_err(|e| e.to_string())
}

/// Create a task. `id`, timestamps and lifecycle stamps are assigned by the backend.
#[tauri::command]
pub fn create_task(db: State<DbState>, task: Task) -> Result<Task, String> {
    validate_task(&task)?;
    let task = Task {
        id: uuid::Uuid::new_v4().to_string(),
        title: task.title.trim().to_string(),
        priority: task.priority.clamp(0, 3),
        ..task
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::insert_task(&conn, &task, now_ms()).map_err(|e| e.to_string())
}

/// Whole-row update; the backend logs status changes to the timeline.
#[tauri::command]
pub fn update_task(db: State<DbState>, task: Task) -> Result<Task, String> {
    validate_task(&task)?;
    let task = Task {
        title: task.title.trim().to_string(),
        priority: task.priority.clamp(0, 3),
        ..task
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::update_task(&conn, &task, now_ms())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())
}

#[tauri::command]
pub fn delete_task(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::delete_task(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn task_events(db: State<DbState>, id: String) -> Result<Vec<TaskEvent>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::task_events(&conn, &id).map_err(|e| e.to_string())
}

// ---- GitHub issues sidebar ----
//
// Network work runs on blocking threads (spawn_blocking) and never holds the DB lock.
// Tokens are fetched per call from gh / the Keychain and never returned over IPC.

const GH_ACCOUNT_KEY: &str = "gh.account";
/// JSON array of logins whose pasted PAT lives in the Keychain.
const GH_TOKEN_LOGINS_KEY: &str = "gh.token_logins";

type SharedDb = std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>;

fn gh_token_logins(db: &SharedDb) -> Result<Vec<String>, String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    Ok(crate::db::get_setting(&conn, GH_TOKEN_LOGINS_KEY)
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

fn gh_client_for(db: &SharedDb, account: &str) -> Result<crate::github::Client, String> {
    let token = if gh_token_logins(db)?.iter().any(|l| l == account) {
        crate::github::keychain_get(account)?
    } else {
        crate::github::gh_token(account)?
    };
    Ok(crate::github::Client::new(token))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Accounts from the gh CLI plus pasted-token accounts.
#[tauri::command]
pub async fn gh_accounts(db: State<'_, DbState>) -> Result<Vec<GhAccount>, String> {
    let db = db.0.clone();
    blocking(move || {
        let mut out: Vec<GhAccount> = crate::github::gh_cli_accounts()
            .into_iter()
            .map(|a| GhAccount {
                can_projects: a
                    .scopes
                    .as_deref()
                    .is_none_or(crate::github::scopes_allow_projects),
                login: a.login,
                via: "gh".into(),
                scopes: a.scopes,
            })
            .collect();
        for login in gh_token_logins(&db)? {
            if !out.iter().any(|a| a.login == login) {
                out.push(GhAccount { login, via: "token".into(), scopes: None, can_projects: true });
            }
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub fn gh_get_account(db: State<DbState>) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::db::get_setting(&conn, GH_ACCOUNT_KEY))
}

#[tauri::command]
pub fn gh_set_account(db: State<DbState>, login: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::set_setting(&conn, GH_ACCOUNT_KEY, &login);
    Ok(())
}

/// Validate a pasted personal access token and keep it in the macOS Keychain.
#[tauri::command]
pub async fn gh_add_token(db: State<'_, DbState>, token: String) -> Result<GhAccount, String> {
    let db = db.0.clone();
    blocking(move || {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err("token is empty".into());
        }
        let (login, scopes) = crate::github::Client::new(token.clone()).whoami()?;
        crate::github::keychain_set(&login, &token)?;
        let mut logins = gh_token_logins(&db)?;
        if !logins.contains(&login) {
            logins.push(login.clone());
        }
        let conn = db.lock().map_err(|e| e.to_string())?;
        crate::db::set_setting(&conn, GH_TOKEN_LOGINS_KEY, &serde_json::to_string(&logins).unwrap_or_default());
        Ok(GhAccount {
            can_projects: scopes.as_deref().is_none_or(crate::github::scopes_allow_projects),
            login,
            via: "token".into(),
            scopes,
        })
    })
    .await
}

/// Forget a pasted-token account (Keychain entry + selection + cache).
#[tauri::command]
pub fn gh_remove_token(db: State<DbState>, login: String) -> Result<(), String> {
    crate::github::keychain_delete(&login);
    let shared = db.0.clone();
    let logins: Vec<String> = gh_token_logins(&shared)?.into_iter().filter(|l| *l != login).collect();
    let mut conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::set_setting(&conn, GH_TOKEN_LOGINS_KEY, &serde_json::to_string(&logins).unwrap_or_default());
    crate::db::gh_set_sources(&mut conn, &login, &[], now_ms()).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn gh_list_repos(db: State<'_, DbState>, account: String) -> Result<Vec<GhRepoInfo>, String> {
    let db = db.0.clone();
    blocking(move || gh_client_for(&db, &account)?.list_repos()).await
}

#[tauri::command]
pub async fn gh_list_projects(db: State<'_, DbState>, account: String) -> Result<GhProjectList, String> {
    let db = db.0.clone();
    blocking(move || {
        gh_client_for(&db, &account)?
            .list_projects()
            .map_err(|e| e.replace("<account>", &account))
    })
    .await
}

#[tauri::command]
pub fn gh_sources(db: State<DbState>, account: String) -> Result<Vec<GhSource>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::gh_sources(&conn, &account).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn gh_set_sources(
    db: State<DbState>,
    account: String,
    sources: Vec<GhSourceInput>,
) -> Result<Vec<GhSource>, String> {
    if let Some(bad) = sources.iter().find(|s| s.kind != "repo" && s.kind != "project") {
        return Err(format!("unknown source kind: {}", bad.kind));
    }
    let mut conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::gh_set_sources(&mut conn, &account, &sources, now_ms()).map_err(|e| e.to_string())
}

/// Refresh every selected source of `account`. Per-source failures are recorded on the
/// source (shown in the UI) instead of failing the whole sync.
#[tauri::command]
pub async fn gh_sync(db: State<'_, DbState>, account: String) -> Result<Vec<GhSource>, String> {
    let db = db.0.clone();
    blocking(move || {
        let sources = {
            let conn = db.lock().map_err(|e| e.to_string())?;
            crate::db::gh_sources(&conn, &account).map_err(|e| e.to_string())?
        };
        let client = gh_client_for(&db, &account)?;
        for s in &sources {
            let fetched = if s.kind == "project" {
                client.project_items(&s.id, &s.key)
            } else {
                client.repo_items(&s.id, &s.key).map(|items| (items, Vec::new()))
            };
            let mut conn = db.lock().map_err(|e| e.to_string())?;
            match fetched {
                Ok((items, opts)) => {
                    crate::db::gh_replace_items(&mut conn, &s.id, &items, &opts, now_ms())
                        .map_err(|e| e.to_string())?;
                }
                Err(e) => {
                    let e = e.replace("<account>", &account);
                    crate::db::gh_set_source_error(&conn, &s.id, &e).map_err(|e| e.to_string())?;
                }
            }
        }
        let conn = db.lock().map_err(|e| e.to_string())?;
        crate::db::gh_sources(&conn, &account).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub fn gh_items(db: State<DbState>, account: String) -> Result<Vec<GhItem>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::gh_items(&conn, &account).map_err(|e| e.to_string())
}
