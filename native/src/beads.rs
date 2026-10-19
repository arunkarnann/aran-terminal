//! Issues sidebar — a GUI over the local Beads (`bd`) tracker of the active project.
//!
//! Every `bd` call runs on one sequential worker thread (embedded Dolt can lock against
//! concurrent calls, even our own), with a timeout, and reports back over a channel. The
//! UI thread never blocks: it drains results each frame and asks for repaints.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, CornerRadius, Frame, Key, Margin, RichText, Sense, Stroke, Vec2};
use serde::{Deserialize, Deserializer};

use crate::session::now_ms;
use crate::theme;
use crate::timefmt::fmt_stamp;

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_secs(15);
const STAT_EVERY: Duration = Duration::from_secs(2);
const CLOSED_PAGE: usize = 50;
const EDIT_STATUSES: [&str; 5] = ["open", "in_progress", "blocked", "deferred", "closed"];
const CREATE_TYPES: [&str; 5] = ["task", "bug", "feature", "epic", "chore"];
const PRIORITY: [&str; 5] = ["Critical", "High", "Medium", "Low", "Backlog"];

// ---------------------------------------------------------------- JSON model

/// `null` → `T::default()` (bd emits nulls for some optional fields).
fn nd<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A dependency edge as `bd list --json` reports it.
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct DepEdge {
    #[serde(deserialize_with = "nd")]
    depends_on_id: String,
    #[serde(rename = "type", deserialize_with = "nd")]
    kind: String,
}

/// One row of `bd list --json`.
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct Issue {
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(deserialize_with = "nd")]
    status: String,
    #[serde(deserialize_with = "nd")]
    priority: i64,
    #[serde(deserialize_with = "nd")]
    issue_type: String,
    assignee: Option<String>,
    #[serde(deserialize_with = "nd")]
    updated_at: String,
    #[serde(deserialize_with = "nd")]
    closed_at: String,
    #[serde(deserialize_with = "nd")]
    labels: Vec<String>,
    #[serde(deserialize_with = "nd")]
    dependencies: Vec<DepEdge>,
    #[serde(deserialize_with = "nd")]
    comment_count: i64,
    parent: Option<String>,
}

impl Issue {
    /// Open issues this one is blocked by (from the list's edges; targets must be in `open`).
    fn blocked_by<'a>(&'a self, open: &'a HashSet<&str>) -> impl Iterator<Item = &'a str> + 'a {
        self.dependencies
            .iter()
            .filter(|d| d.kind == "blocks" && open.contains(d.depends_on_id.as_str()))
            .map(|d| d.depends_on_id.as_str())
    }
}

/// An issue embedded in `bd show --json` (dependency / dependent).
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct Linked {
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(deserialize_with = "nd")]
    status: String,
    #[serde(deserialize_with = "nd")]
    dependency_type: String,
}

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct Comment {
    #[serde(deserialize_with = "nd")]
    author: String,
    #[serde(deserialize_with = "nd")]
    text: String,
    #[serde(deserialize_with = "nd")]
    created_at: String,
}

/// `bd show --json` (first element of the returned array).
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct ShowIssue {
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    description: String,
    #[serde(deserialize_with = "nd")]
    notes: String,
    #[serde(deserialize_with = "nd")]
    design: String,
    #[serde(deserialize_with = "nd")]
    acceptance_criteria: String,
    #[serde(deserialize_with = "nd")]
    owner: String,
    #[serde(deserialize_with = "nd")]
    created_by: String,
    #[serde(deserialize_with = "nd")]
    created_at: String,
    #[serde(deserialize_with = "nd")]
    updated_at: String,
    #[serde(deserialize_with = "nd")]
    started_at: String,
    #[serde(deserialize_with = "nd")]
    closed_at: String,
    #[serde(deserialize_with = "nd")]
    close_reason: String,
    #[serde(deserialize_with = "nd")]
    labels: Vec<String>,
    #[serde(deserialize_with = "nd")]
    dependencies: Vec<Linked>,
    #[serde(deserialize_with = "nd")]
    dependents: Vec<Linked>,
    #[serde(deserialize_with = "nd")]
    comments: Vec<Comment>,
    epic_total_children: Option<i64>,
    epic_closed_children: Option<i64>,
}

/// Parse stdout as JSON; tolerate stray text before the payload.
fn parse_payload<T: for<'de> Deserialize<'de>>(stdout: &str) -> Result<T, String> {
    let s = stdout.trim();
    match serde_json::from_str::<T>(s) {
        Ok(v) => Ok(v),
        Err(first) => {
            let start = s.find(['[', '{']).filter(|&i| i > 0);
            match start.map(|i| serde_json::from_str::<T>(&s[i..])) {
                Some(Ok(v)) => Ok(v),
                _ => Err(format!("couldn't read bd output: {first}")),
            }
        }
    }
}

fn parse_list(stdout: &str) -> Result<Vec<Issue>, String> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let v: Option<Vec<Issue>> = parse_payload(stdout)?;
    Ok(v.unwrap_or_default().into_iter().filter(|i| !i.id.is_empty()).collect())
}

fn parse_show(stdout: &str) -> Result<ShowIssue, String> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<ShowIssue>),
        One(Box<ShowIssue>),
    }
    match parse_payload::<OneOrMany>(stdout)? {
        OneOrMany::Many(v) => v.into_iter().next().ok_or_else(|| "issue not found".to_string()),
        OneOrMany::One(i) => Ok(*i),
    }
}

/// The message to show for a failed call: bd's JSON `{"error": …}` on stderr (or stdout),
/// else the first lines of stderr.
fn error_text(stderr: &str, stdout: &str, code: Option<i32>) -> String {
    #[derive(Deserialize)]
    struct ErrJson {
        error: String,
    }
    for s in [stderr, stdout] {
        if let Ok(e) = parse_payload::<ErrJson>(s) {
            return e.error;
        }
    }
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    if !lines.is_empty() {
        let mut msg = lines.join(" · ");
        if let Some(rest) = msg.strip_prefix("Error: ") {
            msg = rest.to_string();
        }
        return msg;
    }
    match code {
        Some(c) => format!("bd exited with status {c}"),
        None => "bd was terminated".into(),
    }
}

/// RFC 3339 ("2026-09-19T05:36:30Z", optional fraction / offset) → epoch ms.
fn parse_ts(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    if s.len() < 19 || s.as_bytes().get(4) != Some(&b'-') || s.as_bytes().get(13) != Some(&b':') {
        return None;
    }
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        rest = &frac[digits.len()..];
        ms = format!("{digits:0<3}").get(..3)?.parse().ok()?;
    }
    let offset_min = match rest {
        "" | "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) => {
            let v = o.get(1..3)?.parse::<i64>().ok()? * 60 + o.get(4..6)?.parse::<i64>().ok()?;
            if o.starts_with('-') {
                -v
            } else {
                v
            }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi - offset_min) * 60_000 + se * 1000 + ms)
}

// ---------------------------------------------------------------- argv builders

fn list_args() -> Vec<String> {
    // --all: closed + frozen (deferred/pinned) too; closed is filtered client-side.
    vec!["list".into(), "--json".into(), "--all".into(), "--limit=0".into()]
}

fn ready_args() -> Vec<String> {
    vec!["ready".into(), "--json".into(), "--limit=0".into()]
}

fn show_args(id: &str) -> Vec<String> {
    vec!["show".into(), "--json".into(), format!("--id={id}")]
}

