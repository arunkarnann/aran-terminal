//! Git inspection for the active session's working directory.
//!
//! Read-only. We shell out to the user's `git` (same approach as the `ps` memory
//! sampler in pty.rs) rather than linking libgit2 — zero added build weight, and
//! `git`'s porcelain formats are stable. All arguments are passed as separate
//! argv entries (never through a shell), so paths/cwd can't inject; commit
//! hashes are additionally validated as hex before use.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepo {
    pub path: String,
    pub name: String,
}

/// Discover git repos to offer in the panel: the session's own directory (if it's
/// a repo, or sits inside one) plus any immediate child directories that are repos
/// — the monorepo case where one folder holds several independent repos.
pub fn repos(cwd: &str) -> Vec<GitRepo> {
    let mut out: Vec<GitRepo> = Vec::new();
    if cwd.is_empty() {
        return out;
    }
    let mut seen = std::collections::HashSet::new();
    let mut add = |p: &Path| {
        if let Some(s) = p.to_str() {
            if seen.insert(s.to_string()) {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(s)
                    .to_string();
                out.push(GitRepo {
                    path: s.to_string(),
                    name,
                });
            }
        }
    };

    let root = Path::new(cwd);
    // The directory itself (a repo, or any dir inside one — git resolves via -C).
    if root.join(".git").exists() || is_repo(cwd) {
        add(root);
    }
    // Immediate children that are their own repos.
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut children: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            if child.is_dir() && child.join(".git").exists() {
                add(&child);
            }
        }
    }
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    pub path: String,
    /// Single-letter git status (M, A, D, R, C, U…).
    pub code: String,
    pub insertions: i64,
    pub deletions: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub staged: Vec<GitFileChange>,
    pub unstaged: Vec<GitFileChange>,
    pub untracked: Vec<String>,
}

