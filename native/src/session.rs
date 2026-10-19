//! Terminal sessions — ported from `conductor/src-tauri/src/pty.rs` without Tauri.
//!
//! Each session owns a PTY, a reader thread and an `alacritty_terminal::Term` (the
//! screen grid). The reader feeds every chunk to the detection engine first, then to
//! the terminal grid, then asks egui to repaint. Where the Tauri build emitted IPC
//! events, this build updates shared state and the UI reads it each frame.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::Processor;
use eframe::egui;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use rusqlite::Connection;

use crate::detection::{DetectionEngine, Detector, StateEvent};
use crate::ipc::{AttentionState, SessionId};
use crate::{db, notify, shell_integration};

/// Lock a mutex, recovering the data if another thread panicked while holding it.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Project (group) name for a folder: its last path segment.
pub fn project_name(path: &str) -> Option<String> {
    match std::path::Path::new(path).file_name() {
        Some(n) => Some(n.to_string_lossy().into_owned()),
        None if !path.is_empty() => Some(path.to_string()),
        None => None,
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// A command running at least this long fires a "finished" notification.
const LONG_COMMAND_MS: i64 = 10_000;
const SCROLLBACK_LINES: usize = 10_000;

/// Grid size handed to alacritty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Bytes for the PTY go through a channel to a per-session writer thread, so a big
/// paste into a busy app can never block the UI thread (or deadlock with the reader).
type SharedWriter = std::sync::mpsc::Sender<Vec<u8>>;

fn spawn_writer(mut w: Box<dyn Write + Send>, id: &str) -> std::io::Result<SharedWriter> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new().name(format!("pty-w-{}", &id[..8])).spawn(move || {
        for chunk in rx {
            if w.write_all(&chunk).and_then(|_| w.flush()).is_err() {
                break;
            }
        }
    })?;
    Ok(tx)
}

/// Receives terminal-side events (replies to queries, title, bell).
#[derive(Clone)]
pub struct Listener {
    writer: SharedWriter,
    title: Arc<Mutex<Option<String>>>,
    ctx: egui::Context,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            // Replies to device-status / cursor-position queries must reach the app.
            TermEvent::PtyWrite(s) => {
                let _ = self.writer.send(s.into_bytes());
            }
            TermEvent::Title(t) => *lock(&self.title) = Some(t),
            TermEvent::ResetTitle => *lock(&self.title) = None,
            TermEvent::ClipboardStore(_, text) => self.ctx.copy_text(text),
            TermEvent::Wakeup | TermEvent::Bell => self.ctx.request_repaint(),
            _ => {}
        }
    }
}

pub type SharedTerm = Arc<FairMutex<Term<Listener>>>;

/// One live terminal session.
pub struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: SharedWriter,
    child: Box<dyn Child + Send + Sync>,
    pub term: SharedTerm,
    pub exited: Arc<AtomicBool>,
    pub title: Arc<Mutex<Option<String>>>,
    pub size: GridSize,
    pub created_at: i64,
    pub name: Option<String>,
    pub state: AttentionState,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub command_count: i64,
    pub pid: Option<u32>,
    pub rss_kb: Option<i64>,
    pub last_command: Option<String>,
}

impl Session {
    /// Tab label, same rule as the Tauri build: explicit name, else
    /// "<command> › <folder>", else whichever exists, else the terminal title.
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        let cmd = self.last_command.as_deref().map(str::trim).filter(|c| !c.is_empty());
        match (cmd, self.project.as_deref()) {
            (Some(c), Some(f)) => format!("{c} › {f}"),
            (Some(c), None) => c.to_string(),
            (None, Some(f)) => f.to_string(),
            (None, None) => lock(&self.title)
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or("shell")
                .to_string(),
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.writer.send(bytes.to_vec());
    }

