// Sidebar hosting the Dashboard, Tasks and Issues panes, docked left or right (swap
// button in its header). Only enabled panes get a tab; the stored tab wins when its
// pane is on, otherwise the first enabled pane shows.

import type { ReactNode } from "react";
import { useResizable } from "../lib/useResizable";
import { IconPanelSide } from "./Icons";

export type SidebarTab = "dashboard" | "tasks" | "issues";
export type SidebarSide = "left" | "right";

export interface SidebarPane {
  id: SidebarTab;
  label: string;
  icon: ReactNode;
  badge?: number;
  node: ReactNode;
}

/** The pane actually shown: the stored tab if enabled, else the first enabled one. */
export function visiblePane(enabled: SidebarTab[], tab: SidebarTab): SidebarTab | null {
  return enabled.includes(tab) ? tab : (enabled[0] ?? null);
}

interface SidebarProps {
  panes: SidebarPane[];
  tab: SidebarTab;
  onTabChange: (t: SidebarTab) => void;
  side: SidebarSide;
  onToggleSide: () => void;
}

export function Sidebar({ panes, tab, onTabChange, side, onToggleSide }: SidebarProps) {
  const [width, handleProps] = useResizable(side, "conductor-sidebar-width", 320, 240, 520);
  const current = visiblePane(
    panes.map((p) => p.id),
    tab,
  );
  if (!current) return null;
  const moveLabel = side === "left" ? "Move sidebar to the right" : "Move sidebar to the left";

  return (
    <aside className={`sidebar sidebar--${side}`} style={{ width }}>
      <div className={`resize-handle ${side === "left" ? "resize-handle--right" : ""}`} {...handleProps} />
      <div className="sidebar-tabs" role="tablist">
        {panes.map((p) => (
          <button
            key={p.id}
            role="tab"
            aria-selected={current === p.id}
            className={`sidebar-tab ${current === p.id ? "sidebar-tab--on" : ""}`}
            onClick={() => onTabChange(p.id)}
          >
            {p.icon}
            {p.label}
            {!!p.badge && <span className="tb-badge">{p.badge}</span>}
          </button>
        ))}
        <button className="sidebar-side" onClick={onToggleSide} title={moveLabel} aria-label={moveLabel}>
          <IconPanelSide flip={side === "left"} />
        </button>
      </div>
      <div className="sidebar-body">{panes.find((p) => p.id === current)?.node}</div>
    </aside>
  );
}
