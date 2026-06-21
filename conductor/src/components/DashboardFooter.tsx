import { useEffect, useState } from "react";
import { getAppMem, getDailySummary } from "../ipc/api";
import type { Summary } from "../ipc/types";
import { formatDuration, formatMem } from "../lib/ui";
import { getProjectColor, useProjectColors } from "../lib/projectColors";

/**
 * Always-visible "Today" strip pinned to the bottom of the dashboard: where time
 * went per project, plus how much memory the whole app is using. Mirrors the
 * Today modal's data but stays in view. Refreshes on a slow poll (5s) — these
 * numbers move slowly and we don't want to ride the 1s uptime tick.
 */
export function DashboardFooter({ onOpenToday }: { onOpenToday: () => void }) {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [appKb, setAppKb] = useState<number | null>(null);
  useProjectColors(); // re-render when a project color changes

  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      const start = new Date();
      start.setHours(0, 0, 0, 0);
      try {
        const [s, m] = await Promise.all([
          getDailySummary(start.getTime(), Date.now()),
          getAppMem(),
        ]);
        if (alive) {
          setSummary(s);
          setAppKb(m);
        }
      } catch {
        /* transient — keep last good values */
      }
    };
    void refresh();
    const iv = setInterval(refresh, 5000);
    return () => {
      alive = false;
      clearInterval(iv);
    };
  }, []);

  const projects = summary?.perProject ?? [];
  const maxMs = Math.max(1, ...projects.map((p) => p.activeMs));
  // Keep the strip compact — show the busiest few, note the rest.
  const top = projects.slice(0, 3);
  const hidden = projects.length - top.length;

  return (
    <div className="dash-footer">
      <button className="dash-footer-head" onClick={onOpenToday} title="Open full Today summary">
        <span className="dash-footer-title">Today</span>
        {summary && (
          <span className="dash-footer-summary">
            {formatDuration(summary.agentBlockedMs)} blocked · {summary.sessionsOpened} sessions ·{" "}
            {summary.commandsRun} cmds
          </span>
        )}
      </button>

      {top.length > 0 ? (
        <div className="dash-footer-projects">
          {top.map((p) => (
            <div className="dash-foot-row" key={p.project}>
              <span className="dash-foot-name">
                <span
                  className="dash-foot-dot"
                  style={{ background: getProjectColor(p.project) }}
                />
                {p.project}
              </span>
              <span className="dash-foot-track">
                <span
                  className="dash-foot-fill"
                  style={{
                    width: `${(p.activeMs / maxMs) * 100}%`,
                    background: getProjectColor(p.project),
                  }}
                />
              </span>
              <span className="dash-foot-time">{formatDuration(p.activeMs)}</span>
            </div>
          ))}
          {hidden > 0 && <div className="dash-foot-more">+{hidden} more</div>}
        </div>
      ) : (
        <div className="dash-foot-empty">No activity yet today.</div>
      )}

      <div className="dash-footer-mem" title="Memory used by the whole app (this process + the web view)">
        <span>App memory</span>
        <span className="dash-footer-mem-v">{appKb != null ? formatMem(appKb) : "—"}</span>
      </div>
    </div>
  );
}
