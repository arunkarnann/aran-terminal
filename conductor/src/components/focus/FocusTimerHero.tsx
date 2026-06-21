import { useEffect, useMemo, useState } from "react";
import type { SessionId } from "../../ipc/types";
import type { FocusController } from "../../stores/useFocus";
import type { UiSession } from "../../stores/useSessionManager";
import { formatCountdown } from "../../lib/ui";
import { FocusRing } from "./FocusRing";

/** Focus-block duration presets, in minutes. */
const DURATIONS = [25, 50, 90];

interface HeroProps {
  focus: FocusController;
  sessions: UiSession[];
  activeSessionId: SessionId | null;
}

export function FocusTimerHero({ focus, sessions, activeSessionId }: HeroProps) {
  if (focus.activeBlock) {
    return <RunningHero focus={focus} sessions={sessions} />;
  }
  return (
    <StartComposer
      focus={focus}
      sessions={sessions}
      activeSessionId={activeSessionId}
    />
  );
}

function RunningHero({
  focus,
  sessions,
}: {
  focus: FocusController;
  sessions: UiSession[];
}) {
  const block = focus.activeBlock!;
  const elapsed = focus.now - block.startedAt;
  const progress = block.plannedMs > 0 ? elapsed / block.plannedMs : 1;
  const isBreak = block.kind === "break";
  const session = sessions.find((s) => s.id === block.sessionId);
  const who =
    session?.name ?? session?.project ?? (isBreak ? "Break" : "Focus");
  const color = isBreak ? "var(--idle)" : "var(--primary)";

  return (
    <div className="focus-hero">
      <FocusRing
        progress={progress}
        size={300}
        stroke={14}
        color={color}
        className="focus-hero-ring"
      >
        <div className="focus-hero-clock">{formatCountdown(focus.remainingMs)}</div>
        <div className="focus-hero-kind">{isBreak ? "break" : "deep work"}</div>
      </FocusRing>

      <div className="focus-hero-meta">
        <div className="focus-hero-who">{who}</div>
        {block.taskLabel && (
          <div className="focus-hero-task">{block.taskLabel}</div>
        )}
      </div>

      <div className="focus-hero-controls">
        <button className="btn" onClick={() => void focus.extend(5 * 60_000)}>
          +5 min
        </button>
        {!isBreak && (
          <button
            className="btn btn-primary"
            onClick={() => void focus.stop(true)}
            title="Finish now and count this block"
          >
            Done
          </button>
        )}
        <button
          className="btn focus-stop"
          onClick={() => void focus.stop(false)}
          title={isBreak ? "End break" : "Stop without counting it"}
        >
          {isBreak ? "End break" : "Stop"}
        </button>
      </div>
    </div>
  );
}

function StartComposer({
  focus,
  sessions,
  activeSessionId,
}: {
  focus: FocusController;
  sessions: UiSession[];
  activeSessionId: SessionId | null;
}) {
  const [sessionId, setSessionId] = useState<SessionId | null>(activeSessionId);
  const [minutes, setMinutes] = useState(DURATIONS[0]);
  const [custom, setCustom] = useState("");
  const [intention, setIntention] = useState("");

  // Default the target to the active session, and keep it valid as sessions change.
  useEffect(() => {
    setSessionId((cur) => {
      if (cur && sessions.some((s) => s.id === cur)) return cur;
      return activeSessionId ?? sessions[0]?.id ?? null;
    });
  }, [activeSessionId, sessions]);

  const target = sessions.find((s) => s.id === sessionId);
  const plannedMin = useMemo(() => {
    const c = parseInt(custom, 10);
    return custom && c > 0 ? c : minutes;
  }, [custom, minutes]);

  const canStart = sessions.length > 0 && sessionId != null && plannedMin > 0;

  const start = () => {
    if (!canStart) return;
    void focus.start({
      sessionId,
      plannedMs: plannedMin * 60_000,
      taskLabel: intention.trim() || target?.taskLabel || null,
      kind: "focus",
    });
    setIntention("");
    setCustom("");
  };

  return (
    <div className="focus-hero focus-composer">
      <FocusRing progress={0} size={300} stroke={14} color="var(--primary)">
        <div className="focus-hero-clock focus-hero-clock--idle">
          {String(plannedMin).padStart(2, "0")}:00
        </div>
        <div className="focus-hero-kind">ready</div>
      </FocusRing>

      <div className="focus-composer-fields">
        {sessions.length === 0 ? (
          <div className="dashboard-empty">
            Open a terminal first, then start a focus block.
          </div>
        ) : (
          <>
            <label className="focus-field">
              <span className="focus-field-label">Session</span>
              <select
                className="focus-select"
                value={sessionId ?? ""}
                onChange={(e) => setSessionId(e.target.value)}
              >
                {sessions.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name ?? s.project ?? `Session ${s.id.slice(0, 6)}`}
                  </option>
                ))}
              </select>
            </label>

            <div className="focus-field">
              <span className="focus-field-label">Duration</span>
              <div className="focus-durations">
                {DURATIONS.map((m) => (
                  <button
                    key={m}
                    className={`chip ${!custom && minutes === m ? "chip--on" : ""}`}
                    onClick={() => {
                      setMinutes(m);
                      setCustom("");
                    }}
                  >
                    {m}m
                  </button>
                ))}
                <input
                  className="focus-custom"
                  type="number"
                  min={1}
                  placeholder="custom"
                  value={custom}
                  onChange={(e) => setCustom(e.target.value)}
                />
              </div>
            </div>

            <label className="focus-field">
              <span className="focus-field-label">Intention</span>
              <input
                className="focus-intention"
                value={intention}
                placeholder={target?.taskLabel ?? "What will you focus on?"}
                onChange={(e) => setIntention(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") start();
                }}
              />
            </label>

            <button
              className="btn btn-primary focus-start-btn"
              disabled={!canStart}
              onClick={start}
            >
              Start {plannedMin}-minute focus
            </button>
          </>
        )}
      </div>
    </div>
  );
}
