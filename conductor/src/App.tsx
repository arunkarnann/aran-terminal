import { useCallback, useEffect, useRef, useState } from "react";
import { CapDialog } from "./components/CapDialog";
import { Dashboard } from "./components/Dashboard";
import { DailySummary } from "./components/DailySummary";
import { GitPanel } from "./components/GitPanel";
import { Settings } from "./components/Settings";
import { StatusBar } from "./components/StatusBar";
import { TabGroups } from "./components/TabGroups";
import { TabStrip } from "./components/TabStrip";
import { TerminalView } from "./components/TerminalView";
import { useSessionManager } from "./stores/useSessionManager";
import "./App.css";

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

  const openSettings = useCallback(() => {
    savedFontRef.current = { family: fontFamily, size: fontSize };
    setShowSettings(true);
  }, [fontFamily, fontSize]);

  const closeSettings = useCallback(() => {
    // Font changes apply-and-persist instantly (below), so there's nothing to
    // revert on close — closing just dismisses the dialog.
    setShowSettings(false);
  }, []);

  const saveSettings = useCallback((family: string, size: number) => {
    setFontFamily(family);
    setFontSize(size);
    savedFontRef.current = { family, size };
    persistFont(family, size);
    setShowSettings(false);
  }, []);

  // Font edits behave like a text editor: applied to the terminal *and* persisted
  // the instant they change, with no Save/Cancel dance. Mirrors handleZoom.
  const handleFontChange = useCallback((family: string, size: number) => {
    setFontFamily(family);
    setFontSize(size);
    persistFont(family, size);
    savedFontRef.current = { family, size };
  }, []);

  const handleZoom = useCallback((size: number) => {
    setFontSize(size);
    persistFont(fontFamily, size);
    savedFontRef.current = { family: fontFamily, size };
  }, [fontFamily]);

  return (
    <div className="app">
      <header className="topbar">
        {grouped ? (
          <div className="topbar-spacer" />
        ) : (
          <TabStrip
            sessions={mgr.sessions}
            activeSessionId={mgr.activeSessionId}
            onAdd={mgr.createSession}
            onClose={mgr.closeSession}
            onSwitch={mgr.switchSession}
            onRename={mgr.renameSession}
          />
        )}

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
            Group
          </button>
          <button
            className={`tb-btn ${showDashboard ? "tb-btn--on" : ""}`}
            onClick={() => setShowDashboard((v) => !v)}
          >
            Dashboard
            {waitingCount > 0 && <span className="tb-badge">{waitingCount}</span>}
          </button>
        </div>
      </header>

      {grouped && (
        <TabGroups
          sessions={mgr.sessions}
          activeSessionId={mgr.activeSessionId}
          onAdd={mgr.createSession}
          onClose={mgr.closeSession}
          onSwitch={mgr.switchSession}
          onRename={mgr.renameSession}
        />
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
              onFontSizeChange={handleZoom}
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
