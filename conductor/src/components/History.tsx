import { useEffect, useState } from "react";
import { getCommandHistory } from "../ipc/api";
import type { HistoryEntry } from "../ipc/types";
import { formatDuration } from "../lib/ui";

interface HistoryProps {
  onClose: () => void;
  onInsert: (cmd: string) => void;
}

export function History({ onClose, onInsert }: HistoryProps) {
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const t = setTimeout(() => {
      getCommandHistory(search || undefined, 200)
        .then((e) => !cancelled && setEntries(e))
        .catch(() => {})
        .finally(() => !cancelled && setLoading(false));
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [search]);

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog dialog--wide" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">Command history</h2>
        <input
          className="hist-search"
          autoFocus
          placeholder="Search commands…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className="hist-list">
          {loading && entries.length === 0 ? (
            <div className="dashboard-empty">Loading…</div>
          ) : entries.length === 0 ? (
            <div className="dashboard-empty">
              {search
                ? "No matching commands."
                : "No commands yet. Run something in a terminal."}
            </div>
          ) : (
            entries.map((e, i) => (
              <button
                key={i}
                className="hist-row"
                title="Insert into the active terminal"
                onClick={() => {
                  onInsert(e.cmdline);
                  onClose();
                }}
              >
                <code className="hist-cmd">{e.cmdline}</code>
                <span className="hist-meta">
                  {e.project && <span className="hist-proj">{e.project}</span>}
                  {e.durationMs != null && e.durationMs > 0 && (
                    <span>{formatDuration(e.durationMs)}</span>
                  )}
                  {e.exitCode != null && e.exitCode !== 0 && (
                    <span className="hist-fail">exit {e.exitCode}</span>
                  )}
                </span>
              </button>
            ))
          )}
        </div>
        <div className="dialog-actions">
          <span className="hist-hint">Click a command to drop it into the active terminal.</span>
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
