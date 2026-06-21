//! Terminal core — PTY spawn, read loop, stdin/resize, lifecycle (Agent A territory).
//!
//! Phase 0 ships a working single/multi PTY backend. The session map is already
//! keyed by `SessionId`, so Phase 1 (tabs, cap) is additive — see the markers
//! tagged `PHASE 1` and `PHASE 3` below.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

use rusqlite::Connection;

use crate::detection::{DetectionEngine, Detector, StateEvent};
use crate::ipc::{
    self, events, AttentionState, CapReachedEvent, PtyOutputEvent, SessionClosedEvent, SessionId,
    SessionMeta, StateEventPayload,
};
use crate::{db, notify, shell_integration};

/// Tauri-managed handle to the detection engine (shared with reader threads + ticker).
pub struct DetectionState(pub Arc<Mutex<DetectionEngine>>);

impl DetectionState {
    pub fn new() -> Self {
        DetectionState(Arc::new(Mutex::new(DetectionEngine::default())))
    }
}

impl Default for DetectionState {
    fn default() -> Self {
        Self::new()
    }
}

/// One live terminal session: the PTY master (for resize), its writer (stdin),
/// and the child process handle (for kill on close).
struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    shell: String,
    created_at: i64,
    name: Option<String>,
    // Live attention metadata, updated as detection events arrive.
    state: AttentionState,
    cwd: Option<String>,
    project: Option<String>,
    command_count: i64,
    task_label: Option<String>,
    /// Shell PID — also the process-group id, so RSS sums the whole session tree.
    pid: Option<u32>,
    rss_kb: Option<i64>,
    /// Most recent command run in this session (drives the auto tab name).
    last_command: Option<String>,
}

pub struct SessionManager {
    sessions: HashMap<SessionId, Session>,
    cap: usize,
    override_count: u64,
    /// While a focus block is running, only this session may raise a WAITING notification;
    /// others are muted and their labels queued (Focus View). `None` = notify normally.
    notification_focus: Option<SessionId>,
    /// Labels of sessions that went WAITING while muted, drained as a catch-up on unfocus.
    muted_waits: Vec<String>,
}

impl Default for SessionManager {
    fn default() -> Self {
        SessionManager {
            sessions: HashMap::new(),
            cap: 5,
            override_count: 0,
            notification_focus: None,
            muted_waits: Vec::new(),
        }
    }
}

/// Tauri-managed state. `Arc<Mutex<..>>` so the PTY reader threads can reach in.
pub struct PtyState(pub Arc<Mutex<SessionManager>>);

impl PtyState {
    pub fn new() -> Self {
        PtyState(Arc::new(Mutex::new(SessionManager::default())))
    }
}

impl Default for PtyState {
    fn default() -> Self {
        Self::new()
    }
}