impl GitStatus {
    fn not_repo() -> Self {
        GitStatus {
            is_repo: false,
            branch: None,
            ahead: 0,
            behind: 0,
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub relative: String,
    pub timestamp: i64,
}

/// Run `git -C <cwd> <args...>`, returning stdout on success (status 0).
fn run(cwd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run `git -C <cwd> <args...>`, returning stdout on success or stderr on failure.
/// Uses `GIT_TERMINAL_PROMPT=0` to fail fast on auth prompts (no TTY available).
fn run_result(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn is_repo(cwd: &str) -> bool {
    run(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

/// Map path -> (insertions, deletions) from `git diff [--cached] --numstat`.
fn numstat(cwd: &str, cached: bool) -> std::collections::HashMap<String, (i64, i64)> {
    let mut map = std::collections::HashMap::new();
    let mut args = vec!["diff", "--numstat"];
    if cached {
        args.insert(1, "--cached");
    }
    if let Some(out) = run(cwd, &args) {
        for line in out.lines() {
            let mut it = line.splitn(3, '\t');
            let ins = it.next().unwrap_or("0");
            let del = it.next().unwrap_or("0");
            if let Some(path) = it.next() {
                let ins = ins.parse::<i64>().unwrap_or(0); // "-" (binary) -> 0
                let del = del.parse::<i64>().unwrap_or(0);
                map.insert(path.to_string(), (ins, del));
            }
        }
    }
    map
}

pub fn status(cwd: &str) -> GitStatus {
    if cwd.is_empty() || !is_repo(cwd) {
        return GitStatus::not_repo();
    }

    let branch = run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string());

    // Ahead/behind vs upstream (absent if no tracking branch).
    let (mut ahead, mut behind) = (0, 0);
    if let Some(out) = run(cwd, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
        let mut it = out.split_whitespace();
        behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    }

    let staged_counts = numstat(cwd, true);
    let unstaged_counts = numstat(cwd, false);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    if let Some(out) = run(cwd, &["status", "--porcelain=v1"]) {
        for line in out.lines() {
            if line.len() < 3 {
                continue;
            }
            let index = &line[0..1];
            let worktree = &line[1..2];
            let raw = &line[3..];
            // Renames look like "old -> new"; track the new path.
            let path = raw.split(" -> ").last().unwrap_or(raw).to_string();

            if index == "?" && worktree == "?" {
                untracked.push(path);
                continue;
            }
            if index != " " && index != "?" {
                let (i, d) = staged_counts.get(&path).copied().unwrap_or((0, 0));
                staged.push(GitFileChange {
                    path: path.clone(),
                    code: index.to_string(),
                    insertions: i,
                    deletions: d,
                });
            }
            if worktree != " " && worktree != "?" {
                let (i, d) = unstaged_counts.get(&path).copied().unwrap_or((0, 0));
                unstaged.push(GitFileChange {
                    path: path.clone(),
                    code: worktree.to_string(),
                    insertions: i,
                    deletions: d,
                });
            }
        }
    }

    GitStatus {
        is_repo: true,
        branch,
        ahead,
        behind,
        staged,
        unstaged,
        untracked,
    }
}

pub fn log(cwd: &str, limit: usize) -> Vec<GitCommit> {
    if cwd.is_empty() || !is_repo(cwd) {
        return Vec::new();
    }
    // Unit-separator (\x1f) between fields, record per line.
    let fmt = "--pretty=format:%H\x1f%h\x1f%s\x1f%an\x1f%ar\x1f%at";
    let n = format!("-n{}", limit.clamp(1, 500));
    let Some(out) = run(cwd, &["log", &n, fmt]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| {
            let mut f = line.split('\x1f');
            Some(GitCommit {
                hash: f.next()?.to_string(),
                short: f.next()?.to_string(),
                subject: f.next()?.to_string(),
                author: f.next()?.to_string(),
                relative: f.next()?.to_string(),
                timestamp: f.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            })
        })
        .collect()
}

fn valid_hash(hash: &str) -> bool {
    !hash.is_empty()
        && hash.len() <= 40
        && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// Full diff for a single commit (`git show`), capped so a huge commit can't
/// flood the UI.
pub fn show(cwd: &str, hash: &str) -> Result<String, String> {
    if !valid_hash(hash) {
        return Err("invalid commit hash".into());
    }
    run(cwd, &["show", "--no-color", "--stat", "--patch", hash])
        .map(cap_diff)
        .ok_or_else(|| "git show failed".into())
}

/// Diff for a single changed file (staged or working tree).
pub fn diff_file(cwd: &str, path: &str, staged: bool) -> Result<String, String> {
    let mut args = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(path);
    run(cwd, &args)
        .map(cap_diff)
        .ok_or_else(|| "git diff failed".into())
}

// ---- Write actions ----

/// Stage files: `git add -- <paths>`.
pub fn stage(cwd: &str, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["add", "--"];
    let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&refs);
    run_result(cwd, &args).map(|_| ())
}

/// Unstage files: `git reset -q HEAD -- <paths>`.
pub fn unstage(cwd: &str, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["reset", "-q", "HEAD", "--"];
    let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&refs);
    run_result(cwd, &args).map(|_| ())
}

/// Commit staged changes (or all tracked with `all: true`).
pub fn commit(cwd: &str, message: &str, all: bool) -> Result<String, String> {
    let msg = message.trim();
    if msg.is_empty() {
        return Err("Commit message cannot be empty".into());
    }
    let mut args: Vec<&str> = vec!["commit"];
    if all {
        args.push("-a");
    }
    args.push("-m");
    args.push(msg);
    run_result(cwd, &args)
}

/// Fetch all remotes with prune.
pub fn fetch(cwd: &str) -> Result<String, String> {
    run_result(cwd, &["fetch", "--all", "--prune"])
}

/// Fast-forward pull (fails if not possible).
pub fn pull(cwd: &str) -> Result<String, String> {
    run_result(cwd, &["pull", "--ff-only"])
}

/// Push to remote. If `set_upstream` is true, adds `-u origin HEAD`.
pub fn push(cwd: &str, set_upstream: bool) -> Result<String, String> {
    if set_upstream {
        run_result(cwd, &["push", "-u", "origin", "HEAD"])
    } else {
        run_result(cwd, &["push"])
    }
}

/// Keep diffs to a sane size for the panel (~4000 lines).
fn cap_diff(s: String) -> String {
    const MAX_LINES: usize = 4000;
    let mut lines: Vec<&str> = s.lines().take(MAX_LINES).collect();
    if s.lines().count() > MAX_LINES {
        lines.push("… (diff truncated)");
    }
    lines.join("\n")
}

// ---- Branch management ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
    pub remote: bool,
    /// Upstream tracking ref (local branches only), e.g. "origin/main".
    pub upstream: Option<String>,
}

/// Branch names are passed as argv (never through a shell), but reject anything
/// git itself would refuse or that could be read as an option.
fn valid_ref(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains(char::is_whitespace)
        && !name.chars().any(|c| matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

/// List local and remote-tracking branches.
pub fn branches(cwd: &str) -> Vec<GitBranch> {
    if cwd.is_empty() || !is_repo(cwd) {
        return Vec::new();
    }
    let fmt = "--format=%(HEAD)\x1f%(refname:short)\x1f%(upstream:short)";
    let mut out = Vec::new();
    if let Some(s) = run(cwd, &["branch", "--list", fmt]) {
        for line in s.lines() {
            let mut f = line.split('\x1f');
            let head = f.next().unwrap_or("");
            let Some(name) = f.next() else { continue };
            if name.is_empty() || name.starts_with('(') {
                continue; // detached HEAD marker
            }
            let up = f.next().unwrap_or("");
            out.push(GitBranch {
                name: name.to_string(),
                current: head.trim() == "*",
                remote: false,
                upstream: if up.is_empty() { None } else { Some(up.to_string()) },
            });
        }
    }
    if let Some(s) = run(cwd, &["branch", "-r", "--list", "--format=%(refname:short)"]) {
        for name in s.lines().map(str::trim).filter(|n| !n.is_empty()) {
            if name.ends_with("/HEAD") {
                continue;
            }
            out.push(GitBranch {
                name: name.to_string(),
                current: false,
                remote: true,
                upstream: None,
            });
        }
    }
    out
}

/// Check out a branch. For a remote-tracking ref (`origin/foo`) with no local
/// counterpart, create a tracking local branch. With `create`, `git checkout -b`.
pub fn checkout(cwd: &str, branch: &str, create: bool) -> Result<String, String> {
    if !valid_ref(branch) {
        return Err("invalid branch name".into());
    }
    if create {
        return run_result(cwd, &["checkout", "-b", branch]);
    }
    run_result(cwd, &["checkout", branch])
}

/// Create a branch from `source` (defaults to HEAD) and switch to it.
pub fn create_branch(cwd: &str, name: &str, source: Option<&str>) -> Result<String, String> {
    if !valid_ref(name) {
        return Err("invalid branch name".into());
    }
    match source.filter(|s| !s.is_empty()) {
        Some(src) if !valid_ref(src) => Err("invalid source ref".into()),
        Some(src) => run_result(cwd, &["checkout", "-b", name, src]),
        None => run_result(cwd, &["checkout", "-b", name]),
    }
}

/// Delete a local branch (`-D` when force). Refuses the current branch.
pub fn delete_branch(cwd: &str, name: &str, force: bool) -> Result<String, String> {
    if !valid_ref(name) {
        return Err("invalid branch name".into());
    }
    let current = run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if current.trim() == name {
        return Err("Cannot delete the checked-out branch".into());
    }
    run_result(cwd, &["branch", if force { "-D" } else { "-d" }, name])
}

// ---- Remote integration ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRemote {
    pub name: String,
    /// Raw fetch URL as configured.
    pub url: String,
    /// Browsable https URL (GitHub/GitLab/Bitbucket ssh+https forms normalised).
    pub web_url: Option<String>,
}

/// Convert a clone URL into a browsable https URL.
pub fn web_url_for(raw: &str) -> Option<String> {
    let s = raw.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    if let Some(rest) = s.strip_prefix("git@") {
        // git@github.com:user/repo
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{}/{}", host, path.trim_start_matches('/')));
    }
    if let Some(rest) = s.strip_prefix("ssh://") {
        // ssh://git@github.com/user/repo
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next().unwrap_or(host);
        return Some(format!("https://{}/{}", host, path));
    }
    if s.starts_with("https://") || s.starts_with("http://") {
        // Strip embedded credentials: https://user:token@host/…
        if let Some((scheme, rest)) = s.split_once("://") {
            let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
            return Some(format!("{}://{}", scheme, rest));
        }
    }
    None
}

pub fn remotes(cwd: &str) -> Vec<GitRemote> {
    if cwd.is_empty() || !is_repo(cwd) {
        return Vec::new();
    }
    let Some(out) = run(cwd, &["remote", "-v"]) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut res = Vec::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(url)) = (it.next(), it.next()) else { continue };
        if !seen.insert(name.to_string()) {
            continue; // fetch/push pair — keep the first
        }
        res.push(GitRemote {
            name: name.to_string(),
            url: url.to_string(),
            web_url: web_url_for(url),
        });
    }
    res
}

// ---- Stash ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStash {
    pub index: usize,
    pub message: String,
    pub relative: String,
    pub timestamp: i64,
}

pub fn stashes(cwd: &str) -> Vec<GitStash> {
    if cwd.is_empty() || !is_repo(cwd) {
        return Vec::new();
    }
    let Some(out) = run(cwd, &["stash", "list", "--format=%gd\x1f%gs\x1f%cr\x1f%ct"]) else {
        return Vec::new();
    };
    out.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let mut f = line.split('\x1f');
            let _ref = f.next()?;
            let message = f.next()?.to_string();
            Some(GitStash {
                index: i,
                message,
                relative: f.next().unwrap_or("").to_string(),
                timestamp: f.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            })
        })
        .collect()
}

