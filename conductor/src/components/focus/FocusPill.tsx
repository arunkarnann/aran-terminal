import type { FocusController } from "../../stores/useFocus";
import { formatCountdown } from "../../lib/ui";

/** Compact live countdown for the titlebar so a running block is never out of sight. */
export function FocusPill({
  focus,
  onClick,
}: {
  focus: FocusController;
  onClick: () => void;
}) {
  const block = focus.activeBlock;
  if (!block) return null;
  const isBreak = block.kind === "break";

  return (
    <button
      className={`focus-pill ${isBreak ? "focus-pill--break" : ""}`}
      onClick={onClick}
      title={isBreak ? "Break in progress" : "Focus block in progress"}
    >
      <span className="focus-pill-dot" />
      <span className="focus-pill-time">{formatCountdown(focus.remainingMs)}</span>
      <span className="focus-pill-kind">{isBreak ? "break" : "focus"}</span>
    </button>
  );
}
