//! Git sidebar — status, staging, commit, branches, sync, log and stashes for the
//! active terminal's folder. Port of the Tauri build's `GitPanel.tsx`.
//!
//! All git plumbing lives in the shared `conductor/src-tauri/src/git.rs`. Every call
//! into it (even the read-only ones — they shell out to `git`) runs on a single
//! worker thread fed by a FIFO job queue, so the UI thread never waits on git and
//! our own commands never race each other for `index.lock`. Results come back over
//! an mpsc channel, tagged with the repo they belong to so a late answer for a
//! previous folder can't overwrite the current one.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, RichText, Sense, Vec2,
};

use crate::git::{self, GitBranch, GitCommit, GitFileChange, GitRemote, GitRepo, GitStash, GitStatus};
use crate::theme;

/// Slow background poll while the pane is visible.
const POLL: Duration = Duration::from_secs(5);
/// How long a success notice stays up.
const NOTICE_FOR: Duration = Duration::from_secs(4);
const LOG_LIMIT: usize = 30;
/// Untracked-file preview caps (the file is read on the worker).
const PREVIEW_BYTES: usize = 256 * 1024;
const PREVIEW_LINES: usize = 2000;
const DIFF_FONT: f32 = 11.0;

// ---------------------------------------------------------------- worker protocol

/// Everything we know about one repo, loaded in one go on the worker.
struct Snapshot {
    status: GitStatus,
    log: Vec<GitCommit>,
    branches: Vec<GitBranch>,
    remotes: Vec<GitRemote>,
    stashes: Vec<GitStash>,
    merging: bool,
}

enum DiffSrc {
    File { path: String, staged: bool },
    Untracked { path: String },
    Commit { hash: String },
}

enum Op {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard { paths: Vec<String>, untracked: Vec<String> },
    DiscardAll,
    Commit { message: String, all: bool },
    Fetch,
    Pull,
    Push { set_upstream: bool },
    Checkout(String),
    CreateBranch(String),
    StashPush,
    StashPop,
    StashApply(usize),
    StashDrop(usize),
    AbortMerge,
}

impl Op {
    /// (in-progress label, success notice — None for quiet ops like staging).
    fn describe(&self) -> (String, Option<String>) {
        match self {
            Op::Stage(_) => ("Staging…".into(), None),
            Op::Unstage(_) => ("Unstaging…".into(), None),
            Op::Discard { .. } => ("Discarding…".into(), Some("Discarded".into())),
            Op::DiscardAll => ("Discarding…".into(), Some("Discarded all changes".into())),
            Op::Commit { .. } => ("Committing…".into(), Some("Committed".into())),
            Op::Fetch => ("Fetching…".into(), Some("Fetched".into())),
            Op::Pull => ("Pulling…".into(), Some("Pulled".into())),
            Op::Push { set_upstream: true } => ("Publishing…".into(), Some("Published branch".into())),
            Op::Push { .. } => ("Pushing…".into(), Some("Pushed".into())),
            Op::Checkout(b) => (format!("Switching to {b}…"), Some(format!("Switched to {b}"))),
            Op::CreateBranch(b) => (format!("Creating {b}…"), Some(format!("Created {b}"))),
            Op::StashPush => ("Stashing…".into(), Some("Stashed changes".into())),
            Op::StashPop => ("Popping stash…".into(), Some("Popped stash".into())),
            Op::StashApply(_) => ("Applying stash…".into(), Some("Applied stash".into())),
            Op::StashDrop(_) => ("Dropping stash…".into(), Some("Dropped stash".into())),
            Op::AbortMerge => ("Aborting…".into(), Some("Aborted".into())),
        }
    }

    fn run(self, repo: &str) -> Result<String, String> {
        match self {
            Op::Stage(p) => git::stage(repo, &p).map(|_| String::new()),
            Op::Unstage(p) => git::unstage(repo, &p).map(|_| String::new()),
            Op::Discard { paths, untracked } => git::discard(repo, &paths, &untracked).map(|_| String::new()),
            Op::DiscardAll => git::discard_all(repo).map(|_| String::new()),
            Op::Commit { message, all } => git::commit(repo, &message, all),
            Op::Fetch => git::fetch(repo),
            Op::Pull => git::pull(repo),
            Op::Push { set_upstream } => git::push(repo, set_upstream),
            Op::Checkout(b) => git::checkout(repo, &b, false),
            Op::CreateBranch(b) => git::create_branch(repo, &b, None),
            Op::StashPush => git::stash_push(repo, None, true),
            Op::StashPop => git::stash_pop(repo),
            Op::StashApply(i) => git::stash_apply(repo, i),
            Op::StashDrop(i) => git::stash_drop(repo, i),
            Op::AbortMerge => git::abort_merge(repo),
        }
    }
}

enum Job {
    Repos { cwd: String },
    Refresh { repo: String },
    Diff { repo: String, req: u64, src: DiffSrc },
    Op { repo: String, op: Op },
}

type DiffLines = Vec<(LineKind, String)>;

enum Msg {
    Repos { cwd: String, list: Vec<GitRepo> },
    Snapshot { repo: String, snap: Box<Snapshot> },
    Diff { repo: String, req: u64, body: Result<(DiffLines, String), String> },
    Op { repo: String, done: Option<String>, was_commit: bool, result: Result<String, String> },
}

fn load_snapshot(repo: &str) -> Snapshot {
    let mut status = git::status(repo);
    // `git status --porcelain=v1` C-quotes unusual paths ("sp ace.txt"); git.rs passes
    // them through verbatim, so turn them back into real paths before we act on them.
    for f in status.staged.iter_mut().chain(status.unstaged.iter_mut()) {
        f.path = unquote_path(&f.path);
    }
    for p in status.untracked.iter_mut() {
        *p = unquote_path(p);
    }
    if !status.is_repo {
        return Snapshot { status, log: Vec::new(), branches: Vec::new(), remotes: Vec::new(), stashes: Vec::new(), merging: false };
    }
    Snapshot {
        log: git::log(repo, LOG_LIMIT),
        branches: git::branches(repo),
        remotes: git::remotes(repo),
        stashes: git::stashes(repo),
        merging: git::merge_in_progress(repo),
        status,
    }
}

/// Work-tree root for a path inside a repo. `git::repos` hands back the terminal's
/// own folder even when it's a subdirectory, but porcelain paths are root-relative,
/// so every op must run from the root. (git.rs has no public helper for this.)
fn toplevel(path: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !root.is_empty()).then_some(root)
}

/// Map each discovered repo to its work-tree root (via `resolve`) and drop duplicates.
fn root_repos(list: Vec<GitRepo>, resolve: impl Fn(&str) -> Option<String>) -> Vec<GitRepo> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .map(|r| match resolve(&r.path) {
            Some(root) if root != r.path => {
                let name = Path::new(&root).file_name().and_then(|n| n.to_str()).unwrap_or(&root).to_string();
                GitRepo { path: root, name }
            }
            _ => r,
        })
        .filter(|r| seen.insert(r.path.clone()))
        .collect()
}

fn load_diff(repo: &str, src: DiffSrc) -> Result<String, String> {
    match src {
        DiffSrc::File { path, staged } => git::diff_file(repo, &path, staged),
        DiffSrc::Untracked { path } => {
            let full = Path::new(repo).join(&path);
            if full.is_dir() {
                return Ok("(untracked directory)".into());
            }
            std::fs::read(&full).map(|b| untracked_preview(&b)).map_err(|e| e.to_string())
        }
        DiffSrc::Commit { hash } => git::show(repo, &hash),
    }
}

