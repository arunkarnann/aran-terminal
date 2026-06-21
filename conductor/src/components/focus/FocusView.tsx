import type { SessionId } from "../../ipc/types";
import type { FocusController } from "../../stores/useFocus";
import type { UiSession } from "../../stores/useSessionManager";
import { BreakPrompt } from "./BreakPrompt";
import { DailyGoalRing } from "./DailyGoalRing";
import { FocusTimerHero } from "./FocusTimerHero";
import { SessionTimerGrid } from "./SessionTimerGrid";

interface FocusViewProps {
  focus: FocusController;
  sessions: UiSession[];
  activeSessionId: SessionId | null;
  onPickSession: (id: SessionId) => void;
}

/** Full-screen deep-work view: hero timer, daily goal, and per-session timers. */
export function FocusView({
  focus,
  sessions,
  activeSessionId,
  onPickSession,
}: FocusViewProps) {
  return (
    <div className="focus-view">
      <div className="focus-view-main">
        <FocusTimerHero
          focus={focus}
          sessions={sessions}
          activeSessionId={activeSessionId}
        />
      </div>

      <aside className="focus-view-side">
        <DailyGoalRing focus={focus} />
        <SessionTimerGrid
          focus={focus}
          sessions={sessions}
          activeSessionId={activeSessionId}
          onPick={onPickSession}
        />
      </aside>

      <BreakPrompt focus={focus} />
    </div>
  );
}