fn create_args(title: &str, kind: &str, priority: i64) -> Vec<String> {
    vec![
        "create".into(),
        "--json".into(),
        format!("--title={}", title.trim()),
        format!("--type={kind}"),
        format!("--priority={}", priority.clamp(0, 4)),
    ]
}

/// The right command for a status change: close / reopen / update --status.
fn status_args(id: &str, from: &str, to: &str) -> Vec<String> {
    let mut a: Vec<String> = if to == "closed" {
        vec!["close".into(), "--json".into()]
    } else if from == "closed" && to == "open" {
        vec!["reopen".into(), "--json".into()]
    } else {
        vec!["update".into(), "--json".into(), format!("--status={to}")]
    };
    a.push("--".into());
    a.push(id.into());
    a
}

fn close_args(id: &str, reason: &str) -> Vec<String> {
    let mut a: Vec<String> = vec!["close".into(), "--json".into()];
    let r = reason.trim();
    if !r.is_empty() {
        a.push(format!("--reason={r}"));
    }
    a.push("--".into());
    a.push(id.into());
    a
}

fn reopen_args(id: &str) -> Vec<String> {
    vec!["reopen".into(), "--json".into(), "--".into(), id.into()]
}

fn priority_args(id: &str, p: i64) -> Vec<String> {
    vec!["update".into(), "--json".into(), format!("--priority={}", p.clamp(0, 4)), "--".into(), id.into()]
}

fn init_args() -> Vec<String> {
    vec!["init".into(), "--non-interactive".into()]
}

// ---------------------------------------------------------------- discovery

/// Nearest ancestor (inclusive) with a real Beads workspace in `.beads/`. A bare
/// `~/.beads` (bd's global registry) doesn't count.
fn find_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| {
            let b = dir.join(".beads");
            b.is_dir() && ["metadata.json", "config.yaml", "embeddeddolt", "beads.db"].iter().any(|m| b.join(m).exists())
        })
        .map(Path::to_path_buf)
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Candidate `bd` locations, in priority order (a Finder-launched app has a minimal PATH).
fn bd_candidates(env_bd: Option<&str>, home: Option<&str>, path_var: Option<&str>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = env_bd.filter(|p| !p.is_empty()) {
        v.push(PathBuf::from(p));
    }
    if let Some(h) = home.filter(|h| !h.is_empty()) {
        v.push(Path::new(h).join(".local/bin/bd"));
    }
    v.push(PathBuf::from("/opt/homebrew/bin/bd"));
    v.push(PathBuf::from("/usr/local/bin/bd"));
    if let Some(h) = home.filter(|h| !h.is_empty()) {
        v.push(Path::new(h).join("go/bin/bd"));
    }
    if let Some(path) = path_var {
        v.extend(path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("bd")));
    }
    v
}

fn find_bd() -> Option<PathBuf> {
    let env_bd = std::env::var("BD_PATH").ok();
    let home = std::env::var("HOME").ok();
    let path = std::env::var("PATH").ok();
    bd_candidates(env_bd.as_deref(), home.as_deref(), path.as_deref())
        .into_iter()
        .find(|p| is_executable(p))
}

/// PATH for the child: bd's own dir and the usual tool dirs first (git etc.), then ours.
fn child_path(bin: &Path, existing: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = bin.parent() {
        parts.push(d.to_string_lossy().into_owned());
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        parts.push(d.into());
    }
    if let Some(e) = existing {
        parts.extend(e.split(':').filter(|d| !d.is_empty()).map(String::from));
    }
    let mut seen = HashSet::new();
    parts.retain(|p| seen.insert(p.clone()));
    parts.join(":")
}

/// Cheap change signal: mtimes of bd's export files (`last-touched` is rewritten by
/// `bd show`, so it's deliberately not used).
fn fingerprint(root: &Path) -> Vec<Option<SystemTime>> {
    ["issues.jsonl", "interactions.jsonl"]
        .iter()
        .map(|f| std::fs::metadata(root.join(".beads").join(f)).and_then(|m| m.modified()).ok())
        .collect()
}

// ---------------------------------------------------------------- worker

/// Run `bd` with separate argv entries (no shell), killing it after `timeout`.
fn run_bd(bin: &Path, dir: &Path, args: &[String], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(dir)
        .env("BD_NON_INTERACTIVE", "1")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("PATH", child_path(bin, std::env::var("PATH").ok().as_deref()))
        .env_remove("BEADS_DIR")
        .env_remove("BEADS_DB")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run bd: {e}"))?;
    // Drain both pipes on their own threads so a big payload can't stall the child.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    };
    let out_rx = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err_rx = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("bd timed out after {}s (database busy?)", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("bd: {e}")),
        }
    };
    // A grandchild could keep the pipes open; don't wait on it forever.
    let grace = Duration::from_secs(2);
    let stdout = String::from_utf8_lossy(&out_rx.recv_timeout(grace).unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_rx.recv_timeout(grace).unwrap_or_default()).into_owned();
    if status.success() {
        Ok(stdout)
    } else {
        let mut msg = error_text(&stderr, &stdout, status.code());
        if msg.to_lowercase().contains("lock") {
            msg.push_str(" — another bd process may be using the database; try refresh");
        }
        Err(msg)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum JobKind {
    List,
    Ready,
    Show(String),
    Write { what: String, id: Option<String> },
    Init,
}

struct Job {
    bin: PathBuf,
    dir: PathBuf,
    gen: u64,
    kind: JobKind,
    args: Vec<String>,
}

struct Done {
    gen: u64,
    kind: JobKind,
    result: Result<String, String>,
}

fn spawn_worker(ctx: egui::Context) -> (Sender<Job>, Receiver<Done>) {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::channel::<Done>();
    thread::spawn(move || {
        // Ends when the panel (the Sender) is dropped.
        for job in job_rx {
            let result = run_bd(&job.bin, &job.dir, &job.args, TIMEOUT);
            if done_tx.send(Done { gen: job.gen, kind: job.kind, result }).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (job_tx, done_rx)
}

// ---------------------------------------------------------------- presentation helpers

fn status_label(s: &str) -> String {
    match s {
        "in_progress" => "In progress".into(),
        "blocked" => "Blocked".into(),
        "open" => "Open".into(),
        "deferred" => "Deferred".into(),
        "closed" => "Closed".into(),
        other => {
            let t = other.replace('_', " ");
            let mut c = t.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => "Unknown".into(),
            }
        }
    }
}

fn status_color(s: &str) -> Color32 {
    match s {
        "in_progress" => theme::t().primary,
        "blocked" => theme::t().error,
        "open" => theme::t().text,
        "closed" => theme::t().running,
        "pinned" | "hooked" => theme::t().waiting,
        _ => theme::t().muted,
    }
}

fn priority_color(p: i64) -> Color32 {
    match p {
        0 => theme::t().error,
        1 => theme::t().waiting,
        _ => theme::t().muted,
    }
}

fn type_tag(t: &str) -> (String, Color32) {
    match t {
        "bug" => ("bug".into(), theme::t().error),
        "feature" => ("feat".into(), theme::t().primary),
        "epic" => ("epic".into(), theme::t().waiting),
        "decision" => ("dec".into(), theme::t().muted),
        "merge-request" => ("mr".into(), theme::t().muted),
        "" => ("task".into(), theme::t().muted),
        other => (other.into(), theme::t().muted),
    }
}

/// Section order: in_progress, blocked, open, deferred, anything else (A–Z), closed.
fn section_rank(s: &str) -> u8 {
    match s {
        "in_progress" => 0,
        "blocked" => 1,
        "open" => 2,
        "deferred" => 3,
        "closed" => 9,
        _ => 4,
    }
}

/// Group into status sections (in display order); within a section sort by priority then
/// most recently updated — closed ones by most recently closed.
fn group_by_status<'a>(issues: &[&'a Issue]) -> Vec<(String, Vec<&'a Issue>)> {
    let mut map: HashMap<&str, Vec<&Issue>> = HashMap::new();
    for i in issues {
        map.entry(i.status.as_str()).or_default().push(i);
    }
    let mut out: Vec<(String, Vec<&Issue>)> = map
        .into_iter()
        .map(|(s, mut v)| {
            if s == "closed" {
                v.sort_by(|a, b| b.closed_at.cmp(&a.closed_at).then(b.updated_at.cmp(&a.updated_at)));
            } else {
                v.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.updated_at.cmp(&a.updated_at)).then(a.id.cmp(&b.id)));
            }
            (s.to_string(), v)
        })
        .collect();
    out.sort_by(|a, b| section_rank(&a.0).cmp(&section_rank(&b.0)).then(a.0.cmp(&b.0)));
    out
}