fn run_job(job: Job) -> Vec<Msg> {
    match job {
        Job::Repos { cwd } => {
            let list = root_repos(git::repos(&cwd), toplevel);
            vec![Msg::Repos { cwd, list }]
        }
        Job::Refresh { repo } => {
            let snap = Box::new(load_snapshot(&repo));
            vec![Msg::Snapshot { repo, snap }]
        }
        Job::Diff { repo, req, src } => {
            let body = load_diff(&repo, src).map(|text| {
                let text = if text.trim().is_empty() { "(no textual diff)".to_string() } else { text };
                (classify_diff(&text), text)
            });
            vec![Msg::Diff { repo, req, body }]
        }
        Job::Op { repo, op } => {
            let was_commit = matches!(op, Op::Commit { .. });
            let (_, done) = op.describe();
            let result = op.run(&repo);
            // Every write is followed by a fresh snapshot.
            let snap = Box::new(load_snapshot(&repo));
            vec![Msg::Op { repo: repo.clone(), done, was_commit, result }, Msg::Snapshot { repo, snap }]
        }
    }
}

fn spawn_worker(ctx: egui::Context, tx: Sender<Msg>) -> Option<Sender<Job>> {
    let (jobs, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("git-panel".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                for m in run_job(job) {
                    if tx.send(m).is_err() {
                        return;
                    }
                }
                ctx.request_repaint();
            }
        })
        .ok()?;
    Some(jobs)
}

// ---------------------------------------------------------------- pure helpers

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Add,
    Del,
    Hunk,
    Meta,
    Context,
}

const META_PREFIXES: [&str; 16] = [
    "diff ", "index ", "--- ", "+++ ", "new file", "deleted file", "similarity", "dissimilarity", "rename ", "copy ",
    "old mode", "new mode", "Binary files", "commit ", "Author:", "Date:",
];

/// Classify every line of a `git diff` / `git show` output. Stateful: inside a hunk
/// any `+`/`-` line is content (so a deleted `-- comment` line, which shows up as
/// `--- comment`, is a deletion, not a file header). Tabs are expanded for display.
fn classify_diff(text: &str) -> DiffLines {
    let mut in_hunk = false;
    text.lines()
        .map(|l| {
            let kind = if l.starts_with("@@") {
                in_hunk = true;
                LineKind::Hunk
            } else {
                let hunk_kind = if in_hunk {
                    match l.as_bytes().first() {
                        Some(b'+') => Some(LineKind::Add),
                        Some(b'-') => Some(LineKind::Del),
                        Some(b' ') | Some(b'\\') | None => Some(LineKind::Context),
                        _ => None,
                    }
                } else {
                    None
                };
                match hunk_kind {
                    Some(k) => k,
                    None => {
                        in_hunk = false;
                        if META_PREFIXES.iter().any(|p| l.starts_with(p)) || l.starts_with("Merge: ") {
                            LineKind::Meta
                        } else {
                            LineKind::Context
                        }
                    }
                }
            };
            (kind, l.replace('\t', "    "))
        })
        .collect()
}

/// Undo git's C-style path quoting (`"a b.txt"`, `"caf\303\251"`, `"x\"y"`).
/// Unquoted input is returned unchanged.
fn unquote_path(p: &str) -> String {
    let Some(inner) = p.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return p.to_string();
    };
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match it.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'r') => out.push(b'\r'),
            Some(b'a') => out.push(0x07),
            Some(b'b') => out.push(0x08),
            Some(b'f') => out.push(0x0c),
            Some(b'v') => out.push(0x0b),
            Some(d @ b'0'..=b'7') => {
                let mut v = u32::from(d - b'0');
                for _ in 0..2 {
                    match it.peek() {
                        Some(&n @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(n - b'0');
                            it.next();
                        }
                        _ => break,
                    }
                }
                out.push((v & 0xff) as u8);
            }
            Some(o) => out.push(o), // \" and \\
            None => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Render an untracked file's bytes as an "all added" diff body.
