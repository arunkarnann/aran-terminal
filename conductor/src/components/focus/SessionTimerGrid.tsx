import type { SessionId } from "../../ipc/types";
import type { FocusController } from "../../stores/useFocus";
import type { UiSession } from "../../stores/useSessionManager";
import { STATE_META, formatDuration } from "../../lib/ui";

interface GridProps {
  focus: FocusController;
  sessions: UiSession[];
  activeSessionId: SessionId | null;
  onPick: (id: SessionId) => void;
}

export function SessionTimerGrid({
  focus,
  sessions,
  activeSessionId,
  onPick,
}: GridProps) {
  if (sessions.length === 0) return null;
  const focusedId =
    focus.activeBlock?.kind === "focus" ? focus.activeBlock.sessionId : null;
  const blockRunning = focusedId != null;

  // Focused session first, then by urgency (waiting, then most recent).
  const ordered = [...sessions].sort((a, b) => {
    if (a.id === focusedId) return -1;
    if (b.id === focusedId) return 1;
    const order = { WAITING: 0, RUNNING: 1, IDLE: 2 } as const;
    return order[a.state] - order[b.state] || b.lastActivityAt - a.lastActivityAt;
  });

  return (
    <section className="focus-grid-wrap">
      <div className="focus-grid-head">Sessions</div>
      <div className="focus-grid">
        {ordered.map((s) => {
          const isFocused = s.id === focusedId;
          const receded = blockRunning && !isFocused;
          const queued = receded && s.state === "WAITING";
          return (
            <SessionTimerTile
              key={s.id}
              session={s}
              now={focus.now}
              isActive={s.id === activeSessionId}
              isFocused={isFocused}
              receded={receded}
              queued={queued}
              onPick={() => onPick(s.id)}
            />
          );
        })}
      </div>
    </section>
  );
}

interface TileProps {
  session: UiSession;
  now: number;
  isActive: boolean;
  isFocused: boolean;
  receded: boolean;
  queued: boolean;
  onPick: () => void;
}

function SessionTimerTile({
  session,
  now,
  isActive,
  isFocused,
  receded,
  queued,
  onPick,
}: TileProps) {
  const meta = STATE_META[session.state];
  const title =
    session.name ?? session.project ?? `Session ${session.id.slice(0, 6)}`;
  const waiting = session.state === "WAITING" && session.waitingSince != null;

  return (
    <button
      className={`focus-tile ${isActive ? "focus-tile--active" : ""} ${
        isFocused ? "focus-tile--focused" : ""
      } ${receded ? "focus-tile--receded" : ""}`}
      onClick={onPick}
    >
      <div className="focus-tile-top">
        <span className="focus-tile-dot" style={{ background: meta.color }} />
        <span className="focus-tile-title">{title}</span>
        {isFocused && <span className="focus-tile-tag">focusing</span>}
        {queued && <span className="focus-tile-tag focus-tile-tag--queued">queued</span>}
      </div>

      {session.taskLabel && (
        <div className="focus-tile-task">{session.taskLabel}</div>
      )}

      <div className="focus-tile-timers">
        {waiting ? (
          <span className="focus-tile-wait">
            waiting {formatDuration(now - session.waitingSince!)}
          </span>
        ) : (
          <span style={{ color: meta.color }}>{meta.label}</span>
        )}
        <span className="focus-tile-up">up {formatDuration(now - session.createdAt)}</span>
      </div>
    </button>
  );
}
