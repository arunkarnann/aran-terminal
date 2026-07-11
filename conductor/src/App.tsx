import { useCallback, useEffect, useRef, useState } from "react";
import { CapDialog } from "./components/CapDialog";
import { Dashboard } from "./components/Dashboard";
import { DailySummary } from "./components/DailySummary";
import { GitPanel } from "./components/GitPanel";
import { PermissionsSetup } from "./components/PermissionsSetup";
import { Settings } from "./components/Settings";
import { StatusBar } from "./components/StatusBar";
import { TabGroups } from "./components/TabGroups";
import { TabStrip } from "./components/TabStrip";
import { TerminalView } from "./components/TerminalView";
import { UpdateBanner } from "./components/UpdateBanner";
import { useSessionManager } from "./stores/useSessionManager";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { writeStdin } from "./ipc/api";
import "./App.css";

// Shell-escape a dropped path so spaces/quotes survive being typed into the shell.
function shellEscapePath(p: string): string {
  return p.includes(" ") || p.includes("'") || p.includes('"')
    ? `'${p.replace(/'/g, "'\\''")}'`
    : p;
}

const DEFAULT_FONT_FAMILY = "Menlo, Monaco, 'SF Mono', monospace";
const DEFAULT_FONT_SIZE = 13;

function loadFontFamily(): string {
  try { return localStorage.getItem("conductor-font-family") ?? DEFAULT_FONT_FAMILY; } catch { return DEFAULT_FONT_FAMILY; }
}
function loadFontSize(): number {
  try {
    const v = localStorage.getItem("conductor-font-size");
    return v ? Math.max(8, Math.min(32, Number(v))) : DEFAULT_FONT_SIZE;
  } catch { return DEFAULT_FONT_SIZE; }
}
function persistFont(family: string, size: number) {
  try { localStorage.setItem("conductor-font-family", family); } catch {}
  try { localStorage.setItem("conductor-font-size", String(size)); } catch {}
}

