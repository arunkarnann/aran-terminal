import { useEffect, useState } from "react";
import type { UiSession } from "../stores/useSessionManager";
import { STATE_META, formatDuration, formatMem, formatRelative } from "../lib/ui";
import { getProjectColor, useProjectColors } from "../lib/projectColors";

interface SessionCardProps {
  session: UiSession;
  isActive: boolean;
  now: number;
  /** Largest RSS / uptime across visible sessions, for relative bar scaling. */
  maxRssKb: number;
  maxUptimeMs: number;
  onFocus: () => void;
  onSetLabel: (label: string) => void;
}

export function SessionCard({
  session,
  isActive,
  now,
  maxRssKb,
  maxUptimeMs,
  onFocus,
  onSetLabel,
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
      style={session.project ? { boxShadow: `inset 4px 0 0 ${projectColor}` } : undefined}
      onClick={onFocus}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" && !editing) onFocus();
      }}
    >
      <div className="card-top">
        <span className={`card-dot ${isWaiting ? "card-dot--pulse" : ""}`} style={{ background: meta.color }} />
        <span className="card-title">{title}</span>
        {!isWaiting && (
          <span className="card-state" style={{ color: meta.color }}>
            {meta.label}
          </span>
        )}
      </div>

      {/* The wait clock — the signature: invisible lost time made visible. */}
      {isWaiting && session.waitingSince != null && (
        <div className="card-wait">
          Waiting {formatDuration(now - session.waitingSince)}
          <span className="card-wait-hint">tap to respond</span>
        </div>
      )}

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
        <div
          className={`card-label ${session.taskLabel ? "" : "card-label--empty"}`}
          onClick={(e) => {
            e.stopPropagation();
            setEditing(true);
          }}
          title="Click to set a task label"
        >
          {session.taskLabel ?? "Add a task label"}
        </div>
      )}

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
          <span className="bar-k">Time</span>
          <span className="bar-track">
            <span
              className="bar-fill bar-fill--time"
              style={{ width: `${timePct}%`, background: projectColor }}
            />
          </span>
          <span className="bar-v">{formatDuration(uptimeMs)}</span>
        </div>
      </div>

      <div className="card-meta">
        {session.project && (
          <span className="card-proj">
            <span className="card-proj-dot" style={{ background: projectColor }} />
            {session.project}
          </span>
        )}
        <span title="Commands run">{session.commandCount} cmds</span>
        {!isWaiting && (
          <span title="Last activity" className="card-meta-spacer">
            {formatRelative(session.lastActivityAt, now)}
          </span>
        )}
      </div>
    </div>
  );
}
