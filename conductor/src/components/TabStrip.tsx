import { useEffect, useRef, useState, type CSSProperties } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AttentionState, SessionId, SessionMeta } from "../ipc/types";
import { ageTier, formatMem, formatMinutes, memTier } from "../lib/ui";
import { useNow } from "../lib/useNow";
import { getProjectColor, inkOn, setProjectColor, useProjectColors } from "../lib/projectColors";
import { tabDensity, useElementWidth } from "../lib/useTabDensity";
import { IconAlert, IconFolderOpen, IconPlus, IconX } from "./Icons";

const STATE_META: Record<AttentionState, { color: string; label: string }> = {
  RUNNING: { color: "var(--running)", label: "Running" },
  IDLE: { color: "var(--idle)", label: "Idle" },
  WAITING: { color: "var(--waiting)", label: "Waiting for you" },
};

interface TabStripProps {
  sessions: SessionMeta[];
  activeSessionId: SessionId | null;
  onAdd: (cwd?: string) => void;
  onClose: (id: SessionId) => void;
  onSwitch: (id: SessionId) => void;
  onRename: (id: SessionId, name: string) => void;
}

export interface TabGroup {
  key: string;
  project: string | null;
  cwd: string | null;
  sessions: SessionMeta[];
}

/** Bucket sessions by project (folder), preserving first-seen order. Sessions
 *  whose folder isn't known yet collect in a single "loose" group. */
export function groupSessions(sessions: SessionMeta[]): TabGroup[] {
  const map = new Map<string, TabGroup>();
  for (const s of sessions) {
    const key = s.project ?? "__loose__";
    let g = map.get(key);
    if (!g) {
      g = { key, project: s.project, cwd: s.cwd, sessions: [] };
      map.set(key, g);
    }
    if (!g.cwd && s.cwd) g.cwd = s.cwd; // representative folder for the group "+"
    g.sessions.push(s);
  }
  return [...map.values()];
}

export function TabStrip({
  sessions,
  activeSessionId,
  onAdd,
  onClose,
  onSwitch,
  onRename,
}: TabStripProps) {
  const now = useNow(30_000);
  useProjectColors(); // subscribe so tabs re-render when a project color changes
  // "+" opens the new tab in the active tab's folder (⌘T); ⌘N opens a fresh one.
  const activeCwd = sessions.find((s) => s.id === activeSessionId)?.cwd ?? undefined;
  const stripRef = useRef<HTMLDivElement>(null);
  const density = tabDensity(useElementWidth(stripRef), sessions.length);

  return (
    <div className="tab-strip" ref={stripRef} data-density={density}>
      {sessions.map((s) => (
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
        className="tab-add"
        onClick={() => onAdd(activeCwd)}
        title={activeCwd ? `New tab in current folder  ⌘T` : "New terminal  ⌘N"}
      >
        <IconPlus size={15} />
      </button>
    </div>
  );
}

interface TabProps {
  session: SessionMeta;
  now: number;
  isActive: boolean;
  vertical?: boolean;
  onClose: () => void;
  onSelect: () => void;
  onRename: (name: string) => void;
}

export function Tab({ session, now, isActive, vertical, onClose, onSelect, onRename }: TabProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.name ?? "");
  const inputRef = useRef<HTMLInputElement>(null);
  const tabRef = useRef<HTMLDivElement>(null);

  // Keep the active tab visible when the strip has to scroll.
  useEffect(() => {
    if (isActive) tabRef.current?.scrollIntoView({ inline: "nearest", block: "nearest" });
  }, [isActive]);

  // Auto name: "<command> › <folder>" — command first since grouped tabs already show folder.
  const folder = session.project;
  const cmd = session.lastCommand?.trim();
  const autoName =
    cmd && folder
      ? `${cmd} › ${folder}`
      : cmd || folder || `Session ${session.id.slice(0, 8)}`;
  const displayName = session.name ?? autoName;

  const handleDoubleClick = () => {
    setDraft(session.name ?? "");
    setEditing(true);
    setTimeout(() => inputRef.current?.select(), 0);
  };

  const commitRename = () => {
    setEditing(false);
    const trimmed = draft.trim();
    if (trimmed && trimmed !== (session.name ?? "")) {
      onRename(trimmed);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      commitRename();
    } else if (e.key === "Escape") {
      setEditing(false);
    }
  };

  const stateMeta = STATE_META[session.state];

  const uptimeMs = Math.max(0, now - session.createdAt);
  const tier = ageTier(uptimeMs);
  const mem = memTier(session.rssKb);
  const projectColor = getProjectColor(session.project);

  return (
    <div
      ref={tabRef}
      className={`tab tab--age-${tier} tab--mem-${mem} ${vertical ? "tab--vertical" : ""} ${
        isActive ? "tab--active" : ""
      } ${session.state === "WAITING" ? "tab--waiting" : ""}`}
      // Project color as a CSS var so the stylesheet decides where it shows.
      style={
        session.project
          ? ({ "--tab-color": projectColor, "--tab-ink": inkOn(projectColor) } as CSSProperties)
          : undefined
      }
      onClick={onSelect}
      onDoubleClick={handleDoubleClick}
    >
      {session.project && (
        <label
          className="tab-swatch"
          style={{ background: projectColor }}
          title={`${session.project} — click to change color`}
          onClick={(e) => e.stopPropagation()}
        >
          <input
            type="color"
            value={projectColor}
            onChange={(e) => setProjectColor(session.project as string, e.target.value)}
          />
        </label>
      )}
      <span
        className="tab-state-dot"
        style={{ background: stateMeta.color }}
        title={stateMeta.label}
      />
      {editing ? (
        <input
          ref={inputRef}
          className="tab-rename-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitRename}
          onKeyDown={handleKeyDown}
          onClick={(e) => e.stopPropagation()}
          autoFocus
        />
      ) : (
        <span className="tab-label" title={displayName}>{displayName}</span>
      )}
      <span
        className={`tab-uptime tab-uptime--${tier}`}
        title={`Open for ${formatMinutes(uptimeMs)}`}
      >
        {formatMinutes(uptimeMs)}
      </span>
      {session.rssKb != null && (
        <span
          className={`tab-mem tab-mem--${mem}`}
          title={
            mem === "critical"
              ? `High memory: ${formatMem(session.rssKb)} — consider restarting this session`
              : mem === "high"
                ? `Memory climbing: ${formatMem(session.rssKb)}`
                : "Memory (process group)"
          }
        >
          {mem !== "normal" && <IconAlert size={10} className="tab-mem-warn" />}
          {formatMem(session.rssKb)}
        </span>
      )}
      {session.cwd && (
        <button
          className="tab-reveal"
          onClick={(e) => {
            e.stopPropagation();
            revealItemInDir(session.cwd!);
          }}
          title="Reveal in Finder"
        >
          <IconFolderOpen size={13} />
        </button>
      )}
      <button
        className="tab-close"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        title="Close session"
      >
        <IconX size={12} />
      </button>
    </div>
  );
}