function App() {
  const mgr = useSessionManager();
  const [showDashboard, setShowDashboard] = useState(true);
  const [showGit, setShowGit] = useState(false);
  const [showSummary, setShowSummary] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  // First-launch macOS permissions onboarding (issue #3) — shown once, always
  // reachable later from Settings → macOS Permissions.
  const [showPermsSetup, setShowPermsSetup] = useState(() => {
    try {
      return localStorage.getItem("conductor-perms-onboarded") !== "1";
    } catch {
      return false;
    }
  });
  const dismissPermsSetup = useCallback(() => {
    setShowPermsSetup(false);
    try {
      localStorage.setItem("conductor-perms-onboarded", "1");
    } catch {
      /* ignore */
    }
  }, []);
  const [grouped, setGrouped] = useState(() => {
    try {
      return localStorage.getItem("conductor-tab-group") === "1";
    } catch {
      return false;
    }
  });
  const toggleGrouped = useCallback(() => {
    setGrouped((v) => {
      const n = !v;
      try {
        localStorage.setItem("conductor-tab-group", n ? "1" : "0");
      } catch {
        /* ignore */
      }
      return n;
    });
  }, []);
  const [fontFamily, setFontFamily] = useState(loadFontFamily);
  const [fontSize, setFontSize] = useState(loadFontSize);
  // Snapshot of font values when Settings was opened, for cancel/restore.
  const savedFontRef = useRef({ family: fontFamily, size: fontSize });

  const waitingCount = mgr.sessions.filter((s) => s.state === "WAITING").length;
  const activeSession = mgr.sessions.find((s) => s.id === mgr.activeSessionId) ?? null;

  // Keep the latest active session id available to the (once-registered) native
  // drag-drop listener without re-subscribing on every tab switch.
  const activeSessionIdRef = useRef(mgr.activeSessionId);
  activeSessionIdRef.current = mgr.activeSessionId;

  // App-wide file drag-and-drop. Uses Tauri's native drag-drop event, which hands
  // us the dropped POSIX paths directly from the pasteboard — without WKWebView ever
  // opening the files. The old HTML5 dataTransfer.files approach read each File
  // object, forcing WKWebView to materialize media-library-backed items and firing
  // an endless cascade of macOS Photos/Music/Finder privacy prompts. Paths are
  // written into whichever terminal tab is currently active.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const id = activeSessionIdRef.current;
        if (!id) return;
        const paths = event.payload.paths;
        if (!paths || paths.length === 0) return;
        void writeStdin(id, paths.map(shellEscapePath).join(" "));
      })
      .then((u) => {
        if (disposed) u();
        else unlisten = u;
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Sync --zoom-factor CSS variable whenever font size changes so all rem-based
  // UI text (dashboard, session cards, tabs, etc.) scales proportionally.
  useEffect(() => {
    const factor = fontSize / DEFAULT_FONT_SIZE;
    document.documentElement.style.setProperty("--zoom-factor", String(factor));
  }, [fontSize]);

  // ⌘N opens a fresh terminal; ⌘T opens one in the active terminal's folder.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey || e.altKey || e.ctrlKey) return;
      const k = e.key.toLowerCase();
      if (k === "n") {
        e.preventDefault();
        void mgr.createSession();
      } else if (k === "t") {
        e.preventDefault();
        void mgr.createSession(activeSession?.cwd ?? undefined);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mgr, activeSession]);

  const handleZoom = useCallback((size: number) => {
    setFontSize(size);
    persistFont(fontFamily, size);
    savedFontRef.current = { family: fontFamily, size };
  }, [fontFamily]);

  // App-wide zoom: Cmd+= / Cmd+- / Cmd+0 on macOS, Ctrl variants on Linux.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((!e.metaKey && !e.ctrlKey) || e.altKey) return;
      if (e.key === "=" || e.key === "+") {
        e.preventDefault();
        handleZoom(Math.min(32, fontSize + 1));
        return;
      }
      if (e.key === "-") {
        e.preventDefault();
        handleZoom(Math.max(8, fontSize - 1));
        return;
      }
      if (e.key === "0") {
        e.preventDefault();
        handleZoom(DEFAULT_FONT_SIZE);
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fontSize, handleZoom]);

  const openSettings = useCallback(() => {
    savedFontRef.current = { family: fontFamily, size: fontSize };
    setShowSettings(true);
  }, [fontFamily, fontSize]);

  const closeSettings = useCallback(() => {
    setShowSettings(false);
  }, []);

  const saveSettings = useCallback((family: string, size: number) => {
    setFontFamily(family);
    setFontSize(size);
    savedFontRef.current = { family, size };
    persistFont(family, size);
    setShowSettings(false);
  }, []);

  const handleFontChange = useCallback((family: string, size: number) => {
    setFontFamily(family);
    setFontSize(size);
    persistFont(family, size);
    savedFontRef.current = { family, size };
  }, []);

  const topbarActions = (
    <div className="topbar-actions">
      <button className="tb-btn" onClick={() => setShowSummary(true)}>
        Today
      </button>
      <button className="tb-btn" onClick={openSettings}>
        Settings
      </button>
      <button
        className={`tb-btn ${grouped ? "tb-btn--on" : ""}`}
        onClick={toggleGrouped}
        title="Group tabs by folder"
      >
        Tab Group
      </button>
      <button
        className={`tb-btn ${showDashboard ? "tb-btn--on" : ""}`}
        onClick={() => setShowDashboard((v) => !v)}
      >
        Dashboard
        {waitingCount > 0 && <span className="tb-badge">{waitingCount}</span>}
      </button>
    </div>
  );

  return (
    <div className="app">
      <UpdateBanner />
      {grouped ? (
        // Grouped mode: the group tags share one row with the action buttons.
        <TabGroups
          sessions={mgr.sessions}
          activeSessionId={mgr.activeSessionId}
          onAdd={mgr.createSession}
          onClose={mgr.closeSession}
          onSwitch={mgr.switchSession}
          onRename={mgr.renameSession}
          actions={topbarActions}
        />
      ) : (
        <header className="topbar">
          <TabStrip
            sessions={mgr.sessions}
            activeSessionId={mgr.activeSessionId}
            onAdd={mgr.createSession}
            onClose={mgr.closeSession}
            onSwitch={mgr.switchSession}
            onRename={mgr.renameSession}
          />
          {topbarActions}
        </header>
      )}

      <div className="body">
        <main className="workspace">
          {mgr.sessions.length === 0 && (
            <div className="empty-state">
              No terminals yet — press <kbd>+</kbd> to open one.
            </div>
          )}
          {mgr.sessions.map((s) => (
            <TerminalView
              key={s.id}
              sessionId={s.id}
              visible={s.id === mgr.activeSessionId}
              state={s.state}
              cwd={s.cwd}
              project={s.project}
              createdAt={s.createdAt}
              fontFamily={fontFamily}
              fontSize={fontSize}
            />
          ))}
        </main>
        {showGit && (
          <GitPanel
            cwd={activeSession?.cwd ?? null}
            project={activeSession?.project ?? null}
            onClose={() => setShowGit(false)}
          />
        )}
        {showDashboard && (
          <Dashboard
            sessions={mgr.sessions}
            activeSessionId={mgr.activeSessionId}
            onFocus={mgr.switchSession}
            onSetLabel={mgr.setTaskLabel}
            onOpenToday={() => setShowSummary(true)}
          />
        )}
      </div>

      <StatusBar
        cwd={activeSession?.cwd ?? null}
        project={activeSession?.project ?? null}
        gitOpen={showGit}
        onToggleGit={() => setShowGit((v) => !v)}
      />

      {mgr.capDialog && (
        <CapDialog
          capDialog={mgr.capDialog}
          sessions={mgr.sessions}
          onRaise={mgr.raiseCap}
          onCloseSession={mgr.closeFromDialog}
          onDismiss={mgr.dismissDialog}
        />
      )}
      {showSummary && <DailySummary onClose={() => setShowSummary(false)} />}
      {showPermsSetup && (
        <div className="dialog-overlay" onClick={dismissPermsSetup}>
          <div className="dialog dialog--wide" onClick={(e) => e.stopPropagation()}>
            <h2 className="dialog-title">One-time file access setup</h2>
            <PermissionsSetup />
            <div className="dialog-actions">
              <button className="btn" onClick={dismissPermsSetup}>
                Later
              </button>
              <button className="btn btn-primary" onClick={dismissPermsSetup}>
                Done
              </button>
            </div>
          </div>
        </div>
      )}
      {showSettings && (
        <Settings
          cap={mgr.cap}
          fontFamily={fontFamily}
          fontSize={fontSize}
          onApplyCap={mgr.applyCap}
          onFontChange={handleFontChange}
          onSave={saveSettings}
          onClose={closeSettings}
        />
      )}
    </div>
  );
}

export default App;
