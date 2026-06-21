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

/// Keep diffs to a sane size for the panel (~4000 lines).
fn cap_diff(s: String) -> String {
    const MAX_LINES: usize = 4000;
    let mut lines: Vec<&str> = s.lines().take(MAX_LINES).collect();
    if s.lines().count() > MAX_LINES {
        lines.push("… (diff truncated)");
    }
    lines.join("\n")
}
