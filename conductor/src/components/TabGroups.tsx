// Two-tier horizontal tabs: row 1 is the group tags (one per project folder),
// row 2 is the tabs of the active group. The active group always follows the
// active session; clicking another group tag jumps to that group's first tab.

import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { SessionId, SessionMeta } from "../ipc/types";
import { Tab, TabGroup, groupSessions } from "./TabStrip";
import { useNow } from "../lib/useNow";
import { getProjectColor, useProjectColors } from "../lib/projectColors";
import { tabDensity, useElementWidth } from "../lib/useTabDensity";
import { IconPlus, IconX } from "./Icons";

interface TabGroupsProps {
  sessions: SessionMeta[];
  activeSessionId: SessionId | null;
  onAdd: (cwd?: string) => void;
  onClose: (id: SessionId) => void;
  onSwitch: (id: SessionId) => void;
  onRename: (id: SessionId, name: string) => void;
  /** Right-aligned controls (Today/Settings/Group/Dashboard) rendered on the group row. */
  actions?: ReactNode;
}

export function TabGroups({
  sessions,
  activeSessionId,
  onAdd,
  onClose,
  onSwitch,
  onRename,
  actions,
}: TabGroupsProps) {
  const now = useNow(30_000);
  useProjectColors();
  const groups = groupSessions(sessions);
  const activeGroup =
    groups.find((g) => g.sessions.some((s) => s.id === activeSessionId)) ?? groups[0];

  // Track the last active tab per group so switching back restores your place.
  const lastActiveRef = useRef<Record<string, SessionId>>({});

  useEffect(() => {
    if (activeSessionId) {
      const g = groups.find((gg) => gg.sessions.some((s) => s.id === activeSessionId));
      if (g) lastActiveRef.current[g.key] = activeSessionId;
    }
  }, [activeSessionId, groups]);

  // Group pending confirmation to close (null = no dialog open).
  const [pendingClose, setPendingClose] = useState<TabGroup | null>(null);

  const stripRef = useRef<HTMLDivElement>(null);
  const density = tabDensity(useElementWidth(stripRef), activeGroup?.sessions.length ?? 0);

  const confirmCloseGroup = () => {
    if (pendingClose) {
      for (const s of pendingClose.sessions) onClose(s.id);
    }
    setPendingClose(null);
  };

  return (
    <div className="tab2">
      {/* Row 1 — group tags */}
      <div className="tab2-groups">
        {groups.map((g) => {
          const active = g.key === activeGroup?.key;
          const color = getProjectColor(g.project);
          return (
            <div
              key={g.key}
              role="button"
              tabIndex={0}
              className={`tab2-group ${active ? "tab2-group--on" : ""}`}
              style={{ "--tab-color": color } as CSSProperties}
              onClick={() => {
                if (active) return;
                const lastId = lastActiveRef.current[g.key];
                const targetId =
                  lastId && g.sessions.some((s) => s.id === lastId)
                    ? lastId
                    : g.sessions[0]?.id;
                if (targetId) onSwitch(targetId);
              }}
              title={g.cwd ?? g.project ?? "Loose"}
            >
              <span className="tab2-group-dot" />
              <span className="tab2-group-name">{g.project ?? "Loose"}</span>
              <span className="tab2-group-count">{g.sessions.length}</span>
              <button
                className="tab2-group-close"
                onClick={(e) => {
                  e.stopPropagation();
                  // A single-tab group is just a tab: close it directly, no dialog.
                  if (g.sessions.length === 1) onClose(g.sessions[0].id);
                  else setPendingClose(g);
                }}
                title={`Close group — closes all ${g.sessions.length} tab${
                  g.sessions.length === 1 ? "" : "s"
                }`}
              >
                <IconX size={11} />
              </button>
            </div>
          );
        })}
        <button
          className="tab-add tab-add--global"
          onClick={() => onAdd()}
          title="New terminal (fresh folder)  ⌘N"
        >
          <IconPlus size={14} />
          <span className="tb-label">New</span>
        </button>
        {actions}
      </div>

      {/* Row 2 — tabs of the active group */}
      <div className="tab-strip tab2-tabs" ref={stripRef} data-density={density}>
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
          // Prefer the active tab's own folder (the one you're actually in);
          // fall back to the group's representative folder.
          onClick={() => {
            const activeSession = activeGroup?.sessions.find(
              (s) => s.id === activeSessionId,
            );
            onAdd(activeSession?.cwd ?? activeGroup?.cwd ?? undefined);
          }}
          title={
            activeGroup?.project
              ? `New tab in ${activeGroup.project}  ⌘T`
              : "New tab in this folder  ⌘T"
          }
        >
          <IconPlus size={15} />
        </button>
      </div>

      {pendingClose && (
        <div className="dialog-overlay" onClick={() => setPendingClose(null)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h2 className="dialog-title">Close group?</h2>
            <p className="dialog-body">
              This closes all {pendingClose.sessions.length} terminal
              {pendingClose.sessions.length === 1 ? "" : "s"} in{" "}
              <strong>{pendingClose.project ?? "Loose"}</strong>. This can't be undone.
            </p>
            <div className="dialog-actions">
              <button className="btn btn-danger" onClick={confirmCloseGroup}>
                Close all {pendingClose.sessions.length}
              </button>
              <button className="btn" onClick={() => setPendingClose(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