pub fn stash_push(cwd: &str, message: Option<&str>, include_untracked: bool) -> Result<String, String> {
    let mut args: Vec<&str> = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    let msg = message.map(str::trim).filter(|m| !m.is_empty());
    if let Some(m) = msg {
        args.push("-m");
        args.push(m);
    }
    run_result(cwd, &args)
}

fn stash_ref(index: usize) -> String {
    format!("stash@{{{}}}", index)
}

pub fn stash_pop(cwd: &str) -> Result<String, String> {
    run_result(cwd, &["stash", "pop"])
}

pub fn stash_apply(cwd: &str, index: usize) -> Result<String, String> {
    run_result(cwd, &["stash", "apply", &stash_ref(index)])
}

pub fn stash_drop(cwd: &str, index: usize) -> Result<String, String> {
    run_result(cwd, &["stash", "drop", &stash_ref(index)])
}

// ---- Discard ----

/// Discard working-tree changes for the given paths (`git checkout -- <paths>`).
/// Also restores deleted tracked files from HEAD. Untracked paths are removed.
pub fn discard(cwd: &str, paths: &[String], untracked: &[String]) -> Result<(), String> {
    if !paths.is_empty() {
        let mut args: Vec<&str> = vec!["checkout", "--"];
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_result(cwd, &args)?;
    }
    if !untracked.is_empty() {
        let mut args: Vec<&str> = vec!["clean", "-f", "--"];
        let refs: Vec<&str> = untracked.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&refs);
        run_result(cwd, &args)?;
    }
    Ok(())
}