fn matches_search(i: &Issue, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || i.id.to_lowercase().contains(&q) || i.title.to_lowercase().contains(&q)
}

fn small(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).color(color).size(11.0)
}

fn status_mark(ui: &mut egui::Ui, status: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let c = status_color(status);
    let p = ui.painter();
    match status {
        "closed" | "blocked" => {
            p.circle_filled(rect.center(), 6.0, c);
            let mark = if status == "closed" { "✓" } else { "!" };
            p.text(rect.center(), egui::Align2::CENTER_CENTER, mark, egui::FontId::proportional(9.0), theme::t().bg);
        }
        "in_progress" => {
            p.circle_stroke(rect.center(), 5.5, Stroke::new(1.5, c));
            p.circle_filled(rect.center(), 3.0, c);
        }
        "deferred" => {
            p.circle_stroke(rect.center(), 5.5, Stroke::new(1.0, c.gamma_multiply(0.7)));
        }
        _ => {
            p.circle_stroke(rect.center(), 5.5, Stroke::new(1.5, c));
        }
    }
    resp.on_hover_text(status_label(status))
}

fn stamp(s: &str, now: i64) -> Option<String> {
    parse_ts(s).map(|ms| fmt_stamp(ms, now))
}

// ---------------------------------------------------------------- panel

/// The one expanded issue.
struct Detail {
    id: String,
    data: Option<ShowIssue>,
    error: Option<String>,
    loading: bool,
    /// `updated_at` of the list row when `data` was fetched (re-fetch when it moves).
    seen_updated: String,
    reason: String,
}

enum Action {
    ToggleSection(String),
    ToggleOpen(String),
    SetStatus { id: String, from: String, to: String },
    SetPriority { id: String, p: i64 },
    Close { id: String, reason: String },
    Reopen(String),
    MoreClosed,
}

pub struct IssuesPanel {
    ctx: egui::Context,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    bin: Option<PathBuf>,
    bin_checked: Option<Instant>,

    cwd: Option<String>,
    root: Option<PathBuf>,
    root_checked: Option<Instant>,
    gen: u64,

    issues: Vec<Issue>,
    loaded: bool,
    open_count: Option<usize>,
    ready_ids: Option<HashSet<String>>,
    list_inflight: bool,
    ready_inflight: bool,
    pending: usize,
    last_refresh: Option<Instant>,
    last_shown: Option<Instant>,
    last_stat: Option<Instant>,
    fingerprint: Vec<Option<SystemTime>>,
    error: Option<String>,

    // UI state.
    search: String,
    ready_only: bool,
    show_closed: bool,
    type_filter: Option<String>,
    collapsed: HashSet<String>,
    closed_limit: usize,
    draft: String,
    new_type: String,
    new_priority: i64,
    last_draft: String,
    detail: Option<Detail>,
    arm_init: bool,
}

impl IssuesPanel {
    pub fn new(ctx: egui::Context) -> Self {
        let (jobs, done) = spawn_worker(ctx.clone());
        IssuesPanel {
            ctx,
            jobs,
            done,
            bin: None,
            bin_checked: None,
            cwd: None,
            root: None,
            root_checked: None,
            gen: 0,
            issues: Vec::new(),
            loaded: false,
            open_count: None,
            ready_ids: None,
            list_inflight: false,
            ready_inflight: false,
            pending: 0,
            last_refresh: None,
            last_shown: None,
            last_stat: None,
            fingerprint: Vec::new(),
            error: None,
            search: String::new(),
            ready_only: false,
            show_closed: false,
            type_filter: None,
            collapsed: HashSet::from(["closed".to_string()]),
            closed_limit: CLOSED_PAGE,
            draft: String::new(),
            new_type: "task".into(),
            new_priority: 2,
            last_draft: String::new(),
            detail: None,
            arm_init: false,
        }
    }

    /// Open (non-closed) issue count for the header badge, once loaded.
    pub fn open_count(&self) -> Option<usize> {
        self.open_count
    }

    // ------------------------------------------------ plumbing

    fn enqueue(&mut self, dir: PathBuf, kind: JobKind, args: Vec<String>) -> bool {
        let Some(bin) = self.bin.clone() else {
            return false;
        };
        let job = Job { bin, dir, gen: self.gen, kind, args };
        if self.jobs.send(job).is_err() {
            self.error = Some("issue worker stopped".into());
            return false;
        }
        self.pending += 1;
        true
    }

    fn refresh(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.last_refresh = Some(Instant::now());
        if !self.list_inflight && self.enqueue(root.clone(), JobKind::List, list_args()) {
            self.list_inflight = true;
        }
        if self.ready_only && !self.ready_inflight && self.enqueue(root, JobKind::Ready, ready_args()) {
            self.ready_inflight = true;
        }
    }

    fn load_detail(&mut self) {
        let (Some(root), Some(d)) = (self.root.clone(), self.detail.as_mut()) else {
            return;
        };
        d.loading = true;
        let id = d.id.clone();
        self.enqueue(root, JobKind::Show(id.clone()), show_args(&id));
    }

    fn write(&mut self, what: &str, id: Option<String>, args: Vec<String>) {
        if let Some(root) = self.root.clone() {
            self.enqueue(root, JobKind::Write { what: what.into(), id }, args);
        }
    }

    fn set_root(&mut self, root: Option<PathBuf>) {
        if root == self.root {
            return;
        }
        self.root = root;
        self.gen += 1;
        self.issues.clear();
        self.loaded = false;
        self.open_count = None;
        self.ready_ids = None;
        self.list_inflight = false;
        self.ready_inflight = false;
        self.detail = None;
        self.error = None;
        self.fingerprint.clear();
        self.closed_limit = CLOSED_PAGE;
        self.type_filter = None;
        self.arm_init = false;
        self.refresh();
    }

