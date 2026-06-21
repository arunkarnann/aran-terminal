import type { FocusController } from "../../stores/useFocus";
import { formatMinutes } from "../../lib/ui";

/** Break-length presets, in minutes. */
const BREAKS = [5, 15];

/** Shown right after a focus block lands — start a break or get back to it. */
export function BreakPrompt({ focus }: { focus: FocusController }) {
  const done = focus.completed;
  if (!done) return null;

  const startBreak = (min: number) => {
    void focus.start({ sessionId: null, plannedMs: min * 60_000, kind: "break" });
    focus.dismissCompleted();
  };

  return (
    <div className="dialog-overlay" onClick={focus.dismissCompleted}>
      <div className="dialog focus-break" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">Block complete 🎉</h2>
        <p className="dialog-body">
          {formatMinutes(done.plannedMs)} of deep work
          {done.taskLabel ? ` on “${done.taskLabel}”` : ""}. Take a break?
        </p>
        <div className="focus-break-options">
          {BREAKS.map((m) => (
            <button key={m} className="btn" onClick={() => startBreak(m)}>
              {formatMinutes(m * 60_000)} break
            </button>
          ))}
        </div>
        <div className="dialog-actions">
          <button className="btn btn-primary" onClick={focus.dismissCompleted}>
            Back to work
          </button>
        </div>
      </div>
    </div>
  );
}