    /// Resize the grid and the PTY together. No-op if the size didn't change.
    pub fn resize(&mut self, size: GridSize) {
        if size == self.size || size.cols < 2 || size.rows < 1 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.master.resize(PtySize {
            rows: size.rows as u16,
            cols: size.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

pub struct SessionManager {
    /// Insertion order = tab order.
    pub order: Vec<SessionId>,
    pub sessions: HashMap<SessionId, Session>,
    pub cap: usize,
    pub override_count: u64,
    notification_focus: Option<SessionId>,
    muted_waits: Vec<String>,
}

impl Default for SessionManager {
    fn default() -> Self {
        SessionManager {
            order: Vec::new(),
            sessions: HashMap::new(),
            cap: 5,
            override_count: 0,
            notification_focus: None,
            muted_waits: Vec::new(),
        }
    }
}

/// Everything the reader threads and the ticker share.
#[derive(Clone)]
pub struct Core {
    pub mgr: Arc<Mutex<SessionManager>>,
    pub det: Arc<Mutex<DetectionEngine>>,
    pub db: Arc<Mutex<Connection>>,
    pub ctx: egui::Context,
}

#[derive(Debug)]
pub enum CreateError {
    CapReached { limit: usize },
    Spawn(String),
}

impl Core {
    /// Spawn a shell on a fresh PTY. Mirrors the Tauri `pty::create_session`.
    pub fn create_session(&self, cwd: Option<String>, size: GridSize) -> Result<SessionId, CreateError> {
        {
            let g = lock(&self.mgr);
            if g.sessions.len() >= g.cap {
                return Err(CreateError::CapReached { limit: g.cap });
            }
        }
        let spawn = |e: String| CreateError::Spawn(e);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.rows as u16,
                cols: size.cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| spawn(e.to_string()))?;

        let shell_path = default_shell();
        let mut cmd = CommandBuilder::new(&shell_path);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "AranTerminal");
        // Leaked npm config vars break nvm in child shells.
        for (key, _) in std::env::vars() {
            if key.to_ascii_lowercase().starts_with("npm_config_") {
                cmd.env_remove(&key);
            }
        }
        // Don't leak a parent Claude Code session's markers into child shells.
        for key in ["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT"] {
            cmd.env_remove(key);
        }
        // A .app bundle launched from Finder has no UTF-8 locale; without it zsh mangles
        // multibyte input.
        if std::env::var("LANG").is_err() {
            cmd.env("LANG", "en_US.UTF-8");
        }
        shell_integration::apply(&mut cmd, &shell_path);
        let cwd: Option<String> = cwd.or_else(|| std::env::var("HOME").ok());
        if let Some(dir) = cwd.as_ref() {
            cmd.cwd(dir);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| spawn(e.to_string()))?;
        let pid = child.process_id();
        // The slave fd must be dropped in the parent or the reader never sees EOF.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| spawn(e.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        let raw_writer = pair.master.take_writer().map_err(|e| spawn(e.to_string()))?;
        let writer = spawn_writer(raw_writer, &id).map_err(|e| spawn(e.to_string()))?;

        let title = Arc::new(Mutex::new(None));
        let listener = Listener {
            writer: writer.clone(),
            title: title.clone(),
            ctx: self.ctx.clone(),
        };
        let config = TermConfig {
            scrolling_history: SCROLLBACK_LINES,
            ..TermConfig::default()
        };
        let term: SharedTerm = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        let exited = Arc::new(AtomicBool::new(false));

        // Persist + register BEFORE the reader starts: foreign keys reject state rows
        // written before the session row exists.
        db::insert_session(&lock(&self.db), &id, None, &shell_path, now_ms(), 1);
        {
            let mut g = lock(&self.mgr);
            g.order.push(id.clone());
            g.sessions.insert(
                id.clone(),
                Session {
                    master: pair.master,
                    writer,
                    child,
                    term: term.clone(),
                    exited: exited.clone(),
                    title,
                    size,
                    created_at: now_ms(),
                    name: None,
                    state: AttentionState::Running,
                    project: cwd.as_deref().and_then(project_name),
                    cwd,
                    command_count: 0,
                    pid,
                    rss_kb: None,
                    last_command: None,
                },
            );
        }

        let core = self.clone();
        let rid = id.clone();
        std::thread::Builder::new()
            .name(format!("pty-{}", &id[..8]))
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                let mut parser: Processor = Processor::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let evs = lock(&core.det).ingest(&rid, &buf[..n], Instant::now());
                            if !evs.is_empty() {
                                core.process_state_events(evs);
                            }
                            parser.advance(&mut *term.lock(), &buf[..n]);
                            core.ctx.request_repaint();
                        }
                    }
                }
                lock(&core.det).remove(&rid);
                db::mark_session_closed(&lock(&core.db), &rid, now_ms());
                exited.store(true, Ordering::Release);
                core.ctx.request_repaint();
            })
            .map_err(|e| spawn(e.to_string()))?;

        Ok(id)
    }

    /// Persist detection state, update the live session, nudge on WAITING.
    pub fn process_state_events(&self, events: Vec<StateEvent>) {
        for ev in events {
            let at = now_ms();
            let label = lock(&self.mgr).apply_state_event(&ev);

            let mut long_finish: Option<i64> = None;
            {
                let conn = lock(&self.db);
                db::insert_state_event(&conn, &ev.session, ev.state, at, ev.source);
                if ev.command_started {
                    db::insert_command_start(&conn, &ev.session, at);
                }
                if let Some(code) = ev.command_finished {
                    if let Some(started) = db::latest_open_command_started_at(&conn, &ev.session) {
                        let dur = at - started;
                        if dur >= LONG_COMMAND_MS {
                            long_finish = Some(dur);
                        }
                    }
                    db::finish_latest_command(&conn, &ev.session, at, code, ev.command_text.as_deref());
                }
                if let Some(cwd) = &ev.cwd {
                    let name = project_name(cwd);
                    db::update_project(&conn, &ev.session, cwd, name.as_deref());
                }
            }
            if let Some(dur) = long_finish {
                notify::notify("Aran Terminal", &format!("{label} finished a command ({}s)", dur / 1000));
            }
            if ev.state == AttentionState::Waiting
                && lock(&self.mgr).should_notify_waiting(&ev.session, &label)
            {
                notify::notify("Aran Terminal", &format!("{label} is waiting for your input"));
            }
        }
        self.ctx.request_repaint();
    }

    /// One detection tick per second drives the prompt-quiet (T_wait) heuristic.
    pub fn spawn_ticker(&self) {
        let core = self.clone();
        let _ = std::thread::Builder::new().name("ticker".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let evs = lock(&core.det).tick(Instant::now());
            if !evs.is_empty() {
                core.process_state_events(evs);
            }
        });
    }

    /// Samples per-session memory every 2s with one `ps` call.
    pub fn spawn_mem_poller(&self) {
        let mgr = self.mgr.clone();
        let _ = std::thread::Builder::new().name("mem".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            let table = sample_proc_table();
            let mut g = lock(&mgr);
            for s in g.sessions.values_mut() {
                s.rss_kb = s.pid.and_then(|p| subtree_rss(p, &table));
            }
        });
    }

    /// Mark every live session closed (app quit) and stop the shells.
    pub fn shutdown(&self) {
        let now = now_ms();
        let mut g = lock(&self.mgr);
        let conn = lock(&self.db);
        for (id, s) in g.sessions.iter_mut() {
            db::mark_session_closed(&conn, id, now);
            let _ = s.child.kill();
        }
    }
}