    fn drain(&mut self) {
        while let Ok(d) = self.done.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if d.gen != self.gen {
                continue;
            }
            match d.kind {
                JobKind::List => {
                    self.list_inflight = false;
                    // Record even on failure so a changed file can't trigger a retry loop.
                    if let Some(r) = &self.root {
                        self.fingerprint = fingerprint(r);
                    }
                    match d.result.and_then(|s| parse_list(&s)) {
                        Ok(v) => {
                            self.open_count = Some(v.iter().filter(|i| i.status != "closed").count());
                            self.issues = v;
                            self.loaded = true;
                            self.error = None;
                            // Re-fetch the open detail if its issue changed.
                            let moved = self.detail.as_ref().is_some_and(|det| {
                                !det.loading
                                    && self.issues.iter().find(|i| i.id == det.id).is_some_and(|i| i.updated_at != det.seen_updated)
                            });
                            if moved {
                                self.load_detail();
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                JobKind::Ready => {
                    self.ready_inflight = false;
                    match d.result.and_then(|s| parse_list(&s)) {
                        Ok(v) => self.ready_ids = Some(v.into_iter().map(|i| i.id).collect()),
                        Err(e) => self.error = Some(e),
                    }
                }
                JobKind::Show(id) => {
                    let updated = self.issues.iter().find(|i| i.id == id).map(|i| i.updated_at.clone()).unwrap_or_default();
                    if let Some(det) = self.detail.as_mut().filter(|det| det.id == id) {
                        det.loading = false;
                        det.seen_updated = updated;
                        match d.result.and_then(|s| parse_show(&s)) {
                            Ok(data) if !data.id.is_empty() && data.id != id => det.error = Some(format!("bd returned {} for {id}", data.id)),
                            Ok(data) => {
                                det.data = Some(data);
                                det.error = None;
                            }
                            Err(e) => det.error = Some(e),
                        }
                    }
                }
                JobKind::Write { what, id } => match d.result {
                    Ok(_) => {
                        self.error = None;
                        if what == "create" {
                            self.last_draft.clear();
                        }
                        self.refresh();
                        if id.is_some() && self.detail.as_ref().map(|det| &det.id) == id.as_ref() {
                            self.load_detail();
                        }
                    }
                    Err(e) => {
                        if what == "create" && self.draft.is_empty() {
                            self.draft = std::mem::take(&mut self.last_draft);
                        }
                        self.error = Some(format!("{what} failed: {e}"));
                        self.refresh();
                    }
                },
                JobKind::Init => match d.result {
                    Ok(_) => {
                        self.root_checked = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(format!("bd init failed: {e}")),
                },
            }
        }
    }

    /// Resolve binary + project root, and decide whether a refresh is due.
    fn tick(&mut self, cwd: Option<String>) {
        let now = Instant::now();
        let recheck = |t: Option<Instant>| t.is_none_or(|t| now.duration_since(t) >= POLL);
        if self.bin.is_none() && recheck(self.bin_checked) {
            self.bin = find_bd();
            self.bin_checked = Some(now);
        }
        if cwd != self.cwd || (self.root.is_none() && recheck(self.root_checked)) {
            self.cwd = cwd;
            self.root_checked = Some(now);
            let root = self.cwd.as_deref().and_then(|c| find_root(Path::new(c)));
            self.set_root(root);
        }
        let was_hidden = self.last_shown.is_none_or(|t| now.duration_since(t) > Duration::from_secs(3));
        self.last_shown = Some(now);
        let Some(root) = self.root.clone() else {
            return;
        };
        // Keep ticking while the pane is visible (egui sleeps when idle).
        self.ctx.request_repaint_after(STAT_EVERY);
        let age = self.last_refresh.map(|t| now.duration_since(t));
        let mut due = age.is_none_or(|a| a >= POLL) || (was_hidden && age.is_none_or(|a| a >= Duration::from_secs(5)));
        if !due && self.last_stat.is_none_or(|t| now.duration_since(t) >= STAT_EVERY) {
            self.last_stat = Some(now);
            due = self.loaded && !self.list_inflight && fingerprint(&root) != self.fingerprint;
        }
        if due {
            self.refresh();
        }
    }

    // ------------------------------------------------ UI

    /// Draw the pane for the active session's folder.
    pub fn show(&mut self, ui: &mut egui::Ui, cwd: Option<String>) {
        self.drain();
        self.tick(cwd);

        if self.bin.is_none() {
            ui.label(RichText::new("Beads (bd) isn't installed").color(theme::t().text).size(12.0).strong());
            ui.label(small("Install it with  brew install beads  (or set BD_PATH), then refresh.", theme::t().muted));
            if ui.small_button("Look again").clicked() {
                self.bin_checked = None;
            }
            return;
        }
        let Some(root) = self.root.clone() else {
            self.empty_state(ui);
            return;
        };

        self.header(ui, &root);
        self.create_box(ui);
        self.filters(ui);
        if let Some(e) = &self.error {
            ui.add(egui::Label::new(small(e, theme::t().error)).wrap());
        }
        ui.add_space(6.0);

        if !self.loaded {
            if self.list_inflight {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small("Loading issues…", theme::t().muted));
                });
            } else {
                ui.label(small("Couldn't load issues. Press ⟳ to retry.", theme::t().muted));
            }
            return;
        }
        self.list(ui);
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let Some(cwd) = self.cwd.clone() else {
            ui.label(small("Open a terminal in a project folder to see its Beads issues.", theme::t().muted));
            return;
        };
        let name = Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| cwd.clone());
        ui.label(RichText::new("No Beads tracker found").color(theme::t().text).size(12.0).strong());
        ui.add(egui::Label::new(small(format!("Nothing in {name} or its parent folders has a .beads/ directory."), theme::t().muted)).wrap());
        ui.add_space(4.0);
        ui.label(small("Run  bd init  in the project's root to start tracking issues.", theme::t().muted));
        ui.add_space(6.0);
        if self.arm_init {
            ui.add(egui::Label::new(small(format!("Create .beads/ in {cwd}?"), theme::t().text)).wrap());
            ui.horizontal(|ui| {
                if ui.small_button(RichText::new("Create tracker").color(theme::t().primary)).clicked() {
                    self.arm_init = false;
                    self.enqueue(PathBuf::from(&cwd), JobKind::Init, init_args());
                }
                if ui.small_button("Cancel").clicked() {
                    self.arm_init = false;
                }
            });
        } else if ui.small_button("Initialize Beads here…").on_hover_text(format!("bd init in {cwd}")).clicked() {
            self.arm_init = true;
        }
        if self.pending > 0 {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(small("Running bd init…", theme::t().muted));
            });
        }
        if let Some(e) = &self.error {
            ui.add(egui::Label::new(small(e, theme::t().error)).wrap());
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, root: &Path) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let in_progress = self.issues.iter().filter(|i| i.status == "in_progress").count();
        let blocked = self.issues.iter().filter(|i| i.status == "blocked").count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("● {name}")).color(theme::project_color(&name)).size(12.0).strong())
                .on_hover_text(root.display().to_string());
            if let Some(n) = self.open_count {
                let mut s = format!("{n} open");
                if in_progress > 0 {
                    s.push_str(&format!(" · {in_progress} active"));
                }
                ui.label(small(s, theme::t().muted));
                if blocked > 0 {
                    ui.label(small(format!("· {blocked} blocked"), theme::t().error));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("⟳").color(theme::t().muted)).frame(false)).on_hover_text("Refresh").clicked() {
                    self.refresh();
                }
                if self.pending > 0 {
                    ui.add(egui::Spinner::new().size(11.0));
                }
            });
        });
        ui.add_space(4.0);
    }

    fn create_box(&mut self, ui: &mut egui::Ui) {
        let input = ui.add(egui::TextEdit::singleline(&mut self.draft).hint_text("New issue…").desired_width(f32::INFINITY));
        if input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.draft.trim().is_empty() {
            let title = self.draft.trim().to_string();
            let args = create_args(&title, &self.new_type, self.new_priority);
            self.last_draft = std::mem::take(&mut self.draft);
            self.write("create", None, args);
            input.request_focus();
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("bd_new_type").selected_text(RichText::new(&self.new_type).size(11.0)).width(70.0).show_ui(ui, |ui| {
                for t in CREATE_TYPES {
                    ui.selectable_value(&mut self.new_type, t.to_string(), t);
                }
            });
            egui::ComboBox::from_id_salt("bd_new_prio")
                .selected_text(RichText::new(format!("P{}", self.new_priority)).size(11.0).color(priority_color(self.new_priority)))
                .width(44.0)
                .show_ui(ui, |ui| {
                    for (i, name) in PRIORITY.iter().enumerate() {
                        ui.selectable_value(&mut self.new_priority, i as i64, format!("P{i} · {name}"));
                    }
                });
            ui.label(small("↵ create", theme::t().muted));
        });
        ui.add_space(4.0);
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Filter by id or title…").desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .add(egui::Button::selectable(self.ready_only, RichText::new("Ready").size(11.0)))
                .on_hover_text("Only open issues with no active blockers (bd ready)")
                .clicked()
            {
                self.ready_only = !self.ready_only;
                if self.ready_only {
                    self.ready_ids = None;
                    self.refresh();
                }
            }
            if ui
                .add(egui::Button::selectable(self.show_closed, RichText::new("Closed").size(11.0)))
                .on_hover_text("Show closed issues")
                .clicked()
            {
                self.show_closed = !self.show_closed;
                if self.show_closed {
                    self.collapsed.remove("closed");
                }
            }
            let mut types: Vec<&str> = self.issues.iter().map(|i| i.issue_type.as_str()).filter(|t| !t.is_empty()).collect();
            types.sort_unstable();
            types.dedup();
            let mut chosen = self.type_filter.clone();
            egui::ComboBox::from_id_salt("bd_type_filter")
                .selected_text(RichText::new(chosen.as_deref().unwrap_or("All types")).size(11.0))
                .width(80.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "All types");
                    for t in types {
                        ui.selectable_value(&mut chosen, Some(t.to_string()), t);
                    }
                });
            self.type_filter = chosen;
        });
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let now = now_ms();
        let ready_pending = self.ready_only && self.ready_ids.is_none();
        let visible: Vec<&Issue> = self
            .issues
            .iter()
            .filter(|i| self.show_closed || i.status != "closed")
            .filter(|i| !self.ready_only || self.ready_ids.as_ref().is_some_and(|r| r.contains(&i.id)))
            .filter(|i| self.type_filter.as_ref().is_none_or(|t| &i.issue_type == t))
            .filter(|i| matches_search(i, &self.search))
            .collect();
        if ready_pending {
            if self.ready_inflight {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small("Finding ready work…", theme::t().muted));
                });
            } else {
                ui.label(small("Couldn't load ready work. Press ⟳ to retry.", theme::t().muted));
            }
            return;
        }
        if visible.is_empty() {
            let msg = if self.issues.is_empty() {
                "No issues yet. Type above and press Enter."
            } else {
                "No issues match these filters."
            };
            ui.label(small(msg, theme::t().muted));
            return;
        }
        let open_ids: HashSet<&str> = self.issues.iter().filter(|i| i.status != "closed").map(|i| i.id.as_str()).collect();
        let titles: HashMap<&str, &str> = self.issues.iter().map(|i| (i.id.as_str(), i.title.as_str())).collect();
        let mut actions: Vec<Action> = Vec::new();

        egui::ScrollArea::vertical().id_salt("bd_issues_scroll").auto_shrink([false, false]).show(ui, |ui| {
            for (status, list) in group_by_status(&visible) {
                let collapsed = self.collapsed.contains(&status);
                let head = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new(if collapsed { "▸" } else { "▾" }).color(theme::t().muted));
                        dot(ui, status_color(&status), 7.0);
                        ui.label(RichText::new(status_label(&status)).color(theme::t().text).size(12.0).strong());
                        ui.label(small(list.len().to_string(), theme::t().muted));
                    })
                    .response
                    .interact(Sense::click());
                if head.clicked() {
                    actions.push(Action::ToggleSection(status.clone()));
                }
                if collapsed {
                    ui.add_space(4.0);
                    continue;
                }
                let limit = if status == "closed" { self.closed_limit } else { usize::MAX };
                for i in list.iter().take(limit) {
                    let open = self.detail.as_mut().filter(|d| d.id == i.id);
                    issue_row(ui, i, open, &open_ids, &titles, now, &mut actions);
                }
                if list.len() > limit {
                    let more = list.len() - limit;
                    if ui.small_button(format!("Show {} more", more.min(CLOSED_PAGE))).clicked() {
                        actions.push(Action::MoreClosed);
                    }
                }
                ui.add_space(6.0);
            }
        });

        for a in actions {
            self.apply(a);
        }
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::ToggleSection(s) => {
                if !self.collapsed.remove(&s) {
                    self.collapsed.insert(s);
                }
            }
            Action::ToggleOpen(id) => {
                if self.detail.as_ref().is_some_and(|d| d.id == id) {
                    self.detail = None;
                } else {
                    // Make sure the row is visible (e.g. a closed dependency clicked in a detail).
                    if let Some((st, ty, found)) = self.issues.iter().find(|i| i.id == id).map(|i| (i.status.clone(), i.issue_type.clone(), matches_search(i, &self.search))) {
                        if !found {
                            self.search.clear();
                        }
                        if self.type_filter.as_ref().is_some_and(|t| *t != ty) {
                            self.type_filter = None;
                        }
                        if self.ready_only && self.ready_ids.as_ref().is_some_and(|r| !r.contains(&id)) {
                            self.ready_only = false;
                        }
                        if st == "closed" {
                            self.show_closed = true;
                        }
                        self.collapsed.remove(&st);
                    }
                    self.detail = Some(Detail { id, data: None, error: None, loading: false, seen_updated: String::new(), reason: String::new() });
                    self.load_detail();
                }
            }
            Action::SetStatus { id, from, to } => {
                let what = if to == "closed" { "close" } else { "status change" };
                let args = status_args(&id, &from, &to);
                self.write(what, Some(id), args);
            }
            Action::SetPriority { id, p } => {
                let args = priority_args(&id, p);
                self.write("priority change", Some(id), args);
            }
            Action::Close { id, reason } => {
                let args = close_args(&id, &reason);
                if let Some(d) = self.detail.as_mut() {
                    d.reason.clear();
                }
                self.write("close", Some(id), args);
            }
            Action::Reopen(id) => {
                let args = reopen_args(&id);
                self.write("reopen", Some(id), args);
            }
            Action::MoreClosed => self.closed_limit += CLOSED_PAGE,
        }
    }
}