/// A command running at least this long fires a "finished" notification (PRD §6.5).
const LONG_COMMAND_MS: i64 = 10_000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Spawn a shell on a fresh PTY and stream its output to the frontend.
pub fn create_session(
    app: AppHandle,
    mgr: Arc<Mutex<SessionManager>>,
    det: Arc<Mutex<DetectionEngine>>,
    db_conn: Arc<Mutex<Connection>>,
    shell: Option<String>,
    cwd: Option<String>,
) -> Result<SessionId, String> {
    {
        let guard = mgr.lock().unwrap();
        if guard.sessions.len() >= guard.cap {
            let _ = app.emit(
                events::CAP_REACHED,
                CapReachedEvent {
                    limit: guard.cap,
                    current: guard.sessions.len(),
                },
            );
            return Err(format!(
                "cap reached: {} sessions (limit {})",
                guard.sessions.len(),
                guard.cap
            ));
        }
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let shell_path = shell.unwrap_or_else(default_shell);
    let mut cmd = CommandBuilder::new(&shell_path);
    cmd.env("TERM", "xterm-256color");
    // When Aran Terminal is started via `npm run …`, npm leaks its own `npm_config_*`
    // variables (notably `npm_config_prefix`) into our process environment. We inherit
    // the parent env for spawned shells, so those would reach the user's shell and make
    // nvm abort with: "nvm is not compatible with the npm_config_prefix environment
    // variable". Strip the leaked npm config vars so every session starts from a clean env.
    for (key, _) in std::env::vars() {
        if key.to_ascii_lowercase().starts_with("npm_config_") {
            cmd.env_remove(&key);
        }
    }
    // Inject OSC 133 shell-integration so the detector gets prompt/command markers.
    shell_integration::apply(&mut cmd, &shell_path);
    if let Some(dir) = cwd.as_ref() {
        cmd.cwd(dir);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let pid = child.process_id();
    // The slave fd must be dropped in the parent or the reader never sees EOF.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let id = uuid::Uuid::new_v4().to_string();

    // IMPORTANT: persist the session row and register it in the manager BEFORE starting the
    // reader thread. Foreign keys are ON, so any state/command row written before the
    // session row exists would be silently rejected (dropping startup markers + commands).
    db::insert_session(&db_conn.lock().unwrap(), &id, None, &shell_path, now_ms(), 1);
    mgr.lock().unwrap().sessions.insert(
        id.clone(),
        Session {
            master: pair.master,
            writer,
            child,
            shell: shell_path,
            created_at: now_ms(),
            name: None,
            state: AttentionState::Running,
            cwd: None,
            project: None,
            command_count: 0,
            task_label: None,
            pid,
            rss_kb: None,
            last_command: None,
        },
    );

    // Reader thread: PTY bytes -> detection + base64 `pty://output`. On EOF, emit closed.
    let reader_id = id.clone();
    let reader_app = app.clone();
    let reader_det = det.clone();
    let reader_mgr = mgr.clone();
    let reader_db = db_conn.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: shell exited
                Ok(n) => {
                    // Feed the detector first, then paint. State events are rare
                    // (transitions only), so we only lock the manager when there are some.
                    let evs = reader_det
                        .lock()
                        .unwrap()
                        .ingest(&reader_id, &buf[..n], Instant::now());
                    if !evs.is_empty() {
                        process_state_events(&reader_app, &reader_mgr, &reader_db, evs);
                    }
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                    let _ = reader_app.emit(
                        events::PTY_OUTPUT,
                        PtyOutputEvent {
                            id: reader_id.clone(),
                            base64_bytes: b64,
                        },
                    );
                }
                Err(_) => break,
            }
        }
        reader_det.lock().unwrap().remove(&reader_id);
        db::mark_session_closed(&reader_db.lock().unwrap(), &reader_id, now_ms());
        let _ = reader_app.emit(
            events::SESSION_CLOSED,
            SessionClosedEvent {
                id: reader_id.clone(),
                exit_code: None,
            },
        );
    });

    Ok(id)
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Persist detection state to the session + DB, emit `session://state`, nudge on WAITING.
fn process_state_events(
    app: &AppHandle,
    mgr: &Arc<Mutex<SessionManager>>,
    db_conn: &Arc<Mutex<Connection>>,
    events: Vec<StateEvent>,
) {
    for ev in events {
        let at = now_ms();
        let label = mgr.lock().unwrap().apply_state_event(&ev);

        let mut long_finish: Option<i64> = None;
        {
            let conn = db_conn.lock().unwrap();
            db::insert_state_event(&conn, &ev.session, ev.state, at, ev.source);
            if ev.command_started {
                db::insert_command_start(&conn, &ev.session, at);
            }
            if let Some(code) = ev.command_finished {
                // Time the command (for the long-finish nudge) before we close the row.
                if let Some(started) = db::latest_open_command_started_at(&conn, &ev.session) {
                    let dur = at - started;
                    if dur >= LONG_COMMAND_MS {
                        long_finish = Some(dur);
                    }
                }
                db::finish_latest_command(&conn, &ev.session, at, code, ev.command_text.as_deref());
            }
            if let Some(cwd) = &ev.cwd {
                let name = std::path::Path::new(cwd)
                    .file_name()
                    .and_then(|n| n.to_str());
                db::update_project(&conn, &ev.session, cwd, name);
            }
        }
        if let Some(dur) = long_finish {
            notify::notify(
                "Aran Terminal",
                &format!("{label} finished a command ({}s)", dur / 1000),
            );
        }

        let _ = app.emit(
            events::SESSION_STATE,
            StateEventPayload {
                id: ev.session.clone(),
                state: ev.state,
                source: ev.source,
                at,
            },
        );
        if ev.state == AttentionState::Waiting {
            // The engine emits one WAITING transition per blocked episode, so this is
            // already debounced (PRD §6.5). During a focus block, non-focused sessions
            // are muted and queued for a catch-up nudge (Focus View).
            if mgr
                .lock()
                .unwrap()
                .should_notify_waiting(&ev.session, &label)
            {
                notify::notify("Aran Terminal", &format!("{label} is waiting for your input"));
            }
        }
    }
}

/// One `ps` sample (macOS/Unix): per-pid resident memory (KB) and the child list
/// of every pid. We walk the tree by ppid rather than keying on pgid, because an
/// interactive shell uses job control — a foreground job like `opencode` runs in
/// its *own* process group, so summing the shell's pgid would miss it entirely.
struct ProcTable {
    rss: HashMap<u32, i64>,
    children: HashMap<u32, Vec<u32>>,
}

fn sample_proc_table() -> ProcTable {
    let mut rss = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss="])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            if let (Some(pid), Some(ppid), Some(r)) = (it.next(), it.next(), it.next()) {
                if let (Ok(pid), Ok(ppid), Ok(r)) =
                    (pid.parse::<u32>(), ppid.parse::<u32>(), r.parse::<i64>())
                {
                    rss.insert(pid, r);
                    children.entry(ppid).or_default().push(pid);
                }
            }
        }
    }
    ProcTable { rss, children }
}

