// Two-tier horizontal tabs: row 1 is the group tags (one per project folder),
// row 2 is the tabs of the active group. The active group always follows the
// active session; clicking another group tag jumps to that group's first tab.

import type { SessionId, SessionMeta } from "../ipc/types";
import { Tab, groupSessions } from "./TabStrip";
import { useNow } from "../lib/useNow";
import { getProjectColor, useProjectColors } from "../lib/projectColors";

interface TabGroupsProps {
  sessions: SessionMeta[];
  activeSessionId: SessionId | null;
  onAdd: (cwd?: string) => void;
  onClose: (id: SessionId) => void;
  onSwitch: (id: SessionId) => void;
  onRename: (id: SessionId, name: string) => void;
}

export function TabGroups({
  sessions,
  activeSessionId,
  onAdd,
  onClose,
  onSwitch,
  onRename,
}: TabGroupsProps) {
  const now = useNow(30_000);
  useProjectColors();
  const groups = groupSessions(sessions);
  const activeGroup =
    groups.find((g) => g.sessions.some((s) => s.id === activeSessionId)) ?? groups[0];

  return (
    <div className="tab2">
      {/* Row 1 — group tags */}
      <div className="tab2-groups">
        {groups.map((g) => {
          const active = g.key === activeGroup?.key;
          const color = getProjectColor(g.project);
          return (
            <button
              key={g.key}
              className={`tab2-group ${active ? "tab2-group--on" : ""}`}
              style={active ? { boxShadow: `inset 0 -2px 0 ${color}` } : undefined}
              onClick={() => {
                if (!active && g.sessions[0]) onSwitch(g.sessions[0].id);
              }}
              title={g.cwd ?? g.project ?? "Loose"}
            >
              <span className="tab2-group-dot" style={{ background: color }} />
              {g.project ?? "Loose"}
              <span className="tab2-group-count">{g.sessions.length}</span>
            </button>
          );
        })}
        <button
          className="tab-add tab-add--global"
          onClick={() => onAdd()}
          title="New terminal (fresh folder)  ⌘N"
        >
          ＋ New
        </button>
      </div>

      {/* Row 2 — tabs of the active group */}
      <div className="tab-strip tab2-tabs">
        {activeGroup?.sessions.map((s) => (
          <Tab
            key={s.id}
            session={s}
            now={now}
            isActive={s.id === activeSessionId}
            onClose={() => onClose(s.id)}
            onSelect={() => onSwitch(s.id)}
            onRename={(name) => onRename(s.id, name)}
          />
        ))}
        <button
          className="tab-add tab-add--group"
          onClick={() => onAdd(activeGroup?.cwd ?? undefined)}
          title={
            activeGroup?.project
              ? `New tab in ${activeGroup.project}  ⌘T`
              : "New tab in this folder  ⌘T"
          }
        >
          +
        </button>
      </div>
    </div>
  );
}