fn untracked_preview(bytes: &[u8]) -> String {
    let head = &bytes[..bytes.len().min(PREVIEW_BYTES)];
    if head.contains(&0) {
        return "(binary file)".into();
    }
    let text = String::from_utf8_lossy(head);
    let mut out = String::from("@@ new untracked file @@\n");
    for (n, line) in text.lines().enumerate() {
        if n == PREVIEW_LINES {
            out.push_str("… (preview truncated)\n");
            return out;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if bytes.len() > PREVIEW_BYTES {
        out.push_str("… (preview truncated)\n");
    }
    out
}

/// Keep the current repo if it's still offered; else the folder's own repo; else the first.
fn pick_repo(prev: Option<&str>, list: &[&str], cwd: &str) -> Option<String> {
    if let Some(p) = prev.filter(|p| list.contains(p)) {
        return Some(p.to_string());
    }
    list.iter().find(|p| **p == cwd).or_else(|| list.first()).map(|p| p.to_string())
}

/// Commit needs a message and something to commit (`-a` only picks up tracked changes).
fn can_commit(message: &str, staged: usize, unstaged: usize, all: bool) -> bool {
    !message.trim().is_empty() && (staged > 0 || (all && unstaged > 0))
}

/// What to pass to `git checkout` for a branch-menu pick. A remote-tracking ref
/// (`origin/foo`) would detach HEAD, so strip the remote and let git's DWIM check
/// out (or create) the local tracking branch `foo`.
fn checkout_target(name: &str, remote: bool, remotes: &[&str]) -> String {
    if !remote {
        return name.to_string();
    }
    let mut rs: Vec<&str> = remotes.to_vec();
    rs.sort_by_key(|r| std::cmp::Reverse(r.len())); // longest remote name wins
    rs.iter()
        .find_map(|r| name.strip_prefix(r).and_then(|s| s.strip_prefix('/')))
        .filter(|s| !s.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// `git branch -r --format=%(refname:short)` lists `origin/HEAD` as plain `origin`.
fn is_remote_head_alias(name: &str, remotes: &[&str]) -> bool {
    remotes.contains(&name)
}

/// `src/app.rs` → ("app.rs", "src/").
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => (&trimmed[i + 1..], &trimmed[..=i]),
        None => (trimmed, ""),
    }
}

fn ahead_behind(ahead: i64, behind: i64) -> String {
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("↑{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("↓{behind}"));
    }
    parts.join(" ")
}

/// Last non-empty line of an op's output, shortened, for the success notice.
fn summarize(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let mut s: String = line.chars().take(100).collect();
    if line.chars().count() > 100 {
        s.push('…');
    }
    Some(s)
}

/// Push button mode for the current branch.
#[derive(Debug, PartialEq, Eq)]
enum PushMode {
    /// Has an upstream: plain push.
    Push,
    /// No upstream but an `origin` remote: `push -u origin HEAD`.
    Publish,
    /// Detached HEAD, unborn branch, or nowhere to push.
    Unavailable,
}

fn push_mode(branch: Option<&str>, has_upstream: bool, has_origin: bool) -> PushMode {
    match branch {
        None | Some("HEAD") => PushMode::Unavailable,
        Some(_) if has_upstream => PushMode::Push,
        Some(_) if has_origin => PushMode::Publish,
        Some(_) => PushMode::Unavailable,
    }
}

fn origin_web_url(remotes: &[GitRemote]) -> Option<String> {
    remotes
        .iter()
        .find(|r| r.name == "origin")
        .or_else(|| remotes.first())
        .and_then(|r| r.web_url.clone())
}

fn code_color(code: &str) -> Color32 {
    let t = theme::t();
    match code {
        "A" => t.running,
        "D" | "U" => t.error,
        "R" | "C" => t.primary,
        "M" => t.waiting,
        _ => t.muted,
    }
}

fn open_url(url: &str) {
    // `spawn`, not `output`: never wait on the browser.
    let _ = std::process::Command::new("open").arg(url).spawn();
}

// ---------------------------------------------------------------- panel state

struct DiffView {
    req: u64,
    title: String,
    /// Commit hash when this is a `git show` view (enables "open on web").
    hash: Option<String>,
    body: Option<Result<(DiffLines, String), String>>,
    /// Widest line in chars (for the horizontal scroll extent).
    max_chars: usize,
}

#[derive(Clone, PartialEq, Eq)]
enum Armed {
    Discard(String),
    DeleteUntracked(String),
    DiscardAll,
    DropStash(usize),
}

enum Action {
    Op(Op),
    Diff(String, Option<String>, DiffSrc),
    CloseDiff,
    SelectRepo(String),
    Refresh,
    Open(String),
    Arm(Option<Armed>),
    ToggleLog,
    ToggleStash,
}

pub struct GitPanel {
    ctx: egui::Context,
    jobs: Option<Sender<Job>>,
    rx: Receiver<Msg>,

    cwd: Option<String>,
    /// Repos discovered for `repos_for` (None until the first answer for this cwd).
    repos: Vec<GitRepo>,
    repos_for: Option<String>,
    repo: Option<String>,
    snap: Option<Snapshot>,
    /// Snapshots requested but not yet received (refresh coalescing).
    pending_snaps: usize,
    last_refresh: Instant,
    last_show: Option<Instant>,

    busy: Option<String>,
    ops_inflight: usize,
    notice: Option<(String, Instant)>,
    error: Option<String>,

    diff: Option<DiffView>,
    diff_req: u64,

    commit_msg: String,
    commit_all: bool,
    new_branch: Option<String>,
    armed: Option<Armed>,
    show_log: bool,
    show_stash: bool,
}

impl GitPanel {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let jobs = spawn_worker(ctx.clone(), tx);
        GitPanel {
            error: jobs.is_none().then(|| "Could not start the git worker thread".to_string()),
            ctx,
            jobs,
            rx,
            cwd: None,
            repos: Vec::new(),
            repos_for: None,
            repo: None,
            snap: None,
            pending_snaps: 0,
            last_refresh: Instant::now(),
            last_show: None,
            busy: None,
            ops_inflight: 0,
            notice: None,
            diff: None,
            diff_req: 0,
            commit_msg: String::new(),
            commit_all: false,
            new_branch: None,
            armed: None,
            show_log: true,
            show_stash: true,
        }
    }

    fn send(&mut self, job: Job) -> bool {
        let ok = self.jobs.as_ref().is_some_and(|j| j.send(job).is_ok());
        if !ok {
            self.error = Some("git worker is not running".into());
        }
        ok
    }

    /// Queue a snapshot reload. Unforced requests coalesce with one already queued.
    fn refresh(&mut self, force: bool) {
        let Some(repo) = self.repo.clone() else { return };
        if !force && self.pending_snaps > 0 {
            return;
        }
        if self.send(Job::Refresh { repo }) {
            self.pending_snaps += 1;
        }
        self.last_refresh = Instant::now();
    }

    fn set_cwd(&mut self, cwd: Option<String>) {
        self.cwd = cwd.clone();
        self.repos.clear();
        self.repos_for = None;
        self.new_branch = None;
        self.armed = None;
        match cwd {
            Some(c) => {
                self.send(Job::Repos { cwd: c });
            }
            None => self.set_repo(None),
        }
    }

    fn set_repo(&mut self, repo: Option<String>) {
        if repo == self.repo {
            return;
        }
        self.repo = repo;
        self.snap = None;
        self.diff = None;
        self.armed = None;
        self.new_branch = None;
        self.refresh(true);
    }

    fn run_op(&mut self, op: Op) {
        let Some(repo) = self.repo.clone() else { return };
        let (progress, _) = op.describe();
        if self.send(Job::Op { repo, op }) {
            self.busy = Some(progress);
            self.ops_inflight += 1;
            self.pending_snaps += 1;
            self.error = None;
            self.armed = None;
        }
    }

    fn open_diff(&mut self, title: String, hash: Option<String>, src: DiffSrc) {
        let Some(repo) = self.repo.clone() else { return };
        self.diff_req += 1;
        let req = self.diff_req;
        if self.send(Job::Diff { repo, req, src }) {
            self.diff = Some(DiffView { req, title, hash, body: None, max_chars: 0 });
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Repos { cwd, list } => {
                    if self.cwd.as_deref() != Some(cwd.as_str()) {
                        continue; // answer for a folder we've since left
                    }
                    let picked = {
                        let paths: Vec<&str> = list.iter().map(|r| r.path.as_str()).collect();
                        pick_repo(self.repo.as_deref(), &paths, &cwd)
                    };
                    self.repos = list;
                    self.repos_for = Some(cwd);
                    self.set_repo(picked);
                }
                Msg::Snapshot { repo, snap } => {
                    self.pending_snaps = self.pending_snaps.saturating_sub(1);
                    if self.repo.as_deref() == Some(repo.as_str()) {
                        self.snap = Some(*snap);
                    }
                }
                Msg::Diff { repo, req, body } => {
                    if self.repo.as_deref() != Some(repo.as_str()) {
                        continue;
                    }
                    if let Some(d) = self.diff.as_mut().filter(|d| d.req == req) {
                        d.max_chars = body
                            .as_ref()
                            .map(|(lines, _)| lines.iter().map(|(_, l)| l.chars().count()).max().unwrap_or(0))
                            .unwrap_or(0);
                        d.body = Some(body);
                    }
                }
                Msg::Op { repo, done, was_commit, result } => {
                    self.ops_inflight = self.ops_inflight.saturating_sub(1);
                    if self.ops_inflight == 0 {
                        self.busy = None;
                    }
                    if self.repo.as_deref() != Some(repo.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(out) => {
                            if was_commit {
                                self.commit_msg.clear();
                                self.commit_all = false;
                            }
                            if let Some(d) = done {
                                let text = match summarize(&out) {
                                    Some(s) if was_commit => format!("{d} · {s}"),
                                    _ => d,
                                };
                                self.notice = Some((text, Instant::now()));
                            }
                        }
                        Err(e) => {
                            let e = if e.trim().is_empty() { "git command failed".to_string() } else { e };
                            self.error = Some(e.chars().take(800).collect());
                        }
                    }
                }
            }
        }
    }

    /// Draw the pane for the active session's folder.
    pub fn show(&mut self, ui: &mut egui::Ui, cwd: Option<String>) {
        self.drain();
        let now = Instant::now();
        // We request a repaint every POLL while shown, so a longer gap means the pane
        // was hidden (other sidebar tab / sidebar closed) and the data may be stale.
        let reshown = self.last_show.is_none_or(|t| now.duration_since(t) > POLL + Duration::from_secs(2));
        self.last_show = Some(now);
        if cwd != self.cwd {
            self.set_cwd(cwd);
        } else if reshown || (self.busy.is_none() && self.last_refresh.elapsed() >= POLL) {
            self.refresh(false);
        }
        // egui only repaints on input; keep the slow poll ticking while we're shown.
        self.ctx.request_repaint_after(POLL);
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() >= NOTICE_FOR) {
            self.notice = None;
        }

        let mut actions: Vec<Action> = Vec::new();
        let th = theme::t();

        let Some(cwd) = self.cwd.clone() else {
            ui.label(RichText::new("Open a terminal to see its git status.").color(th.muted).size(12.0));
            return;
        };
        if self.repos_for.is_none() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new("Looking for a repository…").color(th.muted).size(12.0));
            });
            return;
        }
        if self.repos.is_empty() || self.snap.as_ref().is_some_and(|s| !s.status.is_repo) {
            ui.label(RichText::new("Not a git repository").color(th.text).size(12.0).strong());
            ui.add(egui::Label::new(RichText::new(&cwd).color(th.muted).size(11.0)).truncate());
            ui.add_space(4.0);
            if ui.small_button("↻ Check again").clicked() {
                self.set_cwd(Some(cwd));
            }
            return;
        }

        // Take the snapshot out so drawing can borrow `self` mutably alongside it.
        let snap = self.snap.take();
        self.header(ui, snap.as_ref(), &mut actions);
        match &snap {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new("Loading…").color(th.muted).size(12.0));
                });
            }
            Some(s) => {
                self.feedback(ui);
                if self.diff.is_some() {
                    self.diff_view(ui, s, &mut actions);
                } else {
                    egui::ScrollArea::vertical().id_salt("git-body").auto_shrink([false, false]).show(ui, |ui| {
                        self.commit_box(ui, s, &mut actions);
                        ui.add_space(6.0);
                        self.changes(ui, s, &mut actions);
                        self.commits(ui, s, &mut actions);
                        self.stashes(ui, s, &mut actions);
                    });
                }
            }
        }
        if self.snap.is_none() {
            self.snap = snap;
        }

        if ui.input(|i| i.key_pressed(Key::Escape) && i.modifiers == Modifiers::NONE) {
            self.armed = None;
        }
        self.apply(actions);
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Op(op) => self.run_op(op),
                Action::Diff(title, hash, src) => self.open_diff(title, hash, src),
                Action::CloseDiff => self.diff = None,
                Action::SelectRepo(r) => self.set_repo(Some(r)),
                Action::Refresh => {
                    if let Some(c) = self.cwd.clone() {
                        self.send(Job::Repos { cwd: c });
                    }
                    self.refresh(true);
                }
                Action::Open(url) => open_url(&url),
                Action::Arm(a) => self.armed = a,
                Action::ToggleLog => self.show_log = !self.show_log,
                Action::ToggleStash => self.show_stash = !self.show_stash,
            }
        }
    }

    // ------------------------------------------------------------ header + sync

    fn header(&mut self, ui: &mut egui::Ui, snap: Option<&Snapshot>, actions: &mut Vec<Action>) {
        let th = theme::t();
        let busy = self.busy.is_some();
        let web = snap.and_then(|s| origin_web_url(&s.remotes));

        // Repo row: picker (monorepo) or name, then ↗ / ↻ on the right.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui.add_enabled(!busy, egui::Button::new(RichText::new("↻").color(th.muted)).frame(false));
                if r.on_hover_text("Refresh").clicked() {
                    actions.push(Action::Refresh);
                }
                if let Some(url) = &web {
                    let r = ui.add(egui::Button::new(RichText::new("↗").color(th.muted)).frame(false));
                    if r.on_hover_text(format!("Open {url}")).clicked() {
                        actions.push(Action::Open(url.clone()));
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let current = self.repos.iter().find(|r| Some(&r.path) == self.repo.as_ref());
                    let name = current.map(|r| r.name.as_str()).unwrap_or("—");
                    if self.repos.len() > 1 {
                        egui::ComboBox::from_id_salt("git-repo")
                            .selected_text(RichText::new(name).size(12.0).strong())
                            .width(ui.available_width().max(60.0))
                            .truncate()
                            .show_ui(ui, |ui| {
                                for r in &self.repos {
                                    let sel = Some(&r.path) == self.repo.as_ref();
                                    if ui.selectable_label(sel, &r.name).on_hover_text(&r.path).clicked() && !sel {
                                        actions.push(Action::SelectRepo(r.path.clone()));
                                    }
                                }
                            });
                    } else {
                        let path = current.map(|r| r.path.as_str()).unwrap_or("");
                        ui.add(egui::Label::new(RichText::new(name).color(th.text).size(12.0).strong()).truncate())
                            .on_hover_text(path);
                    }
                });
            });
        });

        let Some(s) = snap else { return };
        let st = &s.status;
        let current = s.branches.iter().find(|b| b.current);
        let remote_names: Vec<&str> = s.remotes.iter().map(|r| r.name.as_str()).collect();
        let mode = push_mode(
            st.branch.as_deref(),
            current.is_some_and(|b| b.upstream.is_some()),
            remote_names.contains(&"origin"),
        );

        // Branch row: switcher + new-branch, ahead/behind on the right.
        ui.horizontal(|ui| {
            if let Some(draft) = self.new_branch.as_mut() {
                let mut create = false;
                let mut cancel = false;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Cancel").clicked() {
                        cancel = true;
                    }
                    let ok = !draft.trim().is_empty() && !busy;
                    if ui.add_enabled(ok, egui::Button::new(RichText::new("Create").size(11.0)).small()).clicked() {
                        create = true;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let id = Id::unique("git-new-branch");
                        let r = ui.add(
                            egui::TextEdit::singleline(draft)
                                .id(id)
                                .hint_text("new-branch-name")
                                .desired_width(ui.available_width())
                                .font(egui::TextStyle::Small),
                        );
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !draft.trim().is_empty() {
                            create = true;
                        }
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            cancel = true;
                        }
                    });
                });
                if create && !busy {
                    let name = draft.trim().to_string();
                    actions.push(Action::Op(Op::CreateBranch(name)));
                    self.new_branch = None;
                } else if cancel {
                    self.new_branch = None;
                }
                return;
            }

            let mut open_new = false;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ab = ahead_behind(st.ahead, st.behind);
                if !ab.is_empty() {
                    ui.label(RichText::new(ab).color(th.waiting).size(11.0))
                        .on_hover_text(format!("{} ahead · {} behind upstream", st.ahead, st.behind));
                } else if mode == PushMode::Publish {
                    ui.label(RichText::new("local").color(th.muted).size(11.0)).on_hover_text("No upstream branch yet");
                }
                let r = ui.add_enabled(!busy && !s.merging, egui::Button::new(RichText::new("+").size(12.0)).small());
                if r.on_hover_text("New branch from HEAD").clicked() {
                    open_new = true;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new("⑂").color(th.primary).size(12.0));
                    let label = match st.branch.as_deref() {
                        Some("HEAD") => "detached HEAD".to_string(),
                        Some(b) => b.to_string(),
                        None => "no commits yet".to_string(),
                    };
                    ui.add_enabled_ui(!busy && !s.merging, |ui| {
                        egui::ComboBox::from_id_salt("git-branch")
                            .selected_text(RichText::new(label).size(12.0))
                            .width(ui.available_width().max(60.0))
                            .truncate()
                            .show_ui(ui, |ui| {
                                branch_menu(ui, s, &remote_names, actions);
                            });
                    });
                });
            });
            if open_new {
                self.new_branch = Some(String::new());
                ui.memory_mut(|m| m.request_focus(Id::unique("git-new-branch")));
            }
        });

        // Sync row.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let btn = |t: &str| egui::Button::new(RichText::new(t).size(11.0)).small();
            if ui.add_enabled(!busy, btn("Fetch")).on_hover_text("git fetch --all --prune").clicked() {
                actions.push(Action::Op(Op::Fetch));
            }
            let can_pull = !busy && mode == PushMode::Push && !s.merging;
            let pull_tip = if st.behind > 0 { format!("Pull {} commit(s) (fast-forward only)", st.behind) } else { "git pull --ff-only".into() };
            if ui.add_enabled(can_pull, btn("Pull")).on_hover_text(pull_tip).clicked() {
                actions.push(Action::Op(Op::Pull));
            }
            match mode {
                PushMode::Publish => {
                    if ui.add_enabled(!busy, btn("Publish")).on_hover_text("git push -u origin HEAD").clicked() {
                        actions.push(Action::Op(Op::Push { set_upstream: true }));
                    }
                }
                _ => {
                    let ok = !busy && mode == PushMode::Push && st.ahead > 0;
                    let tip = match mode {
                        PushMode::Unavailable => "No branch / remote to push to".to_string(),
                        _ if st.ahead > 0 => format!("Push {} commit(s)", st.ahead),
                        _ => "Nothing to push".to_string(),
                    };
                    if ui.add_enabled(ok, btn("Push")).on_disabled_hover_text(tip.clone()).on_hover_text(tip).clicked() {
                        actions.push(Action::Op(Op::Push { set_upstream: false }));
                    }
                }
            }
            if let Some(b) = &self.busy {
                ui.add(egui::Spinner::new().size(11.0));
                ui.label(RichText::new(b).color(th.muted).size(11.0));
            }
        });

        if s.merging {
            Frame::new()
                .fill(th.waiting.gamma_multiply(0.15))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Merge / cherry-pick in progress").color(th.waiting).size(11.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_enabled(!busy, egui::Button::new(RichText::new("Abort").size(11.0)).small()).clicked() {
                                actions.push(Action::Op(Op::AbortMerge));
                            }
                        });
                    });
                });
        }
        ui.add_space(2.0);
    }

    /// Inline op error (dismissable) or a short-lived success notice.
    fn feedback(&mut self, ui: &mut egui::Ui) {
        let th = theme::t();
        if let Some(e) = self.error.clone() {
            let mut dismiss = false;
            Frame::new()
                .fill(th.error.gamma_multiply(0.12))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            if ui.add(egui::Button::new(RichText::new("✕").color(th.muted).size(11.0)).frame(false)).clicked() {
                                dismiss = true;
                            }
                            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                                ui.add(egui::Label::new(RichText::new(e).color(th.error).size(11.0)).wrap());
                            });
                        });
                    });
                });
            if dismiss {
                self.error = None;
            }
        } else if let Some((n, _)) = &self.notice {
            ui.add(egui::Label::new(RichText::new(format!("✓ {n}")).color(th.running).size(11.0)).truncate());
        }
    }

    // ------------------------------------------------------------ commit box

    fn commit_box(&mut self, ui: &mut egui::Ui, s: &Snapshot, actions: &mut Vec<Action>) {
        let th = theme::t();
        let st = &s.status;
        let busy = self.busy.is_some();
        let id = Id::unique("git-commit-msg");
        let ok = !busy && can_commit(&self.commit_msg, st.staged.len(), st.unstaged.len(), self.commit_all);
        // ⌘↵ commits. Consume it before the TextEdit sees it (it would insert a newline).
        let had_focus = ui.memory(|m| m.has_focus(id));
        let cmd_enter = had_focus && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        ui.add(
            egui::TextEdit::multiline(&mut self.commit_msg)
                .id(id)
                .hint_text("Commit message (⌘↵ to commit)")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        let mut commit = cmd_enter && ok;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if st.staged.is_empty() && self.commit_all {
                    "Commit all".to_string()
                } else {
                    format!("Commit ({})", st.staged.len())
                };
                let tip = if self.commit_msg.trim().is_empty() {
                    "Write a commit message"
                } else if st.staged.is_empty() && !self.commit_all {
                    "Nothing staged — stage files or tick \"all\""
                } else {
                    "Commit (⌘↵)"
                };
                let fill = if ok { th.primary } else { th.border };
                let txt = if ok { th.bg } else { th.muted };
                let r = ui.add_enabled(ok, egui::Button::new(RichText::new(label).size(11.0).color(txt)).fill(fill).small());
                if r.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                    commit = true;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add_enabled_ui(!st.unstaged.is_empty() || self.commit_all, |ui| {
                        ui.checkbox(&mut self.commit_all, RichText::new("all").size(11.0))
                            .on_hover_text("Also commit unstaged changes to tracked files (git commit -a)");
                    });
                });
            });
        });
        if commit && ok {
            actions.push(Action::Op(Op::Commit { message: self.commit_msg.clone(), all: self.commit_all }));
        }
    }

    // ------------------------------------------------------------ changes

    fn changes(&mut self, ui: &mut egui::Ui, s: &Snapshot, actions: &mut Vec<Action>) {
        let th = theme::t();
        let st = &s.status;
        let busy = self.busy.is_some();
        if st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty() {
            ui.label(RichText::new("✓ Working tree clean").color(th.muted).size(12.0));
            ui.add_space(6.0);
            return;
        }
        let open_path = self.diff.as_ref().map(|d| d.title.clone());

        // Staged.
        if !st.staged.is_empty() {
            let mut all = false;
            section_header(ui, "Staged", st.staged.len(), None, |ui| {
                all = ui.add_enabled(!busy, small_btn("Unstage all")).clicked();
            });
            if all {
                actions.push(Action::Op(Op::Unstage(st.staged.iter().map(|f| f.path.clone()).collect())));
            }
            for f in &st.staged {
                let btns = [RowBtn::new("−", "Unstage", !busy)];
                let r = file_row(ui, &f.code, &f.path, Some(f), open_path.as_deref() == Some(f.path.as_str()), &btns);
                if r.open {
                    actions.push(Action::Diff(f.path.clone(), None, DiffSrc::File { path: f.path.clone(), staged: true }));
                }
                if r.button == Some(0) {
                    actions.push(Action::Op(Op::Unstage(vec![f.path.clone()])));
                }
            }
            ui.add_space(6.0);
        }

        // Unstaged.
        if !st.unstaged.is_empty() {
            let mut stage_all = false;
            let mut discard_all = false;
            let armed_all = self.armed == Some(Armed::DiscardAll);
            section_header(ui, "Changes", st.unstaged.len(), None, |ui| {
                stage_all = ui.add_enabled(!busy, small_btn("Stage all")).clicked();
                let txt = if armed_all { RichText::new("Confirm discard").color(th.error) } else { RichText::new("Discard all").color(th.muted) };
                discard_all = ui
                    .add_enabled(!busy, egui::Button::new(txt.size(11.0)).small())
                    .on_hover_text("Throw away all unstaged changes to tracked files — can't be undone")
                    .clicked();
            });
            if stage_all {
                actions.push(Action::Op(Op::Stage(st.unstaged.iter().map(|f| f.path.clone()).collect())));
            }
            if discard_all {
                actions.push(if armed_all { Action::Op(Op::DiscardAll) } else { Action::Arm(Some(Armed::DiscardAll)) });
            }
            for f in &st.unstaged {
                let armed = self.armed == Some(Armed::Discard(f.path.clone()));
                let conflicted = f.code == "U";
                let mut btns = vec![RowBtn::new("+", "Stage", !busy)];
                if !conflicted {
                    btns.push(if armed {
                        RowBtn::danger("Discard?", "Click again to discard — can't be undone", !busy)
                    } else {
                        RowBtn::new("↶", "Discard changes", !busy)
                    });
                }
                let r = file_row(ui, &f.code, &f.path, Some(f), open_path.as_deref() == Some(f.path.as_str()), &btns);
                if r.open {
                    actions.push(Action::Diff(f.path.clone(), None, DiffSrc::File { path: f.path.clone(), staged: false }));
                }
                match r.button {
                    Some(0) => actions.push(Action::Op(Op::Stage(vec![f.path.clone()]))),
                    Some(1) if armed => actions.push(Action::Op(Op::Discard { paths: vec![f.path.clone()], untracked: Vec::new() })),
                    Some(1) => actions.push(Action::Arm(Some(Armed::Discard(f.path.clone())))),
                    _ => {}
                }
            }
            ui.add_space(6.0);
        }

        // Untracked.
        if !st.untracked.is_empty() {
            let mut all = false;
            section_header(ui, "Untracked", st.untracked.len(), None, |ui| {
                all = ui.add_enabled(!busy, small_btn("Stage all")).clicked();
            });
            if all {
                actions.push(Action::Op(Op::Stage(st.untracked.clone())));
            }
            for p in &st.untracked {
                let armed = self.armed == Some(Armed::DeleteUntracked(p.clone()));
                let btns = [
                    RowBtn::new("+", "Stage", !busy),
                    if armed {
                        RowBtn::danger("Delete?", "Click again to delete the file — can't be undone", !busy)
                    } else {
                        RowBtn::new("🗑", "Delete file", !busy)
                    },
                ];
                let r = file_row(ui, "?", p, None, open_path.as_deref() == Some(p.as_str()), &btns);
                if r.open {
                    actions.push(Action::Diff(p.clone(), None, DiffSrc::Untracked { path: p.clone() }));
                }
                match r.button {
                    Some(0) => actions.push(Action::Op(Op::Stage(vec![p.clone()]))),
                    Some(1) if armed => actions.push(Action::Op(Op::Discard { paths: Vec::new(), untracked: vec![p.clone()] })),
                    Some(1) => actions.push(Action::Arm(Some(Armed::DeleteUntracked(p.clone())))),
                    _ => {}
                }
            }
            ui.add_space(6.0);
        }
    }

    // ------------------------------------------------------------ log + stashes

    fn commits(&mut self, ui: &mut egui::Ui, s: &Snapshot, actions: &mut Vec<Action>) {
        let th = theme::t();
        let open = self.show_log;
        let mut toggle = false;
        section_header(ui, "Commits", s.log.len(), Some((open, &mut toggle)), |_| {});
        if toggle {
            actions.push(Action::ToggleLog);
        }
        if !open {
            ui.add_space(4.0);
            return;
        }
        if s.log.is_empty() {
            ui.label(RichText::new("No commits yet").color(th.muted).size(11.0));
        }
        for c in &s.log {
            // Reserve a background slot so the hover tint is painted under the text.
            let bg = ui.painter().add(egui::Shape::Noop);
            let r = Frame::new()
                .inner_margin(Margin::symmetric(4, 2))
                .corner_radius(CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(&c.subject).color(th.text).size(12.0)).truncate().selectable(false));
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} · {} · {}", c.short, c.author, c.relative)).color(th.muted).size(10.5),
                        )
                        .truncate()
                        .selectable(false),
                    );
                })
                .response
                .interact(Sense::click());
            if r.hovered() {
                ui.painter().set(bg, egui::epaint::RectShape::filled(r.rect, CornerRadius::same(4), th.border.gamma_multiply(0.35)));
            }
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                actions.push(Action::Diff(format!("{} · {}", c.short, c.subject), Some(c.hash.clone()), DiffSrc::Commit { hash: c.hash.clone() }));
            }
        }
        ui.add_space(6.0);
    }

    fn stashes(&mut self, ui: &mut egui::Ui, s: &Snapshot, actions: &mut Vec<Action>) {
        let th = theme::t();
        let busy = self.busy.is_some();
        let st = &s.status;
        let dirty = !st.staged.is_empty() || !st.unstaged.is_empty() || !st.untracked.is_empty();
        let open = self.show_stash;
        let mut toggle = false;
        let (mut push, mut pop) = (false, false);
        section_header(ui, "Stashes", s.stashes.len(), Some((open, &mut toggle)), |ui| {
            if !s.stashes.is_empty() {
                pop = ui.add_enabled(!busy, small_btn("Pop")).on_hover_text("Apply and drop the latest stash").clicked();
            }
            push = ui
                .add_enabled(!busy && dirty, small_btn("Stash"))
                .on_hover_text("Stash all changes, including untracked files")
                .on_disabled_hover_text("Nothing to stash")
                .clicked();
        });
        if toggle {
            actions.push(Action::ToggleStash);
        }
        if push {
            actions.push(Action::Op(Op::StashPush));
        }
        if pop {
            actions.push(Action::Op(Op::StashPop));
        }
        if !open {
            return;
        }
        for st in &s.stashes {
            let armed = self.armed == Some(Armed::DropStash(st.index));
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let drop = if armed { RichText::new("Drop?").color(th.error) } else { RichText::new("🗑").color(th.muted) };
                    if ui.add_enabled(!busy, egui::Button::new(drop.size(11.0)).frame(false)).on_hover_text("Drop stash").clicked() {
                        actions.push(if armed { Action::Op(Op::StashDrop(st.index)) } else { Action::Arm(Some(Armed::DropStash(st.index))) });
                    }
                    let apply = egui::Button::new(RichText::new("⤓").color(th.muted).size(11.0)).frame(false);
                    if ui.add_enabled(!busy, apply).on_hover_text("Apply (keep the stash)").clicked() {
                        actions.push(Action::Op(Op::StashApply(st.index)));
                    }
                    ui.label(RichText::new(&st.relative).color(th.muted).size(10.5));
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&st.message).color(th.text).size(11.5)).truncate());
                    });
                });
            });
        }
        ui.add_space(6.0);
    }

    // ------------------------------------------------------------ diff view

    fn diff_view(&mut self, ui: &mut egui::Ui, s: &Snapshot, actions: &mut Vec<Action>) {
        let th = theme::t();
        let Some(d) = self.diff.as_ref() else { return };
        let web = origin_web_url(&s.remotes);
        ui.horizontal(|ui| {
            if ui.small_button("← Back").clicked() {
                actions.push(Action::CloseDiff);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(Ok((_, raw))) = &d.body {
                    if ui.add(egui::Button::new(RichText::new("⧉").color(th.muted)).frame(false)).on_hover_text("Copy").clicked() {
                        ui.ctx().copy_text(raw.clone());
                    }
                }
                if let (Some(h), Some(w)) = (&d.hash, &web) {
                    let url = format!("{w}/commit/{h}");
                    if ui.add(egui::Button::new(RichText::new("↗").color(th.muted)).frame(false)).on_hover_text(format!("Open {url}")).clicked() {
                        actions.push(Action::Open(url));
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(&d.title).color(th.text).size(11.5).strong()).truncate())
                        .on_hover_text(&d.title);
                });
            });
        });
        if ui.input(|i| i.key_pressed(Key::Escape) && i.modifiers == Modifiers::NONE) && !ui.memory(|m| m.focused().is_some()) {
            actions.push(Action::CloseDiff);
        }
        match &d.body {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new("Loading…").color(th.muted).size(11.0));
                });
            }
            Some(Err(e)) => {
                ui.add(egui::Label::new(RichText::new(e).color(th.error).size(11.0)).wrap());
            }
            Some(Ok((lines, _))) => {
                let font = FontId::monospace(DIFF_FONT);
                let char_w = ui.painter().layout_no_wrap("M".into(), font.clone(), th.text).size().x;
                let row_h = (DIFF_FONT * 1.35).round();
                let content_w = d.max_chars as f32 * char_w + 12.0;
                Frame::new().fill(th.panel).corner_radius(CornerRadius::same(4)).inner_margin(Margin::same(4)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::both()
                        .id_salt(("git-diff", d.req))
                        .auto_shrink([false, false])
                        .show_rows(ui, row_h, lines.len(), |ui, range| {
                            let w = content_w.max(ui.available_width());
                            for (kind, text) in &lines[range] {
                                let (fg, bg) = match kind {
                                    LineKind::Add => (th.running, Some(th.running.gamma_multiply(0.10))),
                                    LineKind::Del => (th.error, Some(th.error.gamma_multiply(0.10))),
                                    LineKind::Hunk => (th.primary, None),
                                    LineKind::Meta => (th.muted, None),
                                    LineKind::Context => (th.text, None),
                                };
                                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, row_h), Sense::hover());
                                if ui.is_rect_visible(rect) {
                                    let p = ui.painter();
                                    if let Some(bg) = bg {
                                        p.rect_filled(rect, CornerRadius::ZERO, bg);
                                    }
                                    p.text(rect.left_center() + Vec2::new(2.0, 0.0), egui::Align2::LEFT_CENTER, text, font.clone(), fg);
                                }
                            }
                        });
                });
            }
        }
    }
}