fn dot(ui: &mut egui::Ui, c: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, c);
}

/// One issue row (+ its detail when expanded).
fn issue_row(
    ui: &mut egui::Ui,
    i: &Issue,
    detail: Option<&mut Detail>,
    open_ids: &HashSet<&str>,
    titles: &HashMap<&str, &str>,
    now: i64,
    actions: &mut Vec<Action>,
) {
    let open = detail.is_some();
    let fill = if open { theme::t().border.gamma_multiply(0.45) } else { Color32::TRANSPARENT };
    Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let body = ui
            .vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    status_mark(ui, &i.status);
                    ui.add(egui::Label::new(RichText::new(format!("P{}", i.priority)).color(priority_color(i.priority)).size(10.0).strong()).selectable(false));
                    let (tag, tc) = type_tag(&i.issue_type);
                    ui.add(egui::Label::new(RichText::new(tag).color(tc).size(10.0)).selectable(false));
                    let mut title = RichText::new(&i.title).color(theme::t().text).size(12.5);
                    if i.status == "closed" {
                        title = title.strikethrough().color(theme::t().muted);
                    }
                    ui.add(egui::Label::new(title).truncate().selectable(false)).on_hover_text(&i.title);
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_space(19.0);
                    ui.add(egui::Label::new(RichText::new(&i.id).monospace().color(theme::t().muted).size(10.5)).selectable(false));
                    if let Some(a) = i.assignee.as_deref().filter(|a| !a.is_empty()) {
                        ui.add(egui::Label::new(small(format!("@{a}"), theme::t().muted)).selectable(false));
                    }
                    if let Some(p) = i.parent.as_deref().filter(|p| !p.is_empty()) {
                        let hover = titles.get(p).map(|t| t.to_string()).unwrap_or_default();
                        ui.add(egui::Label::new(small(format!("↳ {p}"), theme::t().muted)).selectable(false)).on_hover_text(hover);
                    }
                    let blockers: Vec<&str> = i.blocked_by(open_ids).collect();
                    if !blockers.is_empty() && i.status != "closed" {
                        ui.add(egui::Label::new(small(format!("⛔ {}", blockers.len()), theme::t().error)).selectable(false))
                            .on_hover_text(format!("Blocked by {}", blockers.join(", ")));
                    }
                    if i.comment_count > 0 {
                        ui.add(egui::Label::new(small(format!("💬 {}", i.comment_count), theme::t().muted)).selectable(false));
                    }
                    for l in i.labels.iter().take(3) {
                        ui.add(egui::Label::new(small(format!("#{l}"), theme::t().primary.gamma_multiply(0.8))).selectable(false));
                    }
                });
            })
            .response
            .interact(Sense::click());
        if body.clicked() {
            actions.push(Action::ToggleOpen(i.id.clone()));
        }
        body.context_menu(|ui| {
            if ui.button("Copy id").clicked() {
                ui.ctx().copy_text(i.id.clone());
                ui.close();
            }
            if ui.button("Copy title").clicked() {
                ui.ctx().copy_text(i.title.clone());
                ui.close();
            }
        });
        if let Some(d) = detail {
            detail_ui(ui, d, i, now, actions);
        }
    });
}

