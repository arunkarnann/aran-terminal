import { useCallback, useEffect, useRef, useState } from "react";
import {
  abandonFocusBlock,
  completeFocusBlock,
  extendFocusBlock,
  getActiveFocusBlock,
  getFocusDay,
  setFocusGoal as apiSetFocusGoal,
  startFocusBlock,
} from "../ipc/api";
import type { FocusBlock, FocusDay, SessionId } from "../ipc/types";
import { startOfDay } from "../lib/ui";
import type { UiSession } from "./useSessionManager";

/** What just finished, so the view can show the right after-block prompt. */
export interface CompletedBlock {
  kind: FocusBlock["kind"];
  plannedMs: number;
  sessionId: SessionId | null;
  taskLabel: string | null;
}

export interface StartOpts {
  sessionId: SessionId | null;
  plannedMs: number;
  taskLabel?: string | null;
  kind?: FocusBlock["kind"];
}

/**
 * Focus View state. Every timer here is timestamp-derived: `now - startedAt` and
 * `startedAt + plannedMs - now`. The interval only forces a re-render; the backend
 * owns the anchor (`startedAt`), so the clock is drift-free and survives reload.
 */
export function useFocus(sessions: UiSession[]) {
  const [activeBlock, setActiveBlock] = useState<FocusBlock | null>(null);
  const [day, setDay] = useState<FocusDay | null>(null);
  const [now, setNow] = useState(() => Date.now());
  /** Set when a focus block elapses; drives the break prompt + chime. Null otherwise. */
  const [completed, setCompleted] = useState<CompletedBlock | null>(null);

  // Guards against double-firing completion while the async settle is in flight.
  const settlingRef = useRef(false);

  const refreshDay = useCallback(async () => {
    try {
      setDay(await getFocusDay(startOfDay(Date.now())));
    } catch {
      /* DB unavailable — leave the last snapshot. */
    }
  }, []);

  // Rehydrate on mount: an active block survives a reload. But a block that already
  // elapsed must have run down while the app was closed (an open app would have completed
  // it live) — don't credit that as focus. Abandon it instead of letting the completion
  // effect stamp a late `ended_at` and award a phantom block + streak day.
  useEffect(() => {
    (async () => {
      try {
        const block = await getActiveFocusBlock();
        if (block && Date.now() >= block.startedAt + block.plannedMs) {
          try {
            await abandonFocusBlock(block.id);
          } catch {
            /* ignore */
          }
        } else {
          setActiveBlock(block);
        }
      } catch {
        /* ignore */
      }
      void refreshDay();
    })();
  }, [refreshDay]);

  // Re-derive `now` once a second so every clock ticks.
  useEffect(() => {
    const iv = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(iv);
  }, []);

  // Webviews throttle backgrounded timers, so a block can elapse while hidden. Catch up
  // the instant we're visible/focused again — this is what fires the chime on return.
  useEffect(() => {
    const wake = () => setNow(Date.now());
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("focus", wake);
    return () => {
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("focus", wake);
    };
  }, []);

  // Completion check — runs on every `now` change (tick or wake).
  useEffect(() => {
    if (!activeBlock || settlingRef.current) return;
    const end = activeBlock.startedAt + activeBlock.plannedMs;
    if (now < end) return;

    settlingRef.current = true;
    const finished = activeBlock;
    (async () => {
      try {
        await completeFocusBlock(finished.id);
      } catch {
        /* already ended elsewhere */
      }
      setActiveBlock(null);
      // A focus block that lands earns a break prompt + chime; a break just clears.
      if (finished.kind === "focus") {
        playChime();
        setCompleted({
          kind: finished.kind,
          plannedMs: finished.plannedMs,
          sessionId: finished.sessionId,
          taskLabel: finished.taskLabel,
        });
      }
      await refreshDay();
      settlingRef.current = false;
    })();
  }, [now, activeBlock, refreshDay]);

  // Edge case: the focused session is closed mid-block — abandon (don't leave the timer
  // pointing at a dead session).
  useEffect(() => {
    if (!activeBlock || activeBlock.kind !== "focus" || !activeBlock.sessionId) return;
    if (settlingRef.current) return;
    const alive = sessions.some((s) => s.id === activeBlock.sessionId);
    if (alive) return;
    settlingRef.current = true;
    const id = activeBlock.id;
    (async () => {
      try {
        await abandonFocusBlock(id);
      } catch {
        /* ignore */
      }
      setActiveBlock(null);
      await refreshDay();
      settlingRef.current = false;
    })();
  }, [sessions, activeBlock, refreshDay]);

  const start = useCallback(
    async (opts: StartOpts) => {
      try {
        const block = await startFocusBlock({
          sessionId: opts.sessionId,
          plannedMs: opts.plannedMs,
          taskLabel: opts.taskLabel ?? null,
          kind: opts.kind ?? "focus",
        });
        setCompleted(null);
        setActiveBlock(block);
      } catch {
        /* leave state as-is */
      }
    },
    [],
  );

  // Finish early. `complete=true` counts it as a landed block; otherwise it's abandoned.
  const stop = useCallback(
    async (complete: boolean) => {
      if (!activeBlock) return;
      const b = activeBlock;
      setActiveBlock(null);
      try {
        await (complete ? completeFocusBlock(b.id) : abandonFocusBlock(b.id));
      } catch {
        /* ignore */
      }
      await refreshDay();
    },
    [activeBlock, refreshDay],
  );

  const extend = useCallback(
    async (addMs: number) => {
      if (!activeBlock) return;
      setActiveBlock((b) => (b ? { ...b, plannedMs: b.plannedMs + addMs } : b));
      try {
        await extendFocusBlock(activeBlock.id, addMs);
      } catch {
        /* optimistic value stands */
      }
    },
    [activeBlock],
  );

  const dismissCompleted = useCallback(() => setCompleted(null), []);

  const setGoal = useCallback(
    async (ms: number) => {
      try {
        await apiSetFocusGoal(ms);
        await refreshDay();
      } catch {
        /* ignore */
      }
    },
    [refreshDay],
  );

  const remainingMs = activeBlock
    ? Math.max(0, activeBlock.startedAt + activeBlock.plannedMs - now)
    : 0;

  return {
    activeBlock,
    day,
    now,
    remainingMs,
    completed,
    start,
    stop,
    extend,
    setGoal,
    dismissCompleted,
  };
}

export type FocusController = ReturnType<typeof useFocus>;

/** A short two-tone chime via Web Audio. Silent-fails if audio is unavailable. */
function playChime() {
  try {
    const Ctx =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext: typeof AudioContext })
        .webkitAudioContext;
    const ctx = new Ctx();
    const notes = [660, 880];
    notes.forEach((freq, i) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.value = freq;
      const t = ctx.currentTime + i * 0.18;
      gain.gain.setValueAtTime(0.0001, t);
      gain.gain.exponentialRampToValueAtTime(0.18, t + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.35);
      osc.connect(gain).connect(ctx.destination);
      osc.start(t);
      osc.stop(t + 0.4);
    });
    setTimeout(() => void ctx.close().catch(() => {}), 1200);
  } catch {
    /* no audio — the visual prompt is enough */
  }
}