/// Discard all unstaged changes (tracked files only).
pub fn discard_all(cwd: &str) -> Result<(), String> {
    run_result(cwd, &["checkout", "--", "."]).map(|_| ())
}

// ---- Merge / rebase / cherry-pick ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitOpResult {
    pub ok: bool,
    pub output: String,
    /// Paths with unresolved conflicts after the operation.
    pub conflicts: Vec<String>,
    /// A merge / cherry-pick is still in progress (needs resolve + commit, or abort).
    pub in_progress: bool,
}

fn conflicted_paths(cwd: &str) -> Vec<String> {
    run(cwd, &["diff", "--name-only", "--diff-filter=U"])
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

pub fn merge_in_progress(cwd: &str) -> bool {
    let git_dir = run(cwd, &["rev-parse", "--git-dir"]).unwrap_or_default();
    let git_dir = git_dir.trim();
    if git_dir.is_empty() {
        return false;
    }
    let base = if Path::new(git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        Path::new(cwd).join(git_dir)
    };
    base.join("MERGE_HEAD").exists() || base.join("CHERRY_PICK_HEAD").exists()
}

fn op_result(cwd: &str, r: Result<String, String>) -> GitOpResult {
    let conflicts = conflicted_paths(cwd);
    let in_progress = merge_in_progress(cwd);
    match r {
        Ok(o) => GitOpResult { ok: true, output: o, conflicts, in_progress },
        Err(e) => GitOpResult { ok: false, output: e, conflicts, in_progress },
    }
}

pub fn merge(cwd: &str, branch: &str) -> Result<GitOpResult, String> {
    if !valid_ref(branch) {
        return Err("invalid branch name".into());
    }
    Ok(op_result(cwd, run_result(cwd, &["merge", "--no-edit", branch])))
}

pub fn abort_merge(cwd: &str) -> Result<String, String> {
    // Whichever is in progress.
    run_result(cwd, &["merge", "--abort"])
        .or_else(|_| run_result(cwd, &["cherry-pick", "--abort"]))
}

pub fn cherry_pick(cwd: &str, hash: &str) -> Result<GitOpResult, String> {
    if !valid_hash(hash) {
        return Err("invalid commit hash".into());
    }
    Ok(op_result(cwd, run_result(cwd, &["cherry-pick", hash])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_url_normalises_common_forms() {
        assert_eq!(
            web_url_for("git@github.com:arunkarnann/aran-terminal.git").as_deref(),
            Some("https://github.com/arunkarnann/aran-terminal")
        );
        assert_eq!(
            web_url_for("ssh://git@gitlab.com/group/repo.git").as_deref(),
            Some("https://gitlab.com/group/repo")
        );
        assert_eq!(
            web_url_for("https://user:tok@github.com/a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(web_url_for("/local/path"), None);
    }

    #[test]
    fn ref_validation_rejects_option_like_names() {
        assert!(valid_ref("feat/thing-1"));
        assert!(!valid_ref("-D"));
        assert!(!valid_ref("a..b"));
        assert!(!valid_ref("has space"));
        assert!(!valid_ref(""));
    }
}
