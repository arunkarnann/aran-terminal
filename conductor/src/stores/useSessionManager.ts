import { useCallback, useEffect, useRef, useState } from "react";
import {
  closeSession as apiCloseSession,
  createSession as apiCreateSession,
  listSessions,
  onCapReached,
  onSessionClosed,
  onSessionState,
  renameSession as apiRenameSession,
  setSessionCap as apiSetSessionCap,
  setTaskLabel as apiSetTaskLabel,
} from "../ipc/api";
import type { CapReachedEvent, SessionId, SessionMeta } from "../ipc/types";

export interface CapDialogState {
  limit: number;
  current: number;
}

/** Session plus frontend-only fields the backend doesn't track. */
export interface UiSession extends SessionMeta {
  lastActivityAt: number;
  /** Epoch ms when this session entered WAITING (null otherwise) — drives the wait clock. */
  waitingSince: number | null;
}

/** Preserve `waitingSince` across a state change: set on entry, cleared on exit. */
function nextWaitingSince(prev: UiSession, nextState: SessionMeta["state"], at: number) {
  if (nextState !== "WAITING") return null;
  return prev.state === "WAITING" && prev.waitingSince ? prev.waitingSince : at;
}

export function useSessionManager() {
  const [sessions, setSessions] = useState<UiSession[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<SessionId | null>(null);
  const [cap, setCap] = useState(5);
  const [capDialog, setCapDialog] = useState<CapDialogState | null>(null);
  const pendingCreateRef = useRef(false);

  // Backend event subscriptions.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    (async () => {
      const subs = await Promise.all([
        onCapReached((e: CapReachedEvent) => {
          setCapDialog({ limit: e.limit, current: e.current });
        }),
        onSessionClosed((e) => {
          setSessions((prev) => prev.filter((s) => s.id !== e.id));
          setActiveSessionId((prev) => (prev === e.id ? null : prev));
        }),
        onSessionState((e) => {
          setSessions((prev) =>
            prev.map((s) =>
              s.id === e.id
                ? {
                    ...s,
                    state: e.state,
                    lastActivityAt: e.at,
                    waitingSince: nextWaitingSince(s, e.state, e.at),
                  }
                : s,
            ),
          );
        }),
      ]);
      unlisteners.push(...subs);
    })();
    return () => unlisteners.forEach((u) => u());
  }, []);

  // Poll backend metadata (project / cwd / command count / task label) for known sessions.
  useEffect(() => {
    const iv = setInterval(async () => {
      let list: SessionMeta[];
      try {
        list = await listSessions();
      } catch {
        return;
      }
      const byId = new Map(list.map((s) => [s.id, s]));
      setSessions((prev) =>
        prev.map((s) => {
          const b = byId.get(s.id);
          return b
            ? {
                ...s,
                name: b.name ?? s.name,
                project: b.project,
                cwd: b.cwd,
                taskLabel: b.taskLabel,
                commandCount: b.commandCount,
                state: b.state,
                rssKb: b.rssKb,
                lastCommand: b.lastCommand,
                waitingSince: nextWaitingSince(s, b.state, s.lastActivityAt),
              }
            : s;
        }),
      );
    }, 2000);
    return () => clearInterval(iv);
  }, []);

  const createSession = useCallback(async (cwd?: string) => {
    if (pendingCreateRef.current) return;
    pendingCreateRef.current = true;
    try {
      const id = await apiCreateSession(cwd ? { cwd } : undefined);
      const now = Date.now();
      // Optimistically derive the project from the spawn folder so the new tab
      // joins the right group immediately (the shell confirms it via OSC 7 soon).
      const project = cwd ? cwd.split("/").filter(Boolean).pop() ?? null : null;
      const newSession: UiSession = {
        id,
        name: null,
        project,
        cwd: cwd ?? null,
        taskLabel: null,
        shell: "",
        createdAt: now,
        openCount: 0,
        commandCount: 0,
        state: "RUNNING",
        rssKb: null,
        lastCommand: null,
        lastActivityAt: now,
        waitingSince: null,
      };
      setSessions((prev) => [...prev, newSession]);
      setActiveSessionId(id);
    } catch {
      // The cap://reached event listener will show the dialog.
    } finally {
      pendingCreateRef.current = false;
    }
  }, []);

  // Always open one terminal by default on launch (skip if the backend already
  // has sessions, e.g. a dev hot-reload, so we don't spawn duplicates).
  const autoOpenedRef = useRef(false);
  useEffect(() => {
    if (autoOpenedRef.current) return;
    autoOpenedRef.current = true;
    (async () => {
      try {
        const existing = await listSessions();
        if (existing.length === 0) void createSession();
      } catch {
        void createSession();
      }
    })();
  }, [createSession]);

  const closeSession = useCallback(async (id: SessionId) => {
    await apiCloseSession(id);
    setSessions((prev) => prev.filter((s) => s.id !== id));
    setActiveSessionId((prev) => (prev === id ? null : prev));
  }, []);

  const switchSession = useCallback((id: SessionId) => {
    setActiveSessionId(id);
  }, []);

  const renameSession = useCallback(async (id: SessionId, name: string) => {
    await apiRenameSession(id, name);
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, name } : s)));
  }, []);

  const setTaskLabel = useCallback(async (id: SessionId, label: string) => {
    await apiSetTaskLabel(id, label);
    setSessions((prev) =>
      prev.map((s) => (s.id === id ? { ...s, taskLabel: label } : s)),
    );
  }, []);

  const applyCap = useCallback(async (newCap: number) => {
    await apiSetSessionCap(newCap);
    setCap(newCap);
  }, []);

  const raiseCap = useCallback(async () => {
    await applyCap(cap + 1);
    setCapDialog(null);
    void createSession();
  }, [applyCap, cap, createSession]);

  const closeFromDialog = useCallback(
    async (id: SessionId) => {
      await closeSession(id);
      setCapDialog(null);
      void createSession();
    },
    [closeSession, createSession],
  );

  const dismissDialog = useCallback(() => setCapDialog(null), []);

  return {
    sessions,
    activeSessionId,
    cap,
    capDialog,
    createSession,
    closeSession,
    switchSession,
    renameSession,
    setTaskLabel,
    applyCap,
    raiseCap,
    closeFromDialog,
    dismissDialog,
  };
}