/// Resident memory (KB) of `root` plus every descendant. `None` if `root` is gone.
/// Iterative DFS with a visited guard against PID-reuse cycles.
fn subtree_rss(root: u32, table: &ProcTable) -> Option<i64> {
    let own = *table.rss.get(&root)?;
    let mut total = own;
    let mut visited = std::collections::HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if let Some(kids) = table.children.get(&pid) {
            for &k in kids {
                if visited.insert(k) {
                    total += table.rss.get(&k).copied().unwrap_or(0);
                    stack.push(k);
                }
            }
        }
    }
    Some(total)
}

/// macOS responsible-pid lookup. The WKWebView's helper processes
/// (`com.apple.WebKit.WebContent` / `.GPU` / `.Networking`) are reparented to
/// launchd, so they never show up as our children — but the OS still records us
/// as their *responsible* process. Resolved at runtime via `dlsym` so a missing
/// private symbol degrades gracefully instead of breaking the link.
#[cfg(target_os = "macos")]
fn responsible_pid(pid: i32) -> Option<i32> {
    use std::os::raw::{c_char, c_int, c_void};
    use std::sync::OnceLock;
    type RespFn = unsafe extern "C" fn(c_int) -> c_int;
    static SYM: OnceLock<Option<RespFn>> = OnceLock::new();
    let f = SYM.get_or_init(|| unsafe {
        extern "C" {
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        }
        // RTLD_DEFAULT on macOS — search every loaded image's exports.
        const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
        let name = b"responsibility_get_pid_responsible_for_pid\0";
        let p = dlsym(RTLD_DEFAULT, name.as_ptr() as *const c_char);
        if p.is_null() {
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, RespFn>(p))
        }
    });
    f.map(|f| unsafe { f(pid as c_int) })
}

/// Resident memory (KB) of the whole app: this process plus the WKWebView helper
/// processes the OS reparents away from us, re-attributed via responsible pid.
/// `None` if the sample fails. macOS only — elsewhere the helpers don't exist.
#[cfg(target_os = "macos")]
pub fn app_rss_kb() -> Option<i64> {
    let me = std::process::id() as i32;
    // In dev, the launcher (cargo/npm) is the responsible process for both us and
    // the helpers; in a packaged build we are. Accept either so it works in both.
    let my_resp = responsible_pid(me).unwrap_or(me);
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,rss=,comm="])
        .output()
        .ok()?;
    let mut total = 0i64;
    let mut found_self = false;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(rss)) = (it.next(), it.next()) else {
            continue;
        };
        let comm = it.collect::<Vec<_>>().join(" ");
        let (Ok(pid), Ok(rss)) = (pid.parse::<i32>(), rss.parse::<i64>()) else {
            continue;
        };
        if pid == me {
            total += rss;
            found_self = true;
        } else if comm.contains("com.apple.WebKit")
            && responsible_pid(pid).is_some_and(|r| r == me || r == my_resp)
        {
            total += rss;
        }
    }
    found_self.then_some(total)
}

#[cfg(not(target_os = "macos"))]
pub fn app_rss_kb() -> Option<i64> {
    None
}

/// Polls per-session memory every 2s. RSS surfaces via `list_sessions` (already polled).
pub fn spawn_mem_poller(mgr: Arc<Mutex<SessionManager>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let table = sample_proc_table();
        if let Ok(mut g) = mgr.lock() {
            for s in g.sessions.values_mut() {
                s.rss_kb = s.pid.and_then(|p| subtree_rss(p, &table));
            }
        }
    });
}

/// Drives the silence heuristic (PRD §6.1 Signal 2): one tick per second, app-wide.
pub fn spawn_ticker(
    app: AppHandle,
    det: Arc<Mutex<DetectionEngine>>,
    mgr: Arc<Mutex<SessionManager>>,
    db_conn: Arc<Mutex<Connection>>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let evs = det.lock().unwrap().tick(Instant::now());
        if !evs.is_empty() {
            process_state_events(&app, &mgr, &db_conn, evs);
        }
    });
}

impl SessionManager {
    /// Update a session's live metadata from a detection event; return a display label.
    fn apply_state_event(&mut self, ev: &StateEvent) -> String {
        if let Some(s) = self.sessions.get_mut(&ev.session) {
            s.state = ev.state;
            if ev.command_started {
                s.command_count += 1;
            }
            if let Some(t) = &ev.command_text {
                s.last_command = Some(t.clone());
            }
            if let Some(cwd) = &ev.cwd {
                s.cwd = Some(cwd.clone());
                s.project = std::path::Path::new(cwd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_string());
            }
            s.name
                .clone()
                .or_else(|| s.project.clone())
                .unwrap_or_else(|| short_id(&ev.session))
        } else {
            short_id(&ev.session)
        }
    }

