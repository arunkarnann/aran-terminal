import { useEffect, useState } from "react";
import { getDailySummary, resetStats } from "../ipc/api";
import type { Summary } from "../ipc/types";
import { formatDuration } from "../lib/ui";

export function DailySummary({ onClose }: { onClose: () => void }) {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fetchSummary = () => {
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    getDailySummary(start.getTime(), Date.now())
      .then(setSummary)
      .catch((e) => setError(String(e)));
  };

  useEffect(() => {
    fetchSummary();
  }, []);

  const handleReset = () => {
    if (!confirm("Reset all historical stats? Open sessions are kept.")) return;
    resetStats().then(fetchSummary).catch((e) => setError(String(e)));
  };

  const maxMs = Math.max(1, ...(summary?.perProject.map((p) => p.activeMs) ?? [1]));

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog dialog--wide" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">Today</h2>
        {error && <p className="dialog-body">Couldn't load summary: {error}</p>}
        {!summary && !error && <p className="dialog-body">Loading…</p>}
        {summary && (
          <>
            <div className="summary-grid">
              <Stat
                big
                label="Agent-blocked time"
                value={formatDuration(summary.agentBlockedMs)}
                hint="time sessions spent waiting on you"
              />
              <Stat label="Sessions opened" value={String(summary.sessionsOpened)} />
              <Stat label="Commands run" value={String(summary.commandsRun)} />
              <Stat label="Cap overrides" value={String(summary.capOverrides)} />
            </div>

            <div className="dialog-label">Time per project</div>
            <div className="proj-bars">
              {summary.perProject.length === 0 ? (
                <div className="dashboard-empty">No activity yet today.</div>
              ) : (
                summary.perProject.map((p) => (
                  <div className="proj-row" key={p.project}>
                    <span className="proj-name">{p.project}</span>
                    <span className="proj-bar-wrap">
                      <span
                        className="proj-bar"
                        style={{ width: `${(p.activeMs / maxMs) * 100}%` }}
                      />
                    </span>
                    <span className="proj-time">{formatDuration(p.activeMs)}</span>
                  </div>
                ))
              )}
            </div>
          </>
        )}
        <div className="dialog-actions">
          <button className="btn btn-danger" onClick={handleReset}>
            Reset stats
          </button>
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  hint,
  big,
}: {
  label: string;
  value: string;
  hint?: string;
  big?: boolean;
}) {
  return (
    <div className={`stat ${big ? "stat--big" : ""}`}>
      <div className="stat-value">{value}</div>
      <div className="stat-label">{label}</div>
      {hint && <div className="stat-hint">{hint}</div>}
    </div>
  );
}