/// Branch switcher menu: local branches, then remote ones (checked out as tracking).
fn branch_menu(ui: &mut egui::Ui, s: &Snapshot, remotes: &[&str], actions: &mut Vec<Action>) {
    let th = theme::t();
    let locals: Vec<&GitBranch> = s.branches.iter().filter(|b| !b.remote).collect();
    let remote: Vec<&GitBranch> = s.branches.iter().filter(|b| b.remote && !is_remote_head_alias(&b.name, remotes)).collect();
    ui.label(RichText::new("Local").color(th.muted).size(10.5));
    for b in &locals {
        let r = ui.selectable_label(b.current, RichText::new(&b.name).size(12.0));
        let r = match &b.upstream {
            Some(u) => r.on_hover_text(format!("tracks {u}")),
            None => r,
        };
        if r.clicked() && !b.current {
            actions.push(Action::Op(Op::Checkout(b.name.clone())));
        }
    }
    if !remote.is_empty() {
        ui.separator();
        ui.label(RichText::new("Remote").color(th.muted).size(10.5));
        for b in &remote {
            let target = checkout_target(&b.name, true, remotes);
            let r = ui.selectable_label(false, RichText::new(&b.name).size(12.0)).on_hover_text(format!("Check out as local branch {target}"));
            if r.clicked() {
                actions.push(Action::Op(Op::Checkout(target)));
            }
        }
    }
}