impl SessionManager {
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
                s.project = project_name(cwd);
            }
            s.name
                .clone()
                .or_else(|| s.project.clone())
                .unwrap_or_else(|| ev.session.chars().take(8).collect())
        } else {
            ev.session.chars().take(8).collect()
        }
    }

    pub fn close(&mut self, id: &str) {
        if let Some(mut s) = self.sessions.remove(id) {
            let _ = s.child.kill();
        }
        self.order.retain(|x| x != id);
    }

    /// Drop sessions whose shell exited. Returns their ids.
    pub fn reap_exited(&mut self) -> Vec<SessionId> {
        let dead: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.exited.load(Ordering::Acquire))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.close(id);
        }
        dead
    }

    pub fn raise_cap(&mut self) {
        self.cap += 1;
        self.override_count += 1;
    }

    #[allow(dead_code)] // wired up with Focus View
    pub fn set_notification_focus(&mut self, id: Option<SessionId>) -> Vec<String> {
        self.notification_focus = id;
        std::mem::take(&mut self.muted_waits)
    }

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

// ---- memory sampling (same approach as the Tauri build) ----

struct ProcTable {
    rss: HashMap<u32, i64>,
    children: HashMap<u32, Vec<u32>>,
}

fn sample_proc_table() -> ProcTable {
    let mut rss = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,rss="]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            if let (Some(pid), Some(ppid), Some(r)) = (it.next(), it.next(), it.next()) {
                if let (Ok(pid), Ok(ppid), Ok(r)) = (pid.parse::<u32>(), ppid.parse::<u32>(), r.parse::<i64>()) {
                    rss.insert(pid, r);
                    children.entry(ppid).or_default().push(pid);
                }
            }
        }
    }
    ProcTable { rss, children }
}

fn subtree_rss(root: u32, table: &ProcTable) -> Option<i64> {
    let mut total = *table.rss.get(&root)?;
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

/// Resident memory (KB) of this process — no webview helpers to add up any more.
pub fn app_rss_kb() -> Option<i64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtree_sums_descendants() {
        let mut rss = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, ppid, r) in [(100u32, 1u32, 38_000i64), (200, 100, 500), (300, 200, 81_520)] {
            rss.insert(pid, r);
            children.entry(ppid).or_default().push(pid);
        }
        let t = ProcTable { rss, children };
        assert_eq!(subtree_rss(200, &t), Some(82_020));
        assert_eq!(subtree_rss(999, &t), None);
    }

    #[test]
    fn names_folders() {
        assert_eq!(project_name("/Users/arun").as_deref(), Some("arun"));
        assert_eq!(project_name("/").as_deref(), Some("/"));
    }
}
