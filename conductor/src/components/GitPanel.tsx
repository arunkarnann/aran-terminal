// Zed-style git panel for the active terminal's working directory.
// Two tabs — History (commit log) and Changes (working-tree status). Clicking a
// commit or a changed file opens its diff. Read-only; data comes from the
// shell-out git commands in src-tauri/src/git.rs.

import { useCallback, useEffect, useState } from "react";
import { gitDiff, gitLog, gitRepos, gitShow, gitStatus } from "../ipc/api";
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
  // Repos discovered under the active terminal's folder (monorepo support), and
  // which one the panel is currently showing.
  const [repos, setRepos] = useState<GitRepo[]>([]);
  const [repoPath, setRepoPath] = useState<string | null>(null);
  const [gitWidth, handleProps] = useResizable("right", "conductor-git-width", 300, 220, 480);

  // Discover repos when the active terminal's directory changes. Keep the current
  // selection if it's still present; otherwise pick the cwd's own repo or the first.
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

  // Refresh on repo change + on a light interval while open. Reset the open diff
  // when the repo changes.
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

  const selectedRepo = repos.find((r) => r.path === repoPath) ?? null;
  const branch = status?.branch ?? "—";
  const dirty =
    (status?.staged.length ?? 0) +
      (status?.unstaged.length ?? 0) +
      (status?.untracked.length ?? 0) >
    0;

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
        <div className="git-scroll">
          <ChangeGroup
            label="Staged"
            files={status?.staged ?? []}
            onPick={(f) => openFile(f, true)}
          />
          <ChangeGroup
            label="Unstaged"
            files={status?.unstaged ?? []}
            onPick={(f) => openFile(f, false)}
          />
          {(status?.untracked.length ?? 0) > 0 && (
            <section className="git-group">
              <div className="git-group-label">Untracked</div>
              {status?.untracked.map((p) => (
                <div key={p} className="git-file git-file--untracked">
                  <span className="git-code git-code--untracked">?</span>
                  <span className="git-path">{p}</span>
                </div>
              ))}
            </section>
          )}
          {!dirty && <div className="git-empty">Working tree clean.</div>}
        </div>
      )}
    </aside>
  );
}

function ChangeGroup({
  label,
  files,
  onPick,
}: {
  label: string;
  files: GitFileChange[];
  onPick: (f: GitFileChange) => void;
}) {
  if (files.length === 0) return null;
  return (
    <section className="git-group">
      <div className="git-group-label">{label}</div>
      {files.map((f) => (
        <button key={`${label}:${f.path}`} className="git-file" onClick={() => onPick(f)}>
          <span className={`git-code git-code--${f.code.toLowerCase()}`}>{f.code}</span>
          <span className="git-path">{f.path}</span>
          <span className="git-numstat">
            {f.insertions > 0 && <span className="git-add">+{f.insertions}</span>}
            {f.deletions > 0 && <span className="git-del">-{f.deletions}</span>}
          </span>
        </button>
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