    pub fn write_stdin(&mut self, id: &str, data: &str) -> Result<(), String> {
        let s = self.sessions.get_mut(id).ok_or("no such session")?;
        s.writer
            .write_all(data.as_bytes())
            .map_err(|e| e.to_string())?;
        s.writer.flush().map_err(|e| e.to_string())
    }

    pub fn resize(&mut self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let s = self.sessions.get(id).ok_or("no such session")?;
        s.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }

    pub fn close(&mut self, id: &str) -> Result<(), String> {
        if let Some(mut s) = self.sessions.remove(id) {
            let _ = s.child.kill();
        }
        Ok(())
    }

    #[allow(dead_code)] // used by the dashboard in Phase 2
    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.keys().cloned().collect()
    }

    pub fn rename_session(&mut self, id: &str, name: String) -> Result<(), String> {
        let s = self.sessions.get_mut(id).ok_or("no such session")?;
        s.name = Some(name);
        Ok(())
    }

    pub fn set_task_label(&mut self, id: &str, label: String) -> Result<(), String> {
        let s = self.sessions.get_mut(id).ok_or("no such session")?;
        s.task_label = Some(label);
        Ok(())
    }

    pub fn set_session_cap(&mut self, new_cap: usize) {
        if new_cap > self.cap {
            self.override_count += 1;
        }
        self.cap = new_cap;
    }

    pub fn list_sessions(&self) -> Vec<SessionMeta> {
        self.sessions
            .iter()
            .map(|(id, s)| SessionMeta {
                id: id.clone(),
                name: s.name.clone(),
                project: s.project.clone(),
                cwd: s.cwd.clone(),
                task_label: s.task_label.clone(),
                shell: s.shell.clone(),
                created_at: s.created_at,
                open_count: 0,
                command_count: s.command_count,
                state: s.state,
                rss_kb: s.rss_kb,
                last_command: s.last_command.clone(),
            })
            .collect()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn override_count(&self) -> u64 {
        self.override_count
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Point notifications at a single session (focus block start), or clear the focus
    /// (block end). Returns the labels of any sessions that went WAITING while muted so
    /// the caller can fire one catch-up notification. Clearing always drains the queue.
    pub fn set_notification_focus(&mut self, id: Option<SessionId>) -> Vec<String> {
        self.notification_focus = id;
        std::mem::take(&mut self.muted_waits)
    }

    /// Whether a WAITING transition for `session` should raise a notification right now.
    /// During a focus block, non-focused sessions are muted and their labels queued.
    pub fn should_notify_waiting(&mut self, session: &SessionId, label: &str) -> bool {
        match &self.notification_focus {
            Some(focus) if focus != session => {
                if !self.muted_waits.iter().any(|l| l == label) {
                    self.muted_waits.push(label.to_string());
                }
                false
            }
            _ => true,
        }
    }
}

// Re-export for command handlers.
pub use ipc::SessionId as Id;

#[cfg(test)]
mod mem_tests {
    use super::{subtree_rss, ProcTable};
    use std::collections::HashMap;

    /// Build a ProcTable from (pid, ppid, rss) triples.
    fn table(rows: &[(u32, u32, i64)]) -> ProcTable {
        let mut rss = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(pid, ppid, r) in rows {
            rss.insert(pid, r);
            children.entry(ppid).or_default().push(pid);
        }
        ProcTable { rss, children }
    }

    #[test]
    fn sums_shell_plus_foreground_job_in_its_own_group() {
        // The real-world bug: shell (cheap) with a heavy foreground job (opencode)
        // that lives in its own process group. Tree walk must still capture it.
        let t = table(&[
            (100, 1, 38_000),  // conductor
            (200, 100, 500),   // shell — child of conductor
            (300, 200, 81_520), // opencode — child of shell
            (400, 300, 5_000), // a node subprocess opencode spawned
        ]);
        // Session pid is the shell (200): shell + opencode + its child.
        assert_eq!(subtree_rss(200, &t), Some(500 + 81_520 + 5_000));
        // Walking from the shell must NOT pull in the conductor parent.
        assert_eq!(subtree_rss(200, &t).unwrap() < 100_000, true);
    }

    #[test]
    fn missing_pid_is_none() {
        let t = table(&[(1, 0, 10)]);
        assert_eq!(subtree_rss(999, &t), None);
    }

    #[test]
    fn cycle_does_not_loop_forever() {
        // Defensive: PID-reuse could in theory create a parent/child cycle.
        let t = table(&[(10, 11, 100), (11, 10, 200)]);
        assert_eq!(subtree_rss(10, &t), Some(300));
    }
}