fn detail_ui(ui: &mut egui::Ui, d: &mut Detail, i: &Issue, now: i64, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    // Status + priority.
    ui.horizontal(|ui| {
        let mut status = i.status.clone();
        egui::ComboBox::from_id_salt(("bd_status", &i.id)).selected_text(status_label(&status)).width(100.0).show_ui(ui, |ui| {
            for s in EDIT_STATUSES {
                ui.selectable_value(&mut status, s.to_string(), status_label(s));
            }
        });
        if status != i.status {
            actions.push(Action::SetStatus { id: i.id.clone(), from: i.status.clone(), to: status });
        }
        let mut prio = i.priority;
        egui::ComboBox::from_id_salt(("bd_prio", &i.id))
            .selected_text(RichText::new(format!("P{prio}")).color(priority_color(prio)))
            .width(50.0)
            .show_ui(ui, |ui| {
                for (n, name) in PRIORITY.iter().enumerate() {
                    ui.selectable_value(&mut prio, n as i64, format!("P{n} · {name}"));
                }
            });
        if prio != i.priority {
            actions.push(Action::SetPriority { id: i.id.clone(), p: prio });
        }
        if d.loading {
            ui.add(egui::Spinner::new().size(11.0));
        }
    });
    // Close (with optional reason) / Reopen.
    if i.status == "closed" {
        ui.horizontal(|ui| {
            if ui.small_button("Reopen").clicked() {
                actions.push(Action::Reopen(i.id.clone()));
            }
        });
    } else {
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 56.0).max(60.0);
            let r = ui.add(egui::TextEdit::singleline(&mut d.reason).hint_text("Close reason (optional)").desired_width(w));
            let enter = r.lost_focus() && ui.input(|inp| inp.key_pressed(Key::Enter));
            if ui.small_button("Close").clicked() || (enter && !d.reason.trim().is_empty()) {
                actions.push(Action::Close { id: i.id.clone(), reason: d.reason.clone() });
            }
        });
    }
    if let Some(e) = &d.error {
        ui.add(egui::Label::new(small(e, theme::t().error)).wrap());
    }
    let Some(s) = &d.data else {
        if d.loading {
            ui.label(small("Loading details…", theme::t().muted));
        }
        return;
    };
    let text_block = |ui: &mut egui::Ui, head: &str, body: &str| {
        if body.trim().is_empty() {
            return;
        }
        ui.add_space(3.0);
        ui.label(small(head, theme::t().muted).strong());
        ui.add(egui::Label::new(RichText::new(body.trim()).color(theme::t().text).size(12.0)).wrap());
    };
    text_block(ui, "Description", &s.description);
    text_block(ui, "Notes", &s.notes);
    text_block(ui, "Design", &s.design);
    text_block(ui, "Acceptance", &s.acceptance_criteria);
    if !s.close_reason.trim().is_empty() {
        text_block(ui, "Close reason", &s.close_reason);
    }
    if !s.labels.is_empty() {
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            for l in &s.labels {
                ui.label(small(format!("#{l}"), theme::t().primary));
            }
        });
    }
    if let (Some(total), Some(closed)) = (s.epic_total_children, s.epic_closed_children) {
        if total > 0 {
            ui.add_space(3.0);
            ui.label(small(format!("Children: {closed}/{total} closed"), theme::t().muted));
        }
    }
    let links = |ui: &mut egui::Ui, head: &str, v: &[Linked], actions: &mut Vec<Action>| {
        if v.is_empty() {
            return;
        }
        ui.add_space(3.0);
        ui.label(small(head, theme::t().muted).strong());
        for l in v {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                status_mark(ui, &l.status);
                let kind = match l.dependency_type.as_str() {
                    "" => String::new(),
                    k => format!("{} ", k.replace('-', " ")),
                };
                ui.add(egui::Label::new(small(kind, theme::t().muted)).selectable(false));
                let id = ui
                    .add(egui::Label::new(RichText::new(&l.id).monospace().size(10.5).color(theme::t().primary)).sense(Sense::click()).selectable(false))
                    .on_hover_text("Open this issue");
                if id.clicked() {
                    actions.push(Action::ToggleOpen(l.id.clone()));
                }
                ui.add(egui::Label::new(small(&l.title, theme::t().text)).truncate().selectable(false)).on_hover_text(&l.title);
            });
        }
    };
    links(ui, "Depends on", &s.dependencies, actions);
    links(ui, "Dependents", &s.dependents, actions);
    if !s.comments.is_empty() {
        ui.add_space(3.0);
        ui.label(small(format!("Comments ({})", s.comments.len()), theme::t().muted).strong());
        for c in &s.comments {
            let when = stamp(&c.created_at, now).unwrap_or_default();
            ui.label(small(format!("{} · {when}", c.author), theme::t().muted));
            ui.add(egui::Label::new(RichText::new(c.text.trim()).color(theme::t().text).size(12.0)).wrap());
        }
    }
    // Who / when.
    ui.add_space(3.0);
    let mut meta: Vec<String> = Vec::new();
    let who = if s.created_by.is_empty() { &s.owner } else { &s.created_by };
    if let Some(c) = stamp(&s.created_at, now) {
        meta.push(if who.is_empty() { format!("Created {c}") } else { format!("Created {c} by {who}") });
    }
    if let Some(t) = stamp(&s.started_at, now) {
        meta.push(format!("Started {t}"));
    }
    if let Some(t) = stamp(&s.closed_at, now) {
        meta.push(format!("Closed {t}"));
    } else if let Some(t) = stamp(&s.updated_at, now) {
        meta.push(format!("Updated {t}"));
    }
    for m in meta {
        ui.label(small(m, theme::t().muted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"id":"kalkyo-nr2.6","title":"M6 cleanup","status":"open","priority":1,"issue_type":"task",
       "owner":"a@b.c","created_at":"2026-09-19T05:36:30Z","updated_at":"2026-09-19T05:36:30Z",
       "dependencies":[{"issue_id":"kalkyo-nr2.6","depends_on_id":"kalkyo-nr2","type":"parent-child","metadata":"{}"}],
       "dependency_count":0,"dependent_count":0,"comment_count":0,"parent":"kalkyo-nr2","unknown_field":{"x":1}},
      {"id":"kalkyo-zyp","title":"Mobile icon","status":"in_progress","priority":0,"issue_type":"bug",
       "assignee":"Arun","labels":["ui-agent"],"updated_at":"2026-07-11T12:53:15Z","comment_count":2,
       "notes":null,"description":"x"},
      {"id":"kalkyo-c1","title":"Done thing","status":"closed","priority":2,"issue_type":"feature",
       "closed_at":"2026-09-19T05:54:10Z","close_reason":"ok","dependencies":[{"issue_id":"kalkyo-c1","depends_on_id":"kalkyo-zyp","type":"blocks"}]}
    ]"#;

    const SHOW: &str = r#"[{"id":"kalkyo-nr2.5","title":"M5","notes":"n","status":"in_progress","priority":1,
      "issue_type":"task","assignee":"Arun","created_at":"2026-09-19T05:36:28Z","created_by":"Arun",
      "updated_at":"2026-09-19T06:05:22Z","started_at":"2026-09-19T05:54:12Z",
      "dependencies":[{"id":"kalkyo-nr2","title":"Album links","status":"open","priority":1,"issue_type":"epic","dependency_type":"parent-child"}],
      "comments":[{"id":"u1","issue_id":"kalkyo-nr2.5","author":"Arun","text":"a comment","created_at":"2026-10-19T19:01:31Z"}],
      "parent":"kalkyo-nr2"}]"#;

    const SHOW_EPIC: &str = r#"[{"id":"kalkyo-nr2","title":"Album","status":"open","issue_type":"epic",
      "dependents":[{"id":"kalkyo-nr2.1","title":"M1","status":"closed","close_reason":"done","dependency_type":"parent-child"}],
      "epic_total_children":6,"epic_closed_children":4,"epic_closeable":false}]"#;

    #[test]
    fn parses_list_with_missing_and_null_fields() {
        let v = parse_list(LIST).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].parent.as_deref(), Some("kalkyo-nr2"));
        assert_eq!(v[0].dependencies[0].kind, "parent-child");
        assert!(v[1].dependencies.is_empty());
        assert_eq!(v[1].labels, vec!["ui-agent".to_string()]);
        assert_eq!(v[1].assignee.as_deref(), Some("Arun"));
        assert_eq!(v[2].closed_at, "2026-09-19T05:54:10Z");
        assert!(parse_list("").unwrap().is_empty());
        assert!(parse_list("null").unwrap().is_empty());
        assert!(parse_list("[]\n").unwrap().is_empty());
        assert!(parse_list("warning: something\n[]").unwrap().is_empty());
        assert!(parse_list("not json").is_err());
    }

    #[test]
    fn parses_show_shapes() {
        let s = parse_show(SHOW).unwrap();
        assert_eq!(s.id, "kalkyo-nr2.5");
        assert_eq!(s.dependencies[0].dependency_type, "parent-child");
        assert_eq!(s.comments[0].text, "a comment");
        let e = parse_show(SHOW_EPIC).unwrap();
        assert_eq!(e.dependents.len(), 1);
        assert_eq!((e.epic_total_children, e.epic_closed_children), (Some(6), Some(4)));
        // A bare object works too; an empty array is "not found".
        assert!(parse_show(r#"{"id":"x"}"#).is_ok());
        assert!(parse_show("[]").is_err());
    }

    #[test]
    fn extracts_errors() {
        let json = "{\n  \"error\": \"invalid status \\\"bogus\\\"\",\n  \"schema_version\": 1\n}\n";
        assert_eq!(error_text(json, "", Some(1)), "invalid status \"bogus\"");
        let plain = "Error: no beads database found\nHint: run 'bd where'\n";
        assert_eq!(error_text(plain, "", Some(1)), "no beads database found · Hint: run 'bd where'");
        assert_eq!(error_text("", "", Some(3)), "bd exited with status 3");
        assert_eq!(error_text("", "", None), "bd was terminated");
    }

    #[test]
    fn builds_argv() {
        assert_eq!(list_args(), ["list", "--json", "--all", "--limit=0"]);
        assert_eq!(ready_args(), ["ready", "--json", "--limit=0"]);
        assert_eq!(show_args("bd-1"), ["show", "--json", "--id=bd-1"]);
        // Dash-leading titles stay a flag value, never a flag.
        assert_eq!(create_args("  -x weird  ", "bug", 9), ["create", "--json", "--title=-x weird", "--type=bug", "--priority=4"]);
        assert_eq!(status_args("bd-1", "open", "closed"), ["close", "--json", "--", "bd-1"]);
        assert_eq!(status_args("bd-1", "closed", "open"), ["reopen", "--json", "--", "bd-1"]);
        assert_eq!(status_args("bd-1", "closed", "in_progress"), ["update", "--json", "--status=in_progress", "--", "bd-1"]);
        assert_eq!(status_args("bd-1", "open", "deferred"), ["update", "--json", "--status=deferred", "--", "bd-1"]);
        assert_eq!(close_args("bd-1", "  "), ["close", "--json", "--", "bd-1"]);
        assert_eq!(close_args("bd-1", "dup of bd-2"), ["close", "--json", "--reason=dup of bd-2", "--", "bd-1"]);
        assert_eq!(reopen_args("bd-1"), ["reopen", "--json", "--", "bd-1"]);
        assert_eq!(priority_args("bd-1", -1), ["update", "--json", "--priority=0", "--", "bd-1"]);
        assert_eq!(init_args(), ["init", "--non-interactive"]);
    }

    #[test]
    fn finds_root_and_ignores_bare_registry() {
        let base = std::env::temp_dir().join(format!("aran-beads-test-{}", uuid::Uuid::new_v4()));
        let proj = base.join("proj");
        let deep = proj.join("src/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        // A bare registry-only `.beads` (like ~/.beads) at the top doesn't count.
        std::fs::create_dir_all(base.join(".beads")).unwrap();
        std::fs::write(base.join(".beads/registry.json"), "{}").unwrap();
        assert_eq!(find_root(&deep), None);
        std::fs::create_dir_all(proj.join(".beads")).unwrap();
        std::fs::write(proj.join(".beads/metadata.json"), "{}").unwrap();
        assert_eq!(find_root(&deep), Some(proj.clone()));
        assert_eq!(find_root(&proj), Some(proj.clone()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bd_candidate_order_and_child_path() {
        let c = bd_candidates(Some("/x/bd"), Some("/h"), Some("/p1::/p2"));
        let c: Vec<String> = c.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            c,
            ["/x/bd", "/h/.local/bin/bd", "/opt/homebrew/bin/bd", "/usr/local/bin/bd", "/h/go/bin/bd", "/p1/bd", "/p2/bd"]
        );
        assert_eq!(bd_candidates(Some(""), None, None).len(), 2);
        let p = child_path(Path::new("/h/.local/bin/bd"), Some("/usr/bin:/custom"));
        assert!(p.starts_with("/h/.local/bin:/opt/homebrew/bin:"));
        assert!(p.ends_with(":/custom"));
        assert_eq!(p.matches("/usr/bin").count(), 1);
    }

    #[test]
    fn groups_in_section_order() {
        let mk = |id: &str, status: &str, p: i64, upd: &str| Issue {
            id: id.into(),
            status: status.into(),
            priority: p,
            updated_at: upd.into(),
            ..Default::default()
        };
        let v = [
            mk("a", "closed", 0, "1"),
            mk("b", "open", 2, "2"),
            mk("c", "open", 1, "1"),
            mk("d", "pinned", 2, "1"),
            mk("e", "in_progress", 3, "1"),
            mk("f", "deferred", 1, "1"),
            mk("g", "blocked", 1, "1"),
            mk("h", "open", 1, "3"),
        ];
        let refs: Vec<&Issue> = v.iter().collect();
        let g = group_by_status(&refs);
        let order: Vec<&str> = g.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(order, ["in_progress", "blocked", "open", "deferred", "pinned", "closed"]);
        let open: Vec<&str> = g[2].1.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(open, ["h", "c", "b"]);
        assert_eq!(status_label("in_review"), "In review");
    }

    #[test]
    fn search_and_blockers() {
        let v = parse_list(LIST).unwrap();
        assert!(matches_search(&v[1], "ZYP"));
        assert!(matches_search(&v[1], "mobile"));
        assert!(!matches_search(&v[1], "nope"));
        assert!(matches_search(&v[1], "  "));
        let open: HashSet<&str> = ["kalkyo-zyp"].into_iter().collect();
        assert_eq!(v[2].blocked_by(&open).collect::<Vec<_>>(), ["kalkyo-zyp"]);
        assert_eq!(v[0].blocked_by(&open).count(), 0);
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("2026-09-19T05:36:30Z"), Some(1_789_796_190_000));
        assert_eq!(parse_ts("2026-10-19T19:01:24.619436Z"), Some(parse_ts("2026-10-19T19:01:24Z").unwrap() + 619));
        assert_eq!(parse_ts("2026-09-19T07:36:30+02:00"), parse_ts("2026-09-19T05:36:30Z"));
        assert_eq!(parse_ts(""), None);
        assert_eq!(parse_ts("garbage-that-is-long"), None);
        assert_eq!(parse_ts("2026-09-19T05:36:30+é:00"), None);
        assert_eq!(parse_ts("2026-09-19T05:36:30.é"), None);
    }

    #[test]
    fn runs_real_process_with_timeout() {
        // Uses /bin/sh only as a stand-in binary for the runner (never bd, never a shell for bd).
        let sh = Path::new("/bin/sh");
        let dir = std::env::temp_dir();
        let ok = run_bd(sh, &dir, &["-c".into(), "printf '[]'; echo hint >&2".into()], TIMEOUT).unwrap();
        assert_eq!(ok, "[]");
        let err = run_bd(sh, &dir, &["-c".into(), "echo '{\"error\":\"boom\"}' >&2; exit 1".into()], TIMEOUT).unwrap_err();
        assert_eq!(err, "boom");
        let big = run_bd(sh, &dir, &["-c".into(), "head -c 300000 /dev/zero | tr '\\0' a".into()], TIMEOUT).unwrap();
        assert_eq!(big.len(), 300_000);
        let t = Instant::now();
        let slow = run_bd(sh, &dir, &["-c".into(), "sleep 5".into()], Duration::from_millis(300)).unwrap_err();
        assert!(slow.contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    /// Opt-in read-only run against any real tracker: `ARAN_BD_READ_DIR=… cargo test read_only`.
    #[test]
    fn read_only_real_tracker() {
        let Ok(dir) = std::env::var("ARAN_BD_READ_DIR") else {
            return;
        };
        let root = find_root(Path::new(&dir)).expect("no .beads");
        let bin = find_bd().expect("bd not found");
        let all = parse_list(&run_bd(&bin, &root, &list_args(), TIMEOUT).unwrap()).unwrap();
        let ready = parse_list(&run_bd(&bin, &root, &ready_args(), TIMEOUT).unwrap()).unwrap();
        let first = all.first().expect("empty tracker");
        let s = parse_show(&run_bd(&bin, &root, &show_args(&first.id), TIMEOUT).unwrap()).unwrap();
        println!("{} issues, {} open, {} ready; show {} ok", all.len(), all.iter().filter(|i| i.status != "closed").count(), ready.len(), s.id);
    }

    /// Opt-in end-to-end run of the exact argv against a SCRATCH tracker (it writes!):
    /// `ARAN_BD_E2E_DIR=/path/to/scratch/repo cargo test e2e -- --nocapture`
    #[test]
    fn e2e_scratch_tracker() {
        let Ok(dir) = std::env::var("ARAN_BD_E2E_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let root = find_root(&dir).expect("scratch dir has no .beads");
        // This test writes: refuse anything that isn't a temp-dir scratch tracker.
        let tmp = std::env::temp_dir();
        assert!(
            root.starts_with(&tmp) || root.starts_with("/private/tmp") || root.starts_with("/tmp"),
            "refusing to write outside a temp dir: {}",
            root.display()
        );
        let bin = find_bd().expect("bd not found");
        let run = |a: Vec<String>| run_bd(&bin, &root, &a, TIMEOUT);
        let title = format!("-e2e {}", uuid::Uuid::new_v4());
        run(create_args(&title, "bug", 1)).unwrap();
        let all = parse_list(&run(list_args()).unwrap()).unwrap();
        let it = all.iter().find(|i| i.title == title.trim()).expect("created issue not listed");
        let id = it.id.clone();
        assert_eq!((it.status.as_str(), it.priority, it.issue_type.as_str()), ("open", 1, "bug"));
        let ready = parse_list(&run(ready_args()).unwrap()).unwrap();
        assert!(ready.iter().any(|i| i.id == id));
        run(status_args(&id, "open", "in_progress")).unwrap();
        run(priority_args(&id, 0)).unwrap();
        let s = parse_show(&run(show_args(&id)).unwrap()).unwrap();
        assert_eq!(s.id, id);
        run(status_args(&id, "in_progress", "deferred")).unwrap();
        run(close_args(&id, "e2e reason")).unwrap();
        let s = parse_show(&run(show_args(&id)).unwrap()).unwrap();
        assert_eq!(s.close_reason, "e2e reason");
        run(reopen_args(&id)).unwrap();
        run(status_args(&id, "open", "closed")).unwrap();
        let all = parse_list(&run(list_args()).unwrap()).unwrap();
        let it = all.iter().find(|i| i.id == id).unwrap();
        assert_eq!((it.status.as_str(), it.priority), ("closed", 0));
        let err = run(status_args("bdt-doesnotexist", "open", "blocked")).unwrap_err();
        println!("expected error text: {err}");
        assert!(!err.is_empty());
    }
}
