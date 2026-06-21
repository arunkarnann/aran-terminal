// Warp-style block UI rendered as an overlay on top of an xterm terminal.
//
// We do NOT own the renderer (xterm draws the text grid), so block affordances
// live in an overlay computed from live `IMarker` line positions — the same
// marker-to-pixel trick the autocomplete ghost uses (see TerminalView.cursorPx).
//
// Mouse discipline: the only pointer-events:auto elements are the thin left
// gutter bars and the hover toolbar. Everything else is pointer-events:none, so
// terminal text selection / clicks are never intercepted. Block creation and all
// overlays are suppressed while a full-screen TUI owns the alternate screen.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Terminal } from "@xterm/xterm";
import type { BlockStatus, BlockTracker, CommandBlock } from "../lib/blocks";

interface BlockLayerProps {
  term: Terminal;
  tracker: BlockTracker;
  host: HTMLDivElement;
  visible: boolean;
  sendInput: (data: string) => void;
}

interface BlockRect {
  id: number;
  status: BlockStatus;
  command: string | null;
  yTop: number;
  height: number;
  promptLine: number;
}

interface StickyInfo {
  id: number;
  command: string;
  status: BlockStatus;
  promptLine: number;
}

type CopyKind = "cmd" | "out" | "all";

const STATUS_LABEL: Record<BlockStatus, string> = {
  active: "prompt",
  running: "running",
  success: "exit 0",
  error: "failed",
  aborted: "aborted",
};

function metrics(term: Terminal, host: HTMLElement) {
  const screen = host.querySelector(".xterm-screen") as HTMLElement | null;
  if (!screen || !term.rows) return null;
  const cellH = screen.clientHeight / term.rows;
  if (!cellH) return null;
  const h = host.getBoundingClientRect();
  const s = screen.getBoundingClientRect();
  return {
    cellH,
    screenTop: s.top - h.top,
    screenLeft: s.left - h.left,
    screenWidth: screen.clientWidth,
  };
}

