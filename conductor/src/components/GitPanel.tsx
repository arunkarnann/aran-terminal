// VS Code / Zed-style git panel: history, changes with stage/unstage, commit, sync.
// Two tabs — History (commit log) and Changes (working-tree status with write actions).

import { useCallback, useEffect, useRef, useState } from "react";
import {
  gitDiff,
  gitFetch,
  gitLog,
  gitPull,
  gitPush,
  gitRepos,
  gitShow,
  gitStage,
  gitStatus,
  gitUnstage,
  gitCommit,
} from "../ipc/api";
import type { GitCommit, GitFileChange, GitRepo, GitStatus } from "../ipc/types";
import { useResizable } from "../lib/useResizable";

interface GitPanelProps {
  cwd: string | null;
  project: string | null;
  onClose: () => void;
}

type Tab = "history" | "changes";
interface DiffView {
  title: string;
  text: string;
}

const REFRESH_MS = 5000;

export function GitPanel({ cwd, project, onClose }: GitPanelProps) {
  const [tab, setTab] = useState<Tab>("history");
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [diff, setDiff] = useState<DiffView | null>(null);
  const [loading, setLoading] = useState(false);
  const [repos, setRepos] = useState<GitRepo[]>([]);
  const [repoPath, setRepoPath] = useState<string | null>(null);
  const [gitWidth, handleProps] = useResizable("right", "conductor-git-width", 300, 220, 480);

  // Commit box state.
  const [commitMsg, setCommitMsg] = useState("");
  const [commitAll, setCommitAll] = useState(false);
  const [committing, setCommitting] = useState(false);

  // Sync state.
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);

  // Discover repos when the active terminal's directory changes.
  useEffect(() => {
    let active = true;
    if (!cwd) {
      setRepos([]);
      setRepoPath(null);
      return;
    }
    gitRepos(cwd)
      .then((list) => {
        if (!active) return;
        setRepos(list);
        setRepoPath((prev) => {
          if (prev && list.some((r) => r.path === prev)) return prev;
          const own = list.find((r) => r.path === cwd);
          return own?.path ?? list[0]?.path ?? null;
        });
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [cwd]);

  const refresh = useCallback(async () => {
    if (!repoPath) {
      setStatus(null);
      setCommits([]);
      return;
    }
    try {
      const [st, log] = await Promise.all([gitStatus(repoPath), gitLog(repoPath, 80)]);
      setStatus(st);
      setCommits(log);
    } catch {
      /* keep last good data */
    }
  }, [repoPath]);

  // Refresh on repo change + on a light interval while open.
  useEffect(() => {
    setDiff(null);
    setLoading(true);
    void refresh().finally(() => setLoading(false));
    const iv = setInterval(refresh, REFRESH_MS);
    return () => clearInterval(iv);
  }, [refresh]);

  const openCommit = useCallback(
    async (c: GitCommit) => {
      if (!repoPath) return;
      setDiff({ title: `${c.short} · ${c.subject}`, text: "Loading…" });
      try {
        const text = await gitShow(repoPath, c.hash);
        setDiff({ title: `${c.short} · ${c.subject}`, text });
      } catch (e) {
        setDiff({ title: c.short, text: String(e) });
      }
    },
    [repoPath],
  );

  const openFile = useCallback(
    async (f: GitFileChange, staged: boolean) => {
      if (!repoPath) return;
      setDiff({ title: f.path, text: "Loading…" });
      try {
        const text = await gitDiff(repoPath, f.path, staged);
        setDiff({ title: f.path, text: text || "(no textual diff)" });
      } catch (e) {
        setDiff({ title: f.path, text: String(e) });
      }
    },
    [repoPath],
  );

  // Stage a single file.
  const doStage = useCallback(
    async (path: string) => {
      if (!repoPath) return;
      try {
        await gitStage(repoPath, [path]);
        await refresh();
      } catch (e) {
        setSyncError(String(e));
      }
    },
    [repoPath, refresh],
  );

  // Unstage a single file.
  const doUnstage = useCallback(
    async (path: string) => {
      if (!repoPath) return;
      try {
        await gitUnstage(repoPath, [path]);
        await refresh();
      } catch (e) {
        setSyncError(String(e));
      }
    },
    [repoPath, refresh],
  );

  // Stage all unstaged + untracked.
  const doStageAll = useCallback(async () => {
    if (!repoPath || !status) return;
    const paths = [
      ...status.unstaged.map((f) => f.path),
      ...status.untracked,
    ];
    if (paths.length === 0) return;
    try {
      await gitStage(repoPath, paths);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    }
  }, [repoPath, status, refresh]);

  // Unstage all staged.
  const doUnstageAll = useCallback(async () => {
    if (!repoPath || !status) return;
    const paths = status.staged.map((f) => f.path);
    if (paths.length === 0) return;
    try {
      await gitUnstage(repoPath, paths);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    }
  }, [repoPath, status, refresh]);

  // Commit.
  const doCommit = useCallback(async () => {
    if (!repoPath || !commitMsg.trim()) return;
    setCommitting(true);
    setSyncError(null);
    try {
      await gitCommit(repoPath, commitMsg, commitAll);
      setCommitMsg("");
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setCommitting(false);
    }
  }, [repoPath, commitMsg, commitAll, refresh]);

  // Fetch.
  const doFetch = useCallback(async () => {
    if (!repoPath) return;
    setSyncing(true);
    setSyncError(null);
    try {
      await gitFetch(repoPath);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setSyncing(false);
    }
  }, [repoPath, refresh]);

  // Pull.
  const doPull = useCallback(async () => {
    if (!repoPath) return;
    setSyncing(true);
    setSyncError(null);
    try {
      await gitPull(repoPath);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setSyncing(false);
    }
  }, [repoPath, refresh]);

  // Push (or publish if no upstream).
  const doPush = useCallback(
    async (setUpstream: boolean) => {
      if (!repoPath) return;
      setSyncing(true);
      setSyncError(null);
      try {
        await gitPush(repoPath, setUpstream);
        await refresh();
      } catch (e) {
        setSyncError(String(e));
      } finally {
        setSyncing(false);
      }
    },
    [repoPath, refresh],
  );

  const selectedRepo = repos.find((r) => r.path === repoPath) ?? null;
  const branch = status?.branch ?? "—";
  const dirty =
    (status?.staged.length ?? 0) +
      (status?.unstaged.length ?? 0) +
      (status?.untracked.length ?? 0) >
    0;
  const hasStaged = (status?.staged.length ?? 0) > 0;
  const noUpstream = status?.isRepo && status.branch && status.ahead === 0 && status.behind === 0;

  return (
    <aside className="git-panel" style={{ width: gitWidth }}>
      <div className="resize-handle" {...handleProps} />
      <div className="git-head">
        <div className="git-title">
          <span className="git-glyph">⎇</span>
          {repos.length > 1 ? (
            <select
              className="git-repo-select"
              value={repoPath ?? ""}
              onChange={(e) => setRepoPath(e.target.value)}
              title="Choose repository"
            >
              {repos.map((r) => (
                <option key={r.path} value={r.path}>
                  {r.name}
                </option>
              ))}
            </select>
          ) : (
            <span className="git-repo">{selectedRepo?.name ?? project ?? "Git"}</span>
          )}
          {status?.isRepo && (
            <span className="git-branch">
              {branch}
              {dirty && <span className="git-dirty">●</span>}
              {status.ahead > 0 && <span className="git-ab">↑{status.ahead}</span>}
              {status.behind > 0 && <span className="git-ab">↓{status.behind}</span>}
            </span>
          )}
        </div>
        <button className="git-x" onClick={onClose} title="Close">
          ✕
        </button>
      </div>

      {/* Sync bar */}
      {status?.isRepo && (
        <div className="git-sync-bar">
          <button
            className="git-sync-btn"
            onClick={doFetch}
            disabled={syncing}
            title="Fetch all remotes"
          >
            Fetch
          </button>
          <button
            className="git-sync-btn"
            onClick={doPull}
            disabled={syncing || status.behind === 0}
            title={status.behind > 0 ? `Pull ${status.behind} commit(s)` : "Nothing to pull"}
          >
            Pull
            {status.behind > 0 && <span className="git-sync-badge">↓{status.behind}</span>}
          </button>
          {noUpstream ? (
            <button
              className="git-sync-btn git-sync-btn--primary"
              onClick={() => doPush(true)}
              disabled={syncing}
              title="Publish branch to remote"
            >
              Publish
            </button>
          ) : (
            <button
              className="git-sync-btn"
              onClick={() => doPush(false)}
              disabled={syncing || status.ahead === 0}
              title={status.ahead > 0 ? `Push ${status.ahead} commit(s)` : "Nothing to push"}
            >
              Push
              {status.ahead > 0 && <span className="git-sync-badge">↑{status.ahead}</span>}
            </button>
          )}
          {syncing && <span className="git-spinner" />}
        </div>
      )}
      {syncError && <div className="git-status-line git-status-line--error">{syncError}</div>}

      <div className="git-tabs">
        <button
          className={`git-tab ${tab === "history" ? "git-tab--on" : ""}`}
          onClick={() => setTab("history")}
        >
          History
        </button>
        <button
          className={`git-tab ${tab === "changes" ? "git-tab--on" : ""}`}
          onClick={() => setTab("changes")}
        >
          Changes
          {dirty && (
            <span className="git-count">
              {(status?.staged.length ?? 0) +
                (status?.unstaged.length ?? 0) +
                (status?.untracked.length ?? 0)}
            </span>
          )}
        </button>
      </div>

      {!cwd ? (
        <div className="git-empty">No active terminal.</div>
      ) : !repoPath || (status && !status.isRepo) ? (
        <div className="git-empty">
          No git repository here.
          <div className="git-empty-path">{cwd}</div>
        </div>
      ) : diff ? (
        <DiffPane diff={diff} onBack={() => setDiff(null)} />
      ) : tab === "history" ? (
        <div className="git-scroll">
          {commits.length === 0 ? (
            <div className="git-empty">{loading ? "Loading…" : "No commits."}</div>
          ) : (
            commits.map((c) => (
              <button key={c.hash} className="git-commit" onClick={() => openCommit(c)}>
                <span className="git-node" />
                <span className="git-commit-body">
                  <span className="git-subject">{c.subject || "(no message)"}</span>
                  <span className="git-meta">
                    <span className="git-hash">{c.short}</span>
                    {c.author} · {c.relative}
                  </span>
                </span>
              </button>
            ))
          )}
        </div>
      ) : (
        <div className="git-scroll" ref={scrollRef}>
          {/* Staged changes */}
          <ChangeGroup
            label="Staged"
            files={status?.staged ?? []}
            onPick={(f) => openFile(f, true)}
            actionLabel="−"
            actionTitle="Unstage"
            onAction={(f) => doUnstage(f.path)}
            groupAction={doUnstageAll}
            groupActionLabel="Unstage all"
          />

          {/* Unstaged changes */}
          <ChangeGroup
            label="Unstaged"
            files={status?.unstaged ?? []}
            onPick={(f) => openFile(f, false)}
            actionLabel="+"
            actionTitle="Stage"
            onAction={(f) => doStage(f.path)}
            groupAction={doStageAll}
            groupActionLabel="Stage all"
          />

          {/* Untracked files */}
          {(status?.untracked.length ?? 0) > 0 && (
            <section className="git-group">
              <div className="git-group-label">
                Untracked
                <button
                  className="git-group-action"
                  onClick={doStageAll}
                  title="Stage all untracked"
                >
                  Stage all
                </button>
              </div>
              {status?.untracked.map((p) => (
                <div key={p} className="git-file git-file--untracked">
                  <span className="git-code git-code--untracked">?</span>
                  <span className="git-path">{p}</span>
                  <button
                    className="git-file-action"
                    onClick={() => doStage(p)}
                    title="Stage"
                  >
                    +
                  </button>
                </div>
              ))}
            </section>
          )}

          {!dirty && <div className="git-empty">Working tree clean.</div>}

          {/* Commit box */}
          {status?.isRepo && (
            <div className="git-commit-box">
              <textarea
                className="git-commit-input"
                placeholder="Commit message…"
                value={commitMsg}
                onChange={(e) => setCommitMsg(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    doCommit();
                  }
                }}
                rows={3}
              />
              <div className="git-commit-actions">
                <label className="git-commit-all-label">
                  <input
                    type="checkbox"
                    checked={commitAll}
                    onChange={(e) => setCommitAll(e.target.checked)}
                  />
                  Commit all
                </label>
                <button
                  className="git-commit-btn"
                  onClick={doCommit}
                  disabled={committing || !commitMsg.trim() || (!hasStaged && !commitAll)}
                  title={
                    !hasStaged && !commitAll
                      ? "Nothing staged"
                      : commitMsg.trim()
                        ? "Commit (⌘Enter)"
                        : "Enter a message"
                  }
                >
                  {committing ? "Committing…" : "Commit"}
                </button>
              </div>
            </div>
          )}
        </div>
      )}
    </aside>
  );
}

function ChangeGroup({
  label,
  files,
  onPick,
  actionLabel,
  actionTitle,
  onAction,
  groupAction,
  groupActionLabel,
}: {
  label: string;
  files: GitFileChange[];
  onPick: (f: GitFileChange) => void;
  actionLabel: string;
  actionTitle: string;
  onAction: (f: GitFileChange) => void;
  groupAction?: () => void;
  groupActionLabel?: string;
}) {
  if (files.length === 0) return null;
  return (
    <section className="git-group">
      <div className="git-group-label">
        {label}
        {groupAction && files.length > 0 && (
          <button className="git-group-action" onClick={groupAction} title={groupActionLabel}>
            {groupActionLabel}
          </button>
        )}
      </div>
      {files.map((f) => (
        <div key={`${label}:${f.path}`} className="git-file">
          <button className="git-file-pick" onClick={() => onPick(f)}>
            <span className={`git-code git-code--${f.code.toLowerCase()}`}>{f.code}</span>
            <span className="git-path">{f.path}</span>
            <span className="git-numstat">
              {f.insertions > 0 && <span className="git-add">+{f.insertions}</span>}
              {f.deletions > 0 && <span className="git-del">-{f.deletions}</span>}
            </span>
          </button>
          <button
            className="git-file-action"
            onClick={(e) => {
              e.stopPropagation();
              onAction(f);
            }}
            title={actionTitle}
          >
            {actionLabel}
          </button>
        </div>
      ))}
    </section>
  );
}

function DiffPane({ diff, onBack }: { diff: DiffView; onBack: () => void }) {
  return (
    <div className="git-diff">
      <button className="git-back" onClick={onBack}>
        ‹ back
      </button>
      <div className="git-diff-title" title={diff.title}>
        {diff.title}
      </div>
      <pre className="git-diff-body">
        {diff.text.split("\n").map((line, i) => (
          <div key={i} className={`gd ${diffClass(line)}`}>
            {line || " "}
          </div>
        ))}
      </pre>
    </div>
  );
}

function diffClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "gd-meta";
  if (line.startsWith("@@")) return "gd-hunk";
  if (line.startsWith("diff ") || line.startsWith("index ")) return "gd-meta";
  if (line.startsWith("+")) return "gd-add";
  if (line.startsWith("-")) return "gd-del";
  return "";
}
