import { useCallback, useEffect, useRef, useState } from "react";
import {
  closeSession as apiCloseSession,
  closeSessionSnapshot,
  createSession as apiCreateSession,
  deleteSessionSnapshot,
  listSessions,
  loadSessionSnapshots,
  onCapReached,
  onSessionClosed,
  onSessionState,
  renameSession as apiRenameSession,
  setSessionCap as apiSetSessionCap,
  setTaskLabel as apiSetTaskLabel,
} from "../ipc/api";
import type { CapReachedEvent, SessionId, SessionMeta } from "../ipc/types";

/** Given the session list before a close, choose which session to activate next. */
function pickNextSession(
  allSessions: UiSession[],
  closedId: SessionId,
  remaining: UiSession[],
): SessionId | null {
  if (remaining.length === 0) return null;

  const closedIdx = allSessions.findIndex((s) => s.id === closedId);
  if (closedIdx < 0) return remaining[0]?.id ?? null;

  const closedSession = allSessions[closedIdx];

  // 1. Prefer a sibling in the same project group (next, then previous, then first).
  if (closedSession?.project) {
    const sameProject = remaining.filter((s) => s.project === closedSession.project);
    if (sameProject.length > 0) {
      const projectSessions = allSessions.filter(
        (s) => s.project === closedSession.project,
      );
      const projectIdx = projectSessions.findIndex((s) => s.id === closedId);
      const nextInProject = projectSessions[projectIdx + 1]
        ?? projectSessions[Math.max(0, projectIdx - 1)];
      if (nextInProject && remaining.some((s) => s.id === nextInProject.id)) {
        return nextInProject.id;
      }
      return sameProject[0].id;
    }
  }

  // 2. Fallback: the session at the same position, or the last, or the first.
  const nextIdx = Math.min(closedIdx, remaining.length - 1);
  return (remaining[nextIdx] ?? remaining[remaining.length - 1] ?? remaining[0])?.id ?? null;
}

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
  const sessionsRef = useRef(sessions);
  /** SessionId → serialized xterm.js scrollback (base64) for restore. */
  const [restoreScrollbacks, setRestoreScrollbacks] = useState<
    Map<SessionId, string>
  >(new Map());

  useEffect(() => {
    sessionsRef.current = sessions;
  }, [sessions]);

  // Backend event subscriptions.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    (async () => {
      const subs = await Promise.all([
        onCapReached((e: CapReachedEvent) => {
          setCapDialog({ limit: e.limit, current: e.current });
        }),
        onSessionClosed((e) => {
          const prevSessions = sessionsRef.current;
          const remaining = prevSessions.filter((s) => s.id !== e.id);
          setSessions(remaining);
          setActiveSessionId((prev) => {
            if (prev !== e.id) return prev;
            return pickNextSession(prevSessions, e.id, remaining);
          });
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
                // Keep the optimistic project/cwd until the backend has a real
                // value (OSC 7). Otherwise the poll clobbers them with null and
                // the tab briefly falls into the "Loose" group, then jumps back.
                project: b.project ?? s.project,
                cwd: b.cwd ?? s.cwd,
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

  // On first mount: try to restore the previous session, falling back to a
  // single fresh terminal. Skip if the backend already has live sessions
  // (dev hot-reload) so we don't spawn duplicates.
  const autoOpenedRef = useRef(false);
  useEffect(() => {
    if (autoOpenedRef.current) return;
    autoOpenedRef.current = true;
    (async () => {
      try {
        const existing = await listSessions();
        if (existing.length > 0) return; // hot-reload: sessions already live

        // Check for persisted session snapshots.
        const snaps = await loadSessionSnapshots();
        if (snaps.length > 0) {
          // Check restore-on-launch setting.
          let restore = true;
          try {
            restore = localStorage.getItem("conductor-restore-on-launch") !== "0";
          } catch {
            /* default on */
          }
          if (restore) {
            // Create sessions through the store so frontend state updates
            // immediately. Track creation order so we can map each snapshot's
            // scrollback to its newly-created session reliably.
            const scrollbacks = new Map<SessionId, string>();
            const prevCount = sessionsRef.current.length;
            for (const s of snaps) {
              await createSession(s.snapshot.cwd ?? undefined);
              await new Promise((r) => setTimeout(r, 50));
            }
            // The restored sessions have fresh ids; the periodic auto-save and
            // on-close flush persist them under those ids. Drop the consumed
            // snapshots so the next launch doesn't restore them a second time.
            for (const s of snaps) {
              try { await deleteSessionSnapshot(s.snapshot.sessionId); } catch { /* non-critical */ }
            }
            // Map snapshot scrollbacks to new sessions by creation order:
            // snaps[0] → sessions[prevCount], snaps[1] → sessions[prevCount+1], etc.
            const mapScrollbacks = () => {
              const current = sessionsRef.current;
              for (let i = 0; i < snaps.length; i++) {
                if (!snaps[i].scrollbackBase64) continue;
                const session = current[prevCount + i];
                if (session) scrollbacks.set(session.id, snaps[i].scrollbackBase64!);
              }
              if (scrollbacks.size > 0) setRestoreScrollbacks(new Map(scrollbacks));
            };
            // Try immediately, then after the first poll cycle as a fallback.
            setTimeout(mapScrollbacks, 100);
            setTimeout(mapScrollbacks, 2200);
            return;
          }
        }
        // No snapshots or restore disabled: create one fresh terminal.
        void createSession();
      } catch {
        void createSession();
      }
    })();
  }, [createSession]);

  const closeSession = useCallback(async (id: SessionId) => {
    await apiCloseSession(id);
    try { await closeSessionSnapshot(id); } catch { /* non-critical */ }
    setRestoreScrollbacks((prev) => {
      const next = new Map(prev);
      next.delete(id);
      return next;
    });
    const prevSessions = sessionsRef.current;
    const remaining = prevSessions.filter((s) => s.id !== id);
    setSessions(remaining);
    setActiveSessionId((prev) => {
      if (prev !== id) return prev;
      return pickNextSession(prevSessions, id, remaining);
    });
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
    restoreScrollbacks,
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
