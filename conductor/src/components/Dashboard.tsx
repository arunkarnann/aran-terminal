import { useEffect, useState } from "react";
import type { AttentionState, SessionId } from "../ipc/types";
import type { UiSession } from "../stores/useSessionManager";
import { SessionCard } from "./SessionCard";
import { DashboardFooter } from "./DashboardFooter";
import { formatDuration } from "../lib/ui";

type Filter = "ALL" | AttentionState;

const FILTERS: { key: Filter; label: string }[] = [
  { key: "ALL", label: "All" },
  { key: "RUNNING", label: "Running" },
  { key: "IDLE", label: "Idle" },
];

interface DashboardProps {
  sessions: UiSession[];
  activeSessionId: SessionId | null;
  onFocus: (id: SessionId) => void;
  onSetLabel: (id: SessionId, label: string) => void;
  onOpenToday: () => void;
}

const REST_ORDER: Record<AttentionState, number> = { WAITING: 0, RUNNING: 1, IDLE: 2 };

export function Dashboard({
  sessions,
  activeSessionId,
  onFocus,
  onSetLabel,
  onOpenToday,
}: DashboardProps) {
  const [filter, setFilter] = useState<Filter>("ALL");
  const [now, setNow] = useState(() => Date.now());

  // Tick once a second so the wait clock and uptimes stay live.
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, []);

  // Longest-waiting first — that's the most urgent.
  const waiting = sessions
    .filter((s) => s.state === "WAITING")
    .sort((a, b) => (a.waitingSince ?? 0) - (b.waitingSince ?? 0));

  const rest = sessions
    .filter((s) => s.state !== "WAITING")
    .filter((s) => filter === "ALL" || s.state === filter)
    .sort(
      (a, b) =>
        REST_ORDER[a.state] - REST_ORDER[b.state] ||
        b.lastActivityAt - a.lastActivityAt,
    );

  const running = sessions.filter((s) => s.state === "RUNNING").length;
  const idle = sessions.filter((s) => s.state === "IDLE").length;
  // Scale the per-card RAM/Time bars relative to the busiest/oldest session.
  const maxRssKb = Math.max(1, ...sessions.map((s) => s.rssKb ?? 0));
  const maxUptimeMs = Math.max(1, ...sessions.map((s) => now - s.createdAt));
  const longestWait =
    waiting.length > 0 ? now - (waiting[0].waitingSince ?? now) : 0;

  return (
    <aside className="dashboard">
      <Ribbon
        waiting={waiting.length}
        running={running}
        idle={idle}
        longestWaitMs={longestWait}
        empty={sessions.length === 0}
      />

      <div className="filters">
        {FILTERS.map((f) => (
          <button
            key={f.key}
            className={`chip ${filter === f.key ? "chip--on" : ""}`}
            onClick={() => setFilter(f.key)}
          >
            {f.label}
          </button>
        ))}
      </div>

      <div className="dashboard-scroll">
        {waiting.length > 0 && (
          <section className="needs-you">
            <div className="zone-label zone-label--alert">Needs you</div>
            {waiting.map((s) => (
              <SessionCard
                key={s.id}
                session={s}
                isActive={s.id === activeSessionId}
                now={now}
                maxRssKb={maxRssKb}
                maxUptimeMs={maxUptimeMs}
                onFocus={() => onFocus(s.id)}
                onSetLabel={(l) => onSetLabel(s.id, l)}
              />
            ))}
          </section>
        )}

        {sessions.length === 0 ? (
          <div className="dashboard-empty">
            No terminals open. Press <kbd>+</kbd> to start one.
          </div>
        ) : (
          <section>
            <div className="zone-label">
              {filter !== "ALL"
                ? title(filter)
                : waiting.length > 0
                  ? "Other sessions"
                  : "Sessions"}
            </div>
            {rest.length === 0 ? (
              <div className="dashboard-empty">
                {waiting.length > 0
                  ? "Everything else is handled."
                  : "Nothing here."}
              </div>
            ) : (
              rest.map((s) => (
                <SessionCard
                  key={s.id}
                  session={s}
                  isActive={s.id === activeSessionId}
                  now={now}
                  maxRssKb={maxRssKb}
                  maxUptimeMs={maxUptimeMs}
                  onFocus={() => onFocus(s.id)}
                  onSetLabel={(l) => onSetLabel(s.id, l)}
                />
              ))
            )}
          </section>
        )}
      </div>

      <DashboardFooter onOpenToday={onOpenToday} />
    </aside>
  );
}

function title(f: AttentionState) {
  return f[0] + f.slice(1).toLowerCase();
}

interface RibbonProps {
  waiting: number;
  running: number;
  idle: number;
  longestWaitMs: number;
  empty: boolean;
}

function Ribbon({ waiting, running, idle, longestWaitMs, empty }: RibbonProps) {
  if (empty) {
    return (
      <div className="ribbon ribbon--calm">
        <div className="ribbon-head">Aran Terminal</div>
        <div className="ribbon-sub">No sessions yet.</div>
      </div>
    );
  }
  if (waiting > 0) {
    return (
      <div className="ribbon ribbon--alert">
        <div className="ribbon-head">
          <span className="ribbon-pulse" />
          {waiting} {waiting === 1 ? "session needs" : "sessions need"} you
        </div>
        <div className="ribbon-sub">
          longest wait {formatDuration(longestWaitMs)}
        </div>
      </div>
    );
  }
  return (
    <div className="ribbon ribbon--calm">
      <div className="ribbon-head">All clear</div>
      <div className="ribbon-sub">
        {running} running · {idle} idle — nothing needs you
      </div>
    </div>
  );
}
