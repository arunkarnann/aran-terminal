import { useState } from "react";
import type { FocusController } from "../../stores/useFocus";
import { formatMinutes } from "../../lib/ui";
import { FocusRing } from "./FocusRing";

/** Goal presets, in minutes. */
const GOALS = [60, 120, 180, 240];

export function DailyGoalRing({ focus }: { focus: FocusController }) {
  const [editing, setEditing] = useState(false);
  const day = focus.day;

  // Count the in-progress focus block toward today's total, live.
  const live =
    focus.activeBlock && focus.activeBlock.kind === "focus"
      ? focus.now - focus.activeBlock.startedAt
      : 0;
  const focusMs = (day?.focusMs ?? 0) + Math.max(0, live);
  const goalMs = day?.goalMs ?? 0;
  const progress = goalMs > 0 ? focusMs / goalMs : 0;
  const met = goalMs > 0 && focusMs >= goalMs;
  const blocks = (day?.blocksCompleted ?? 0) + (live > 0 ? 1 : 0);
  const streak = day?.streakDays ?? 0;

  return (
    <div className="focus-goal">
      <FocusRing
        progress={progress}
        size={132}
        stroke={10}
        color={met ? "var(--success)" : "var(--accent)"}
      >
        <div className="focus-goal-now">{formatMinutes(focusMs)}</div>
        <div className="focus-goal-of">of {formatMinutes(goalMs)}</div>
      </FocusRing>

      <div className="focus-goal-side">
        <div className="focus-goal-title">
          Today{met && <span className="focus-goal-met">✓ goal met</span>}
        </div>

        <div className="focus-goal-stats">
          <span title="Completed focus blocks today">
            <strong>{blocks}</strong> {blocks === 1 ? "block" : "blocks"}
          </span>
          <span className="focus-streak" title="Consecutive days with focus">
            🔥 <strong>{streak}</strong> day{streak === 1 ? "" : "s"}
          </span>
        </div>

        <div className="focus-goal-dots" aria-hidden>
          {Array.from({ length: Math.min(blocks, 12) }).map((_, i) => (
            <span key={i} className="focus-dot" />
          ))}
        </div>

        {editing ? (
          <div className="focus-goal-edit">
            {GOALS.map((m) => (
              <button
                key={m}
                className={`chip ${goalMs === m * 60_000 ? "chip--on" : ""}`}
                onClick={() => {
                  void focus.setGoal(m * 60_000);
                  setEditing(false);
                }}
              >
                {formatMinutes(m * 60_000)}
              </button>
            ))}
          </div>
        ) : (
          <button
            className="focus-goal-edit-btn"
            onClick={() => setEditing(true)}
          >
            Set goal
          </button>
        )}
      </div>
    </div>
  );
}
