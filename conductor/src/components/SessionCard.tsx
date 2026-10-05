import { useEffect, useState, type CSSProperties } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { UiSession } from "../stores/useSessionManager";
import { STATE_META, formatDuration, formatMem, formatRelative } from "../lib/ui";
import { getProjectColor, useProjectColors } from "../lib/projectColors";
import { IconFolderOpen, IconTag, IconX } from "./Icons";

interface SessionCardProps {
  session: UiSession;
  isActive: boolean;
  now: number;
  /** Largest RSS / uptime across visible sessions, for relative bar scaling. */
  maxRssKb: number;
  maxUptimeMs: number;
  onFocus: () => void;
  onSetLabel: (label: string) => void;
  onClose: () => void;
}

export function SessionCard({
  session,
  isActive,
  now,
  maxRssKb,
  maxUptimeMs,
  onFocus,
  onSetLabel,
  onClose,
}: SessionCardProps) {
  const meta = STATE_META[session.state];
  const isWaiting = session.state === "WAITING";
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.taskLabel ?? "");
  useProjectColors(); // re-render when a project color changes
  const projectColor = getProjectColor(session.project);

  const uptimeMs = Math.max(0, now - session.createdAt);
  const isRunning = session.state === "RUNNING";
  const ramPct =
    maxRssKb > 0 && session.rssKb ? Math.min(100, (session.rssKb / maxRssKb) * 100) : 0;
  const timePct = maxUptimeMs > 0 ? Math.min(100, (uptimeMs / maxUptimeMs) * 100) : 0;

  useEffect(() => {
    if (!editing) setDraft(session.taskLabel ?? "");
  }, [session.taskLabel, editing]);

  const title =
    session.name ?? session.project ?? `Session ${session.id.slice(0, 8)}`;

  const commit = () => {
    setEditing(false);
    const t = draft.trim();
    if (t && t !== (session.taskLabel ?? "")) onSetLabel(t);
  };

  return (
    <div
      className={`card card--${session.state.toLowerCase()} ${
        isActive ? "card--active" : ""
      }`}
      style={session.project ? ({ "--card-color": projectColor } as CSSProperties) : undefined}
      onClick={onFocus}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !editing) onFocus();
      }}
    >
      <div className="card-top">
        <span className={`card-dot ${isWaiting ? "card-dot--pulse" : ""}`} style={{ background: meta.color }} />
        <span className="card-title" title={title}>{title}</span>
        <div className="card-tools" onClick={(e) => e.stopPropagation()}>
          <button
            className="card-tool"
            onClick={() => setEditing(true)}
            title={session.taskLabel ? "Edit task label" : "Add task label"}
          >
            <IconTag size={13} />
          </button>
          {session.cwd && (
            <button
              className="card-tool"
              onClick={() => void revealItemInDir(session.cwd!)}
              title="Reveal in Finder"
            >
              <IconFolderOpen size={13} />
            </button>
          )}
          <button className="card-tool card-tool--danger" onClick={onClose} title="Close session">
            <IconX size={13} />
          </button>
        </div>
      </div>

      {/* The wait clock — the signature: invisible lost time made visible. */}
      {isWaiting && session.waitingSince != null && (
        <div className="card-wait">
          Waiting {formatDuration(now - session.waitingSince)}
          <span className="card-wait-hint">tap to respond</span>
        </div>
      )}

      <div className="card-chips">
        {editing ? (
          <input
            className="card-label-input"
            value={draft}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") setEditing(false);
            }}
            onClick={(e) => e.stopPropagation()}
            placeholder="What's this for?"
          />
        ) : (
          <button
            className={`card-label ${session.taskLabel ? "" : "card-label--empty"}`}
            onClick={(e) => {
              e.stopPropagation();
              setEditing(true);
            }}
            title="Click to set a task label"
          >
            <IconTag size={11} />
            <span className="card-label-text">{session.taskLabel ?? "Add label"}</span>
          </button>
        )}
        {session.project && !editing && (
          <span className="card-proj" title={session.cwd ?? session.project}>
            <span className="card-proj-dot" style={{ background: projectColor }} />
            {session.project}
          </span>
        )}
        {!editing && (
          <span className="card-state">
            {!isWaiting && <span style={{ color: meta.color }}>{meta.label}</span>}
            <span title="Commands run">{session.commandCount} cmds</span>
            {!isWaiting && (
              <span title="Last activity">{formatRelative(session.lastActivityAt, now)}</span>
            )}
          </span>
        )}
      </div>

      <div className="card-bars">
        <div className="bar-row" title="Memory (shell + all child processes), relative to the heaviest session">
          <span className="bar-k">RAM</span>
          <span className="bar-track">
            <span
              className={`bar-fill bar-fill--ram ${isRunning ? "bar-fill--live" : ""}`}
              style={{ width: `${ramPct}%` }}
            />
          </span>
          <span className="bar-v">{formatMem(session.rssKb)}</span>
        </div>
        <div className="bar-row" title="Uptime, relative to the longest-running session">
          <span className="bar-k">Up</span>
          <span className="bar-track">
            <span
              className="bar-fill bar-fill--time"
              style={{ width: `${timePct}%`, background: projectColor }}
            />
          </span>
          <span className="bar-v">{formatDuration(uptimeMs)}</span>
        </div>
      </div>
    </div>
  );
}