fn small_btn(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(11.0).color(theme::t().muted)).small()
}

/// Section header: optional ▸/▾ toggle, title, count; `right` adds buttons on the right.
fn section_header(ui: &mut egui::Ui, title: &str, count: usize, toggle: Option<(bool, &mut bool)>, right: impl FnOnce(&mut egui::Ui)) {
    let th = theme::t();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            right(ui);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let head = ui
                    .horizontal(|ui| {
                        if let Some((open, _)) = &toggle {
                            ui.label(RichText::new(if *open { "▾" } else { "▸" }).color(th.muted));
                        }
                        ui.label(RichText::new(title).color(th.text).size(12.0).strong());
                        ui.label(RichText::new(count.to_string()).color(th.muted).size(11.0));
                    })
                    .response
                    .interact(Sense::click());
                if let Some((_, clicked)) = toggle {
                    if head.clicked() {
                        *clicked = true;
                    }
                }
            });
        });
    });
}

struct RowBtn {
    text: RichText,
    hover: &'static str,
    enabled: bool,
}

impl RowBtn {
    fn new(text: &str, hover: &'static str, enabled: bool) -> Self {
        RowBtn { text: RichText::new(text).color(theme::t().muted).size(12.0), hover, enabled }
    }
    fn danger(text: &str, hover: &'static str, enabled: bool) -> Self {
        RowBtn { text: RichText::new(text).color(theme::t().error).size(11.0).strong(), hover, enabled }
    }
}

