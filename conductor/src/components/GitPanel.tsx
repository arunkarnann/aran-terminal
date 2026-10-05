// VS Code / Zed-style git panel: history, changes with stage/unstage, commit, sync,
// plus branch switching, remote links, stash, discard, merge and cherry-pick (#6).
// Two tabs — History (commit log) and Changes (working-tree status with write actions).

import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  gitAbortMerge,
  gitBranches,
  gitCheckout,
  gitCherryPick,
  gitCommit,
  gitCreateBranch,
  gitDeleteBranch,
  gitDiff,
  gitDiscard,
  gitDiscardAll,
  gitFetch,
  gitLog,
  gitMerge,
  gitMergeInProgress,
  gitPull,
  gitPush,
  gitRemotes,
  gitRepos,
  gitShow,
  gitStage,
  gitStashApply,
  gitStashDrop,
  gitStashes,
  gitStashPop,
  gitStashPush,
  gitStatus,
  gitUnstage,
} from "../ipc/api";
import type {
  GitBranch,
  GitCommit,
  GitFileChange,
  GitOpResult,
  GitRemote,
  GitRepo,
  GitStash,
  GitStatus,
} from "../ipc/types";
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
const NOTICE_MS = 3000;

export function GitPanel({ cwd, project, onClose }: GitPanelProps) {
  const [tab, setTab] = useState<Tab>("history");
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [branches, setBranches] = useState<GitBranch[]>([]);
  const [remotes, setRemotes] = useState<GitRemote[]>([]);
  const [stashes, setStashes] = useState<GitStash[]>([]);
  const [inProgress, setInProgress] = useState(false);
  const [diff, setDiff] = useState<DiffView | null>(null);
  const [loading, setLoading] = useState(false);
  const [repos, setRepos] = useState<GitRepo[]>([]);
  const [repoPath, setRepoPath] = useState<string | null>(null);
  const [gitWidth, handleProps] = useResizable("right", "conductor-git-width", 300, 220, 480);

  // Commit box state.
  const [commitMsg, setCommitMsg] = useState("");
  const [commitAll, setCommitAll] = useState(false);
  const [committing, setCommitting] = useState(false);

  // Sync / operation state.
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [opResult, setOpResult] = useState<GitOpResult | null>(null);

  // Inline editors.
  const [newBranch, setNewBranch] = useState<string | null>(null); // null = closed
  const [mergePick, setMergePick] = useState<string | null>(null); // null = closed
  const [stashMsg, setStashMsg] = useState<string | null>(null); // null = closed
  const [stashUntracked, setStashUntracked] = useState(true);
  const [confirmDiscardAll, setConfirmDiscardAll] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);

  // Success feedback that fades on its own.
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(t);
  }, [notice]);

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
      setBranches([]);
      setRemotes([]);
      setStashes([]);
      setInProgress(false);
      return;
    }
    try {
      const [st, log, br, rm, stl, prog] = await Promise.all([
        gitStatus(repoPath),
        gitLog(repoPath, 80),
        gitBranches(repoPath),
        gitRemotes(repoPath),
        gitStashes(repoPath),
        gitMergeInProgress(repoPath),
      ]);
      setStatus(st);
      setCommits(log);
      setBranches(br);
      setRemotes(rm);
      setStashes(stl);
      setInProgress(prog);
    } catch {
      /* keep last good data */
    }
  }, [repoPath]);

  // Refresh on repo change + on a light interval while open.
  useEffect(() => {
    setDiff(null);
    setOpResult(null);
    setLoading(true);
    void refresh().finally(() => setLoading(false));
    const iv = setInterval(refresh, REFRESH_MS);
    return () => clearInterval(iv);
  }, [refresh]);

  /** Run a write op with busy/error/success feedback, then refresh. */
  const runOp = useCallback(
    async (label: string, fn: () => Promise<unknown>) => {
      if (!repoPath) return false;
      setSyncing(true);
      setSyncError(null);
      try {
        await fn();
        setNotice(label);
        await refresh();
        return true;
      } catch (e) {
        setSyncError(String(e));
        return false;
      } finally {
        setSyncing(false);
      }
    },
    [repoPath, refresh],
  );

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

  // ---- Stage / unstage ----
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

  const doStageAll = useCallback(async () => {
    if (!repoPath || !status) return;
    const paths = [...status.unstaged.map((f) => f.path), ...status.untracked];
    if (paths.length === 0) return;
    try {
      await gitStage(repoPath, paths);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    }
  }, [repoPath, status, refresh]);

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

  // ---- Discard ----
  const doDiscard = useCallback(
    (path: string) => runOp(`Discarded ${path}`, () => gitDiscard(repoPath!, [path])),
    [repoPath, runOp],
  );
  const doDeleteUntracked = useCallback(
    (path: string) => runOp(`Deleted ${path}`, () => gitDiscard(repoPath!, [], [path])),
    [repoPath, runOp],
  );
  const doDiscardAll = useCallback(async () => {
    setConfirmDiscardAll(false);
    await runOp("Discarded all unstaged changes", () => gitDiscardAll(repoPath!));
  }, [repoPath, runOp]);

  // ---- Commit ----
  const doCommit = useCallback(async () => {
    if (!repoPath || !commitMsg.trim()) return;
    setCommitting(true);
    setSyncError(null);
    try {
      await gitCommit(repoPath, commitMsg, commitAll);
      setCommitMsg("");
      setNotice("Committed");
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setCommitting(false);
    }
  }, [repoPath, commitMsg, commitAll, refresh]);

  // ---- Sync ----
  const doFetch = useCallback(() => runOp("Fetched", () => gitFetch(repoPath!)), [repoPath, runOp]);
  const doPull = useCallback(() => runOp("Pulled", () => gitPull(repoPath!)), [repoPath, runOp]);
  const doPush = useCallback(
    (setUpstream: boolean) =>
      runOp(setUpstream ? "Published" : "Pushed", () => gitPush(repoPath!, setUpstream)),
    [repoPath, runOp],
  );

  // ---- Branches ----
  const doCheckout = useCallback(
    async (name: string) => {
      const target = branches.find((b) => b.name === name);
      if (!target || target.current) return;
      // Remote-tracking ref: checking out "origin/foo" creates/tracks local "foo".
      const local = target.remote ? name.split("/").slice(1).join("/") : name;
      const exists = branches.some((b) => !b.remote && b.name === local);
      await runOp(`Switched to ${local}`, () =>
        target.remote && !exists
          ? gitCheckout(repoPath!, local, true).catch(() => gitCheckout(repoPath!, name))
          : gitCheckout(repoPath!, local),
      );
    },
    [branches, repoPath, runOp],
  );

  const doCreateBranch = useCallback(async () => {
    const name = newBranch?.trim();
    if (!name) return;
    const ok = await runOp(`Created ${name}`, () => gitCreateBranch(repoPath!, name));
    if (ok) setNewBranch(null);
  }, [newBranch, repoPath, runOp]);

  const doDeleteBranch = useCallback(async () => {
    const cur = branches.find((b) => b.current);
    const others = branches.filter((b) => !b.remote && !b.current);
    if (!cur || others.length === 0) return;
    // Delete the current branch: switch to the first other local branch first.
    const fallback = others.find((b) => b.name === "main" || b.name === "master") ?? others[0];
    if (!confirm(`Delete branch "${cur.name}"? You'll be switched to "${fallback.name}".`)) return;
    await runOp(`Deleted ${cur.name}`, async () => {
      await gitCheckout(repoPath!, fallback.name);
      try {
        await gitDeleteBranch(repoPath!, cur.name, false);
      } catch (e) {
        if (!confirm(`${e}\n\nForce delete "${cur.name}"? Unmerged commits will be lost.`)) throw e;
        await gitDeleteBranch(repoPath!, cur.name, true);
      }
    });
  }, [branches, repoPath, runOp]);

  // ---- Remote ----
  const origin = remotes.find((r) => r.name === "origin") ?? remotes[0] ?? null;
  const openOnWeb = useCallback(() => {
    if (origin?.webUrl) void openUrl(origin.webUrl).catch((e) => setSyncError(String(e)));
  }, [origin]);
  const openCommitOnWeb = useCallback(
    (hash: string) => {
      if (origin?.webUrl) void openUrl(`${origin.webUrl}/commit/${hash}`).catch(() => {});
    },
    [origin],
  );
  const copyRemoteUrl = useCallback(() => {
    if (!origin) return;
    void navigator.clipboard?.writeText(origin.url).then(
      () => setNotice("Remote URL copied"),
      (e) => setSyncError(String(e)),
    );
  }, [origin]);

  // ---- Stash ----
  const doStashPush = useCallback(async () => {
    const ok = await runOp("Stashed", () =>
      gitStashPush(repoPath!, stashMsg ?? undefined, stashUntracked),
    );
    if (ok) setStashMsg(null);
  }, [repoPath, runOp, stashMsg, stashUntracked]);
  const doStashPop = useCallback(() => runOp("Stash popped", () => gitStashPop(repoPath!)), [repoPath, runOp]);
  const doStashApply = useCallback(
    (i: number) => runOp("Stash applied", () => gitStashApply(repoPath!, i)),
    [repoPath, runOp],
  );
  const doStashDrop = useCallback(
    (i: number) => runOp("Stash dropped", () => gitStashDrop(repoPath!, i)),
    [repoPath, runOp],
  );

  // ---- Merge / cherry-pick ----
  const applyOpResult = useCallback(
    (r: GitOpResult, label: string) => {
      setOpResult(r.ok && r.conflicts.length === 0 ? null : r);
      if (r.ok && r.conflicts.length === 0) setNotice(label);
      else if (r.conflicts.length > 0) setSyncError(`${r.conflicts.length} conflict(s) — resolve or abort`);
      else setSyncError(r.output || `${label} failed`);
    },
    [],
  );

  const doMerge = useCallback(async () => {
    const branch = mergePick;
    if (!branch || !repoPath) return;
    setSyncing(true);
    setSyncError(null);
    try {
      const r = await gitMerge(repoPath, branch);
      applyOpResult(r, `Merged ${branch}`);
      setMergePick(null);
      await refresh();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setSyncing(false);
    }
  }, [mergePick, repoPath, applyOpResult, refresh]);

  const doAbort = useCallback(async () => {
    const ok = await runOp("Aborted", () => gitAbortMerge(repoPath!));
    if (ok) setOpResult(null);
  }, [repoPath, runOp]);

  const doCherryPick = useCallback(
    async (c: GitCommit) => {
      if (!repoPath) return;
      setSyncing(true);
      setSyncError(null);
      try {
        const r = await gitCherryPick(repoPath, c.hash);
        applyOpResult(r, `Cherry-picked ${c.short}`);
        await refresh();
      } catch (e) {
        setSyncError(String(e));
      } finally {
        setSyncing(false);
      }
    },
    [repoPath, applyOpResult, refresh],
  );

  const selectedRepo = repos.find((r) => r.path === repoPath) ?? null;
  const branch = status?.branch ?? "—";
  const localBranches = branches.filter((b) => !b.remote);
  const remoteBranches = branches.filter((b) => b.remote);
  const dirty =
    (status?.staged.length ?? 0) +
      (status?.unstaged.length ?? 0) +
      (status?.untracked.length ?? 0) >
    0;
  const hasStaged = (status?.staged.length ?? 0) > 0;
  const currentBranch = branches.find((b) => b.current) ?? null;
  const noUpstream = status?.isRepo && status.branch && currentBranch && !currentBranch.upstream;

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
              {dirty && <span className="git-dirty">●</span>}
              {status.ahead > 0 && <span className="git-ab">↑{status.ahead}</span>}
              {status.behind > 0 && <span className="git-ab">↓{status.behind}</span>}
            </span>
          )}
        </div>
        {origin && (
          <>
            <button
              className="git-x git-head-btn"
              onClick={openOnWeb}
              disabled={!origin.webUrl}
              title={origin.webUrl ? `Open ${origin.webUrl}` : origin.url}
            >
              ↗
            </button>
            <button className="git-x git-head-btn" onClick={copyRemoteUrl} title="Copy remote URL">
              ⧉
            </button>
          </>
        )}
        <button className="git-x" onClick={onClose} title="Close">
          ✕
        </button>
      </div>

      {/* Branch bar */}
      {status?.isRepo && (
        <div className="git-branch-bar">
          {newBranch === null ? (
            <>
              <select
                className="git-branch-select"
                value={currentBranch?.name ?? branch}
                onChange={(e) => void doCheckout(e.target.value)}
                disabled={syncing || inProgress}
                title="Switch branch"
              >
                {!currentBranch && <option value={branch}>{branch}</option>}
                {localBranches.length > 0 && (
                  <optgroup label="Local">
                    {localBranches.map((b) => (
                      <option key={b.name} value={b.name}>
                        {b.name}
                      </option>
                    ))}
                  </optgroup>
                )}
                {remoteBranches.length > 0 && (
                  <optgroup label="Remote">
                    {remoteBranches.map((b) => (
                      <option key={b.name} value={b.name}>
                        {b.name}
                      </option>
                    ))}
                  </optgroup>
                )}
              </select>
              <button
                className="git-sync-btn"
                onClick={() => setNewBranch("")}
                disabled={syncing || inProgress}
                title="Create a new branch from HEAD"
              >
                +
              </button>
              <button
                className="git-sync-btn"
                onClick={doDeleteBranch}
                disabled={syncing || inProgress || localBranches.length < 2}
                title="Delete current branch"
              >
                🗑
              </button>
            </>
          ) : (
            <>
              <input
                className="git-inline-input"
                autoFocus
                placeholder="new-branch-name"
                value={newBranch}
                onChange={(e) => setNewBranch(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void doCreateBranch();
                  if (e.key === "Escape") setNewBranch(null);
                }}
              />
              <button
                className="git-sync-btn git-sync-btn--primary"
                onClick={doCreateBranch}
                disabled={syncing || !newBranch.trim()}
              >
                Create
              </button>
              <button className="git-sync-btn" onClick={() => setNewBranch(null)}>
                ✕
              </button>
            </>
          )}
        </div>
      )}

      {/* Sync bar */}
      {status?.isRepo && (
        <div className="git-sync-bar">
          {inProgress ? (
            <>
              <span className="git-inprogress">
                {opResult?.conflicts.length ? `${opResult.conflicts.length} conflict(s)` : "Merge in progress"}
              </span>
              <button className="git-sync-btn git-sync-btn--danger" onClick={doAbort} disabled={syncing}>
                Abort
              </button>
            </>
          ) : mergePick !== null ? (
            <>
              <select
                className="git-branch-select"
                value={mergePick}
                onChange={(e) => setMergePick(e.target.value)}
                autoFocus
              >
                <option value="">Merge into {branch}…</option>
                {branches
                  .filter((b) => !b.current)
                  .map((b) => (
                    <option key={b.name} value={b.name}>
                      {b.name}
                    </option>
                  ))}
              </select>
              <button
                className="git-sync-btn git-sync-btn--primary"
                onClick={doMerge}
                disabled={syncing || !mergePick}
              >
                Merge
              </button>
              <button className="git-sync-btn" onClick={() => setMergePick(null)}>
                ✕
              </button>
            </>
          ) : stashMsg !== null ? (
            <>
              <input
                className="git-inline-input"
                autoFocus
                placeholder="Stash message (optional)"
                value={stashMsg}
                onChange={(e) => setStashMsg(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void doStashPush();
                  if (e.key === "Escape") setStashMsg(null);
                }}
              />
              <label className="git-commit-all-label" title="Include untracked files (git stash -u)">
                <input
                  type="checkbox"
                  checked={stashUntracked}
                  onChange={(e) => setStashUntracked(e.target.checked)}
                />
                -u
              </label>
              <button
                className="git-sync-btn git-sync-btn--primary"
                onClick={doStashPush}
                disabled={syncing}
              >
                Stash
              </button>
              <button className="git-sync-btn" onClick={() => setStashMsg(null)}>
                ✕
              </button>
            </>
          ) : (
            <>
              <button className="git-sync-btn" onClick={doFetch} disabled={syncing} title="Fetch all remotes">
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
              <button
                className="git-sync-btn"
                onClick={() => setMergePick("")}
                disabled={syncing || branches.length < 2}
                title="Merge a branch into the current one"
              >
                Merge
              </button>
              <button
                className="git-sync-btn"
                onClick={() => setStashMsg("")}
                disabled={syncing || !dirty}
                title={dirty ? "Stash working-tree changes" : "Nothing to stash"}
              >
                Stash
              </button>
              {syncing && <span className="git-spinner" />}
            </>
          )}
        </div>
      )}
      {syncError && <div className="git-status-line git-status-line--error">{syncError}</div>}
      {notice && !syncError && <div className="git-status-line git-status-line--ok">{notice}</div>}
      {opResult && opResult.conflicts.length > 0 && (
        <div className="git-status-line git-status-line--error">
          {opResult.conflicts.map((p) => (
            <div key={p} className="git-conflict-path">
              ⚠ {p}
            </div>
          ))}
        </div>
      )}

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
          {stashes.length > 0 && (
            <span className="git-count git-count--stash" title={`${stashes.length} stash(es)`}>
              ⧗{stashes.length}
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
            commits.map((c, i) => (
              <div key={c.hash} className="git-commit-row">
                <button className="git-commit" onClick={() => openCommit(c)}>
                  <span className="git-node" />
                  <span className="git-commit-body">
                    <span className="git-subject">{c.subject || "(no message)"}</span>
                    <span className="git-meta">
                      <span className="git-hash">{c.short}</span>
                      {c.author} · {c.relative}
                    </span>
                  </span>
                </button>
                <span className="git-commit-actions-hover">
                  {origin?.webUrl && (
                    <button
                      className="git-file-action"
                      onClick={() => openCommitOnWeb(c.hash)}
                      title="Open commit on remote"
                    >
                      ↗
                    </button>
                  )}
                  {i > 0 && (
                    <button
                      className="git-file-action"
                      onClick={() => void doCherryPick(c)}
                      disabled={syncing || inProgress}
                      title={`Cherry-pick ${c.short} onto ${branch}`}
                    >
                      🍒
                    </button>
                  )}
                </span>
              </div>
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
            secondaryLabel="↶"
            secondaryTitle="Discard changes"
            onSecondary={(f) => {
              if (confirm(`Discard changes to ${f.path}? This can't be undone.`)) void doDiscard(f.path);
            }}
            groupAction={doStageAll}
            groupActionLabel="Stage all"
            groupSecondary={() => setConfirmDiscardAll(true)}
            groupSecondaryLabel="Discard all"
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
                    onClick={() => {
                      if (confirm(`Delete untracked file ${p}?`)) void doDeleteUntracked(p);
                    }}
                    title="Delete file"
                  >
                    🗑
                  </button>
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

          {/* Stashes */}
          {stashes.length > 0 && (
            <section className="git-group">
              <div className="git-group-label">
                Stashes
                <button
                  className="git-group-action"
                  onClick={doStashPop}
                  disabled={syncing}
                  title="Apply and drop the latest stash"
                >
                  Pop latest
                </button>
              </div>
              {stashes.map((s) => (
                <div key={s.index} className="git-file git-stash">
                  <span className="git-code">{s.index}</span>
                  <span className="git-path" title={s.message}>
                    {s.message}
                    <span className="git-meta"> · {s.relative}</span>
                  </span>
                  <button
                    className="git-file-action"
                    onClick={() => void doStashApply(s.index)}
                    disabled={syncing}
                    title="Apply (keep stash)"
                  >
                    ⤓
                  </button>
                  <button
                    className="git-file-action"
                    onClick={() => {
                      if (confirm(`Drop stash "${s.message}"?`)) void doStashDrop(s.index);
                    }}
                    disabled={syncing}
                    title="Drop"
                  >
                    🗑
                  </button>
                </div>
              ))}
            </section>
          )}

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

      {confirmDiscardAll && (
        <div className="dialog-overlay" onClick={() => setConfirmDiscardAll(false)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h2 className="dialog-title">Discard all changes?</h2>
            <p className="dialog-body">
              This reverts every unstaged change to {status?.unstaged.length ?? 0} tracked file
              {(status?.unstaged.length ?? 0) === 1 ? "" : "s"}. Untracked files are kept. This
              can't be undone.
            </p>
            <div className="dialog-actions">
              <button className="btn btn-danger" onClick={doDiscardAll}>
                Discard all
              </button>
              <button className="btn" onClick={() => setConfirmDiscardAll(false)}>
                Cancel
              </button>
            </div>
          </div>
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
  secondaryLabel,
  secondaryTitle,
  onSecondary,
  groupSecondary,
  groupSecondaryLabel,
}: {
  label: string;
  files: GitFileChange[];
  onPick: (f: GitFileChange) => void;
  actionLabel: string;
  actionTitle: string;
  onAction: (f: GitFileChange) => void;
  groupAction?: () => void;
  groupActionLabel?: string;
  secondaryLabel?: string;
  secondaryTitle?: string;
  onSecondary?: (f: GitFileChange) => void;
  groupSecondary?: () => void;
  groupSecondaryLabel?: string;
}) {
  if (files.length === 0) return null;
  return (
    <section className="git-group">
      <div className="git-group-label">
        {label}
        <span className="git-group-actions">
          {groupSecondary && files.length > 0 && (
            <button
              className="git-group-action git-group-action--danger"
              onClick={groupSecondary}
              title={groupSecondaryLabel}
            >
              {groupSecondaryLabel}
            </button>
          )}
          {groupAction && files.length > 0 && (
            <button className="git-group-action" onClick={groupAction} title={groupActionLabel}>
              {groupActionLabel}
            </button>
          )}
        </span>
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
          {onSecondary && (
            <button
              className="git-file-action"
              onClick={(e) => {
                e.stopPropagation();
                onSecondary(f);
              }}
              title={secondaryTitle}
            >
              {secondaryLabel}
            </button>
          )}
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
