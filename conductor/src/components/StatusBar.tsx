// Zed-style bottom status bar. Minimal: shows the active terminal's project and a
// compact git item (branch + change count) that toggles the Git panel.

import { useEffect, useState } from "react";
import { gitStatus } from "../ipc/api";
import type { GitStatus } from "../ipc/types";
import { getProjectColor, useProjectColors } from "../lib/projectColors";

interface StatusBarProps {
  cwd: string | null;
  project: string | null;
  gitOpen: boolean;
  onToggleGit: () => void;
}

export function StatusBar({ cwd, project, gitOpen, onToggleGit }: StatusBarProps) {
  const [status, setStatus] = useState<GitStatus | null>(null);
  useProjectColors(); // re-render on color changes

  useEffect(() => {
    let active = true;
    if (!cwd) {
      setStatus(null);
      return;
    }
    const load = () =>
      gitStatus(cwd)
        .then((s) => active && setStatus(s))
        .catch(() => {});
    load();
    const iv = setInterval(load, 5000);
    return () => {
      active = false;
      clearInterval(iv);
    };
  }, [cwd]);

  const isRepo = !!status?.isRepo;
  const changes = isRepo
    ? status!.staged.length + status!.unstaged.length + status!.untracked.length
    : 0;

  return (
    <footer className="statusbar">
      <div className="sb-left">
        {project && (
          <span className="sb-item sb-project">
            <span className="sb-dot" style={{ background: getProjectColor(project) }} />
            {project}
          </span>
        )}
      </div>
      <div className="sb-right">
        <button
          className={`sb-git ${gitOpen ? "sb-git--on" : ""}`}
          onClick={onToggleGit}
          title={isRepo ? `Git · ${status?.branch}` : "Git"}
        >
          <span className="sb-git-glyph">⎇</span>
          {isRepo ? (
            <>
              <span className="sb-branch">{status?.branch}</span>
              {changes > 0 && <span className="sb-changes">{changes}</span>}
              {status!.ahead > 0 && <span className="sb-ab">↑{status!.ahead}</span>}
              {status!.behind > 0 && <span className="sb-ab">↓{status!.behind}</span>}
            </>
          ) : (
            <span className="sb-branch sb-muted">{cwd ? "no repo" : "git"}</span>
          )}
        </button>
      </div>
    </footer>
  );
}