struct RowResp {
    open: bool,
    button: Option<usize>,
}

/// One changed-file row: status letter, file name (+ dim dir), ±counts, buttons.
/// Buttons are laid out first from the right so the name truncates into what's left.
fn file_row(ui: &mut egui::Ui, code: &str, path: &str, change: Option<&GitFileChange>, active: bool, btns: &[RowBtn]) -> RowResp {
    let th = theme::t();
    let mut out = RowResp { open: false, button: None };
    let fill = if active { th.border.gamma_multiply(0.45) } else { Color32::TRANSPARENT };
    Frame::new().fill(fill).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(4, 0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // Rendered right→left, so iterate in reverse to keep the given order.
                for (i, b) in btns.iter().enumerate().rev() {
                    let r = ui.add_enabled(b.enabled, egui::Button::new(b.text.clone()).frame(false)).on_hover_text(b.hover);
                    if r.clicked() {
                        out.button = Some(i);
                    }
                }
                if let Some(c) = change.filter(|c| c.insertions > 0 || c.deletions > 0) {
                    ui.add_space(2.0);
                    if c.deletions > 0 {
                        ui.label(RichText::new(format!("−{}", c.deletions)).color(th.error).size(10.5));
                    }
                    if c.insertions > 0 {
                        ui.label(RichText::new(format!("+{}", c.insertions)).color(th.running).size(10.5));
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(code).color(code_color(code)).size(11.0).monospace().strong());
                    let (name, dir) = split_path(path);
                    let mut job = egui::text::LayoutJob::default();
                    RichText::new(name).color(th.text).size(12.0).append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
                    if !dir.is_empty() {
                        RichText::new(format!("  {dir}")).color(th.muted).size(10.5).append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
                    }
                    let r = ui
                        .add(egui::Label::new(job).truncate().selectable(false).sense(Sense::click()))
                        .on_hover_text(path)
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if r.clicked() {
                        out.open = true;
                    }
                });
            });
        });
    });
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<LineKind> {
        classify_diff(text).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn classify_basic_diff() {
        let d = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n\\ No newline at end of file";
        use LineKind::*;
        assert_eq!(kinds(d), vec![Meta, Meta, Meta, Meta, Hunk, Context, Del, Add, Context]);
    }

    #[test]
    fn classify_deleted_double_dash_line_is_a_deletion() {
        // Deleting a SQL/Lua comment `-- note` renders as `--- note` inside the hunk.
        let d = "--- a/q.sql\n+++ b/q.sql\n@@ -1,2 +1,1 @@\n--- note\n+++ added plus line\n select 1;";
        use LineKind::*;
        assert_eq!(kinds(d), vec![Meta, Meta, Hunk, Del, Add, Context]);
    }

    #[test]
    fn classify_next_file_header_ends_hunk() {
        let d = "@@ -1 +1 @@\n-a\n+b\ndiff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -0,0 +1 @@\n+z";
        use LineKind::*;
        assert_eq!(kinds(d), vec![Hunk, Del, Add, Meta, Meta, Meta, Hunk, Add]);
    }

    #[test]
    fn classify_show_header_and_tabs() {
        let d = "commit abc123\nAuthor: A <a@b>\nDate:   now\n\n    subject\n\n x | 2 +-\n@@ -1 +1 @@\n+\tindented";
        let c = classify_diff(d);
        use LineKind::*;
        let k: Vec<LineKind> = c.iter().map(|(k, _)| *k).collect();
        assert_eq!(k, vec![Meta, Meta, Meta, Context, Context, Context, Context, Hunk, Add]);
        assert_eq!(c[8].1, "+    indented");
    }

    #[test]
    fn untracked_preview_text_binary_and_cap() {
        assert_eq!(untracked_preview(b"a\nb\n"), "@@ new untracked file @@\n+a\n+b\n");
        assert_eq!(untracked_preview(b"PNG\0\x01"), "(binary file)");
        let many = "x\n".repeat(PREVIEW_LINES + 5);
        let p = untracked_preview(many.as_bytes());
        assert!(p.ends_with("… (preview truncated)\n"));
        assert_eq!(p.lines().filter(|l| l.starts_with('+')).count(), PREVIEW_LINES);
        let kinds: Vec<LineKind> = classify_diff(&untracked_preview(b"-- not a header\n")).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, vec![LineKind::Hunk, LineKind::Add]);
    }

    #[test]
    fn unquotes_git_paths() {
        assert_eq!(unquote_path("plain.rs"), "plain.rs");
        assert_eq!(unquote_path("\"sp ace.txt\""), "sp ace.txt");
        assert_eq!(unquote_path("\"caf\\303\\251.md\""), "café.md");
        assert_eq!(unquote_path("\"a\\\"b\\\\c\\td\""), "a\"b\\c\td");
        assert_eq!(unquote_path("\"dir/x y/\""), "dir/x y/");
    }

    #[test]
    fn repos_resolve_to_root_and_dedupe() {
        let r = |p: &str| GitRepo { path: p.into(), name: p.rsplit('/').next().unwrap_or(p).into() };
        // Terminal in /w/sub (inside repo /w) with child repo /w/sub/inner.
        let resolve = |p: &str| match p {
            "/w/sub" => Some("/w".to_string()),
            "/w/sub/inner" => Some("/w/sub/inner".to_string()),
            _ => None,
        };
        let out = root_repos(vec![r("/w/sub"), r("/w/sub/inner"), r("/w/sub")], resolve);
        let got: Vec<(&str, &str)> = out.iter().map(|g| (g.path.as_str(), g.name.as_str())).collect();
        assert_eq!(got, vec![("/w", "w"), ("/w/sub/inner", "inner")]);
    }

    #[test]
    fn pick_repo_prefers_previous_then_cwd_then_first() {
        let list = ["/m/a", "/m", "/m/b"];
        assert_eq!(pick_repo(Some("/m/b"), &list, "/m").as_deref(), Some("/m/b"));
        assert_eq!(pick_repo(Some("/gone"), &list, "/m").as_deref(), Some("/m"));
        assert_eq!(pick_repo(None, &["/m/a", "/m/b"], "/m").as_deref(), Some("/m/a"));
        assert_eq!(pick_repo(Some("/m/a"), &[], "/m"), None);
    }

    #[test]
    fn commit_enablement() {
        assert!(!can_commit("  ", 3, 0, false));
        assert!(can_commit("msg", 1, 0, false));
        assert!(!can_commit("msg", 0, 2, false));
        assert!(can_commit("msg", 0, 2, true));
        // -a skips untracked files: nothing tracked changed → nothing to commit.
        assert!(!can_commit("msg", 0, 0, true));
    }

    #[test]
    fn checkout_target_strips_remote() {
        let remotes = ["origin", "up", "up/stream"];
        assert_eq!(checkout_target("feat/x", false, &remotes), "feat/x");
        assert_eq!(checkout_target("origin/feat/x", true, &remotes), "feat/x");
        assert_eq!(checkout_target("up/stream/main", true, &remotes), "main");
        assert_eq!(checkout_target("up/dev", true, &remotes), "dev");
        assert_eq!(checkout_target("other/dev", true, &remotes), "other/dev");
        assert!(is_remote_head_alias("origin", &remotes));
        assert!(!is_remote_head_alias("origin/main", &remotes));
    }

    #[test]
    fn path_split_and_counts() {
        assert_eq!(split_path("src/app.rs"), ("app.rs", "src/"));
        assert_eq!(split_path("README.md"), ("README.md", ""));
        assert_eq!(split_path("a/b/dir/"), ("dir", "a/b/"));
        assert_eq!(ahead_behind(0, 0), "");
        assert_eq!(ahead_behind(2, 0), "↑2");
        assert_eq!(ahead_behind(1, 3), "↑1 ↓3");
    }

    #[test]
    fn summarize_takes_last_line() {
        assert_eq!(summarize(""), None);
        assert_eq!(summarize("[main abc] msg\n 1 file changed\n\n").as_deref(), Some("1 file changed"));
        let long = "x".repeat(150);
        assert_eq!(summarize(&long).map(|s| s.chars().count()), Some(101));
    }

    #[test]
    fn push_modes() {
        assert_eq!(push_mode(Some("main"), true, true), PushMode::Push);
        assert_eq!(push_mode(Some("main"), true, false), PushMode::Push);
        assert_eq!(push_mode(Some("feat"), false, true), PushMode::Publish);
        assert_eq!(push_mode(Some("feat"), false, false), PushMode::Unavailable);
        assert_eq!(push_mode(Some("HEAD"), false, true), PushMode::Unavailable);
        assert_eq!(push_mode(None, false, true), PushMode::Unavailable);
    }

    #[test]
    fn web_url_prefers_origin() {
        let r = |n: &str, w: Option<&str>| GitRemote { name: n.into(), url: String::new(), web_url: w.map(String::from) };
        assert_eq!(origin_web_url(&[r("up", Some("https://u")), r("origin", Some("https://o"))]).as_deref(), Some("https://o"));
        assert_eq!(origin_web_url(&[r("up", Some("https://u"))]).as_deref(), Some("https://u"));
        assert_eq!(origin_web_url(&[]), None);
    }
}