export function BlockLayer({ term, tracker, host, visible, sendInput }: BlockLayerProps) {
  const [rects, setRects] = useState<BlockRect[]>([]);
  const [sticky, setSticky] = useState<StickyInfo | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [hoveredId, setHoveredId] = useState<number | null>(null);
  const [copied, setCopied] = useState<{ id: number; kind: CopyKind } | null>(null);

  const rafRef = useRef<number | null>(null);
  const sigRef = useRef("");
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const compute = useCallback(() => {
    const m = metrics(term, host);
    if (!m || term.buffer.active.type === "alternate") {
      if (sigRef.current !== "EMPTY") {
        sigRef.current = "EMPTY";
        setRects([]);
        setSticky(null);
      }
      return;
    }
    const buf = term.buffer.active;
    const viewportY = buf.viewportY;
    const rows = term.rows;
    const lastLine = buf.length - 1;
    const blocks = tracker.list().filter((b) => !b.promptMarker.isDisposed);

    const out: BlockRect[] = [];
    let stickyCand: CommandBlock | null = null;
    for (let i = 0; i < blocks.length; i++) {
      const b = blocks[i];
      const top = b.promptMarker.line;
      const bottom =
        i + 1 < blocks.length ? blocks[i + 1].promptMarker.line - 1 : lastLine;
      if (bottom < top) continue;
      if (top <= viewportY && bottom >= viewportY) stickyCand = b;

      const visTop = Math.max(top, viewportY);
      const visBottom = Math.min(bottom, viewportY + rows - 1);
      if (visBottom < visTop) continue; // off-screen

      out.push({
        id: b.id,
        status: b.status,
        command: b.command,
        yTop: m.screenTop + (visTop - viewportY) * m.cellH,
        height: (visBottom - visTop + 1) * m.cellH,
        promptLine: top,
      });
    }

    const stickyInfo: StickyInfo | null =
      stickyCand &&
      stickyCand.command &&
      stickyCand.promptMarker.line < viewportY
        ? {
            id: stickyCand.id,
            command: stickyCand.command,
            status: stickyCand.status,
            promptLine: stickyCand.promptMarker.line,
          }
        : null;

    // Skip React work when nothing visible changed (output frames are frequent).
    const sig = JSON.stringify({ out, stickyInfo });
    if (sig === sigRef.current) return;
    sigRef.current = sig;
    setRects(out);
    setSticky(stickyInfo);
  }, [term, tracker, host]);

  const schedule = useCallback(() => {
    if (rafRef.current != null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      try {
        compute();
      } catch {
        /* overlay math must never break the terminal */
      }
    });
  }, [compute]);

  // Recompute on every source of geometry change.
  useEffect(() => {
    const unsubTracker = tracker.subscribe(schedule);
    const d1 = term.onRender(schedule);
    const d2 = term.onScroll(schedule);
    const d3 = term.onResize(schedule);
    const onWin = () => schedule();
    window.addEventListener("resize", onWin);
    schedule();
    return () => {
      unsubTracker();
      d1.dispose();
      d2.dispose();
      d3.dispose();
      window.removeEventListener("resize", onWin);
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
    };
  }, [term, tracker, schedule]);

  useEffect(() => {
    if (visible) schedule();
  }, [visible, schedule]);

  // Keyboard: ⌘↑ / ⌘↓ jump between blocks (only for the visible terminal).
  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
      const blocks = tracker.list().filter((b) => !b.promptMarker.isDisposed);
      if (!blocks.length) return;
      const ref = term.buffer.active.viewportY;
      let target: CommandBlock | undefined;
      if (e.key === "ArrowUp") {
        target = [...blocks].reverse().find((b) => b.promptMarker.line < ref);
      } else {
        target = blocks.find((b) => b.promptMarker.line > ref);
      }
      if (!target) return;
      e.preventDefault();
      term.scrollToLine(target.promptMarker.line);
      setSelectedId(target.id);
      schedule();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, term, tracker, schedule]);

  const flashCopied = useCallback((id: number, kind: CopyKind) => {
    setCopied({ id, kind });
    if (copyTimer.current) clearTimeout(copyTimer.current);
    copyTimer.current = setTimeout(() => setCopied(null), 1100);
  }, []);

  const doCopy = useCallback(
    (id: number, kind: CopyKind) => {
      const b = tracker.list().find((x) => x.id === id);
      if (!b) return;
      const text =
        kind === "cmd"
          ? tracker.commandText(b)
          : kind === "out"
            ? tracker.outputText(b)
            : tracker.fullText(b);
      void navigator.clipboard?.writeText(text).then(
        () => flashCopied(id, kind),
        () => {},
      );
    },
    [tracker, flashCopied],
  );

  const doRerun = useCallback(
    (id: number) => {
      const b = tracker.list().find((x) => x.id === id);
      const cmd = b?.command?.trim();
      if (!cmd) return;
      sendInput(`${cmd}\r`);
    },
    [tracker, sendInput],
  );

  useEffect(
    () => () => {
      if (copyTimer.current) clearTimeout(copyTimer.current);
    },
    [],
  );

  if (!visible) return null;

  return (
    <div className="blk-layer" aria-hidden>
      {rects.map((r) => {
        const isHover = hoveredId === r.id;
        const isSel = selectedId === r.id;
        return (
          <div key={r.id} className="blk-anchor" style={{ top: r.yTop, height: r.height }}>
            {/* Boundary line at the top of the block. */}
            <div className="blk-sep" />
            {/* Selection / hover background highlight (non-interactive). */}
            {(isSel || isHover) && (
              <div className={`blk-highlight ${isSel ? "blk-highlight--sel" : ""}`} />
            )}
            {/* Left gutter bar — the only always-on interactive surface. */}
            <button
              type="button"
              className={`blk-bar blk-bar--${r.status} ${isSel ? "blk-bar--sel" : ""}`}
              title={r.command ? `${r.command} · ${STATUS_LABEL[r.status]}` : STATUS_LABEL[r.status]}
              onMouseEnter={() => setHoveredId(r.id)}
              onMouseLeave={() => setHoveredId((h) => (h === r.id ? null : h))}
              onClick={() =>
                setSelectedId((s) => (s === r.id ? null : r.id))
              }
            />
            {/* Hover toolbar, anchored top-right of the block. */}
            {isHover && (
              <div
                className="blk-toolbar"
                onMouseEnter={() => setHoveredId(r.id)}
                onMouseLeave={() => setHoveredId((h) => (h === r.id ? null : h))}
              >
                <span className={`blk-dot blk-dot--${r.status}`} />
                <button
                  type="button"
                  className="blk-btn"
                  disabled={!r.command}
                  onClick={() => doCopy(r.id, "cmd")}
                >
                  {copied?.id === r.id && copied.kind === "cmd" ? "✓ cmd" : "copy cmd"}
                </button>
                <button type="button" className="blk-btn" onClick={() => doCopy(r.id, "out")}>
                  {copied?.id === r.id && copied.kind === "out" ? "✓ out" : "copy output"}
                </button>
                <button type="button" className="blk-btn" onClick={() => doCopy(r.id, "all")}>
                  {copied?.id === r.id && copied.kind === "all" ? "✓ all" : "copy all"}
                </button>
                <button
                  type="button"
                  className="blk-btn blk-btn--accent"
                  disabled={!r.command}
                  onClick={() => doRerun(r.id)}
                >
                  re-run
                </button>
              </div>
            )}
          </div>
        );
      })}

      {/* Sticky command header: shows the command you're scrolled inside. */}
      {sticky && (
        <button
          type="button"
          className={`blk-sticky blk-sticky--${sticky.status}`}
          onClick={() => {
            term.scrollToLine(sticky.promptLine);
            setSelectedId(sticky.id);
            schedule();
          }}
          title="Jump to command"
        >
          <span className={`blk-dot blk-dot--${sticky.status}`} />
          <span className="blk-sticky-cmd">{sticky.command}</span>
          <span className="blk-sticky-hint">{STATUS_LABEL[sticky.status]}</span>
        </button>
      )}
    </div>
  );
}
