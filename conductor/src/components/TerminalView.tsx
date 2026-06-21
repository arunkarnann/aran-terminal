import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { fontProbe, primaryFamily } from "../lib/fonts";
import { ageTier } from "../lib/ui";
import { useNow } from "../lib/useNow";
import { getProjectColor, useProjectColors } from "../lib/projectColors";
import "@xterm/xterm/css/xterm.css";
import {
  decodePtyBytes,
  devLog,
  getCommandSuggestions,
  onPtyOutput,
  onSessionClosed,
  resizePty,
  writeStdin,
} from "../ipc/api";
import { useTheme } from "../themes/useTheme";
import { toXtermTheme } from "../themes";
import { BlockTracker } from "../lib/blocks";
import { BlockLayer } from "./BlockLayer";
import { CommandHelp } from "./CommandHelp";
import type { AttentionState, SessionId } from "../ipc/types";

const MAX_SUGGESTIONS = 6;

const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 32;
const ZOOM_STEP = 1;

interface TerminalViewProps {
  sessionId: SessionId | null;
  visible: boolean;
  state: AttentionState;
  cwd: string | null;
  project: string | null;
  createdAt: number;
  fontFamily: string;
  fontSize: number;
  onFontSizeChange: (size: number) => void;
}

interface GhostUI {
  text: string;
  x: number;
  y: number;
  cellH: number;
}
interface PanelUI {
  items: string[];
  selected: number;
  x: number;
  y: number;
}

/** Pixel position of the terminal cursor, relative to the host. Uses only public API:
 *  the rendered screen's size ÷ cols/rows gives the cell size. */
function cursorPx(term: Terminal, host: HTMLElement) {
  const screen = host.querySelector(".xterm-screen") as HTMLElement | null;
  if (!screen || !term.cols || !term.rows) return null;
  const cellW = screen.clientWidth / term.cols;
  const cellH = screen.clientHeight / term.rows;
  if (!cellW || !cellH) return null;
  const s = screen.getBoundingClientRect();
  const h = host.getBoundingClientRect();
  return {
    x: s.left - h.left + term.buffer.active.cursorX * cellW,
    y: s.top - h.top + term.buffer.active.cursorY * cellH,
    cellW,
    cellH,
  };
}

export function TerminalView({
  sessionId,
  visible,
  state,
  cwd,
  project,
  createdAt,
  fontFamily,
  fontSize,
  onFontSizeChange,
}: TerminalViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const webglRef = useRef<WebglAddon | null>(null);
  const trackerRef = useRef<BlockTracker | null>(null);
  const ffRef = useRef(fontFamily);
  const fsRef = useRef(fontSize);
  const onZoomRef = useRef(onFontSizeChange);
  const { theme } = useTheme();

  // Self-healing WebGL renderer loader, kept in a ref so both the init effect and
  // the font-change effect can rebuild the renderer. A lost GL context (common on
  // macOS with several terminals) corrupts the glyph atlas → scrambled text; xterm
  // requires us to dispose and recreate the addon. A font-family swap needs the
  // same treatment: the atlas caches glyphs by char, not by face, so without a
  // rebuild the old typeface stays on screen. If WebGL is unavailable we fall back
  // to the DOM renderer, which reads the font straight from options.
  const loadWebglRef = useRef<() => void>(() => {});
  loadWebglRef.current = () => {
    const term = termRef.current;
    if (!term) return;
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        try {
          webgl.dispose();
        } catch {
          /* already gone */
        }
        webglRef.current = null;
        loadWebglRef.current();
      });
      term.loadAddon(webgl);
      webglRef.current = webgl;
    } catch {
      webglRef.current = null; // DOM-renderer fallback
    }
  };

  // Flips true once xterm + the block tracker exist, so BlockLayer can mount.
  const [blocksReady, setBlocksReady] = useState(false);

  // Command --help flag palette (command captured from the live prompt input).
  const [helpOpen, setHelpOpen] = useState(false);
  const [helpCommand, setHelpCommand] = useState("");

  // Age-based coloring of the whole terminal (fresh → stale ≥30m → old ≥4h).
  const now = useNow(30_000);
  const ageClass = `terminal-host--age-${ageTier(Math.max(0, now - createdAt))}`;

  // Project color (thin top accent), reactive to user color changes.
  useProjectColors();
  const projectColor = getProjectColor(project);

  // Rendered autocomplete UI.
  const [ghost, setGhost] = useState<GhostUI | null>(null);
  const [panel, setPanel] = useState<PanelUI | null>(null);

  // Autocomplete model (refs so the xterm data handler reads live values).
  const inputRef = useRef("");
  const dirtyRef = useRef(false);
  const atPromptRef = useRef(false);
  const cwdRef = useRef<string | null>(cwd);
  const suggsRef = useRef<string[]>([]);
  const selRef = useRef(0);
  const panelOpenRef = useRef(false);
  const uiActiveRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    cwdRef.current = cwd;
  }, [cwd]);

  useEffect(() => {
    atPromptRef.current = state === "IDLE";
  }, [state]);

  // Keep refs in sync with props.
  ffRef.current = fontFamily;
  fsRef.current = fontSize;
  onZoomRef.current = onFontSizeChange;

  // Initialize xterm once.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const term = new Terminal({
      fontFamily: ffRef.current,
      fontSize: fsRef.current,
      cursorBlink: true,
      theme: toXtermTheme(theme.terminal),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.querySelector(".terminal-grid") as HTMLElement);
    loadWebglRef.current();
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    // Command-block tracking (OSC 133/633/7 over the live byte stream).
    trackerRef.current = new BlockTracker(term);
    setBlocksReady(true);

    const onWinResize = () => fit.fit();
    window.addEventListener("resize", onWinResize);

    // Re-fit whenever the container changes size — opening/closing the Git or
    // Dashboard panel resizes the workspace without firing a window resize, so
    // without this the canvas keeps its old width and overflows the panels.
    let roRaf: number | null = null;
    const ro = new ResizeObserver(() => {
      if (roRaf != null) cancelAnimationFrame(roRaf);
      roRaf = requestAnimationFrame(() => {
        roRaf = null;
        // Skip while hidden (display:none → 0px) to avoid shrinking to min size.
        if (host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit();
      });
    });
    ro.observe(host);

    const onKeyDown = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.metaKey) return;
      const t = termRef.current;
      if (!t) return;
      if (e.key === "=" || e.key === "+") {
        e.preventDefault();
        onZoomRef.current(Math.min(MAX_FONT_SIZE, fsRef.current + ZOOM_STEP));
        return;
      }
      if (e.key === "-") {
        e.preventDefault();
        onZoomRef.current(Math.max(MIN_FONT_SIZE, fsRef.current - ZOOM_STEP));
        return;
      }
      if (e.key === "0") {
        e.preventDefault();
        onZoomRef.current(13);
        return;
      }
    };
    window.addEventListener("keydown", onKeyDown);

    return () => {
      window.removeEventListener("resize", onWinResize);
      window.removeEventListener("keydown", onKeyDown);
      if (roRaf != null) cancelAnimationFrame(roRaf);
      ro.disconnect();
      trackerRef.current?.dispose();
      trackerRef.current = null;
      setBlocksReady(false);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, []);

  useEffect(() => {
    if (visible && fitRef.current) {
      const id = setTimeout(() => fitRef.current?.fit(), 0);
      return () => clearTimeout(id);
    }
  }, [visible]);

  // Sync xterm theme when theme changes.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.options.theme = toXtermTheme(theme.terminal);
  }, [theme.terminal]);

  // Sync font props to xterm instance. We wait for the font to be loaded before
  // measuring (otherwise xterm caches fallback metrics) and clear the WebGL glyph
  // atlas so the new face actually repaints instead of reusing cached glyphs.
  // Skip the initial mount: the terminal was already constructed with the right
  // font, so clearing the atlas at startup is needless (and a corruption vector).
  const fontInitRef = useRef(true);
  const prevFamilyRef = useRef(fontFamily);
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    if (fontInitRef.current) {
      fontInitRef.current = false;
      prevFamilyRef.current = fontFamily;
      return;
    }
    // A family swap needs the renderer rebuilt; a size-only change (zoom) just
    // needs a re-fit, so we avoid the heavier reload on every Ctrl+= keystroke.
    const familyChanged = prevFamilyRef.current !== fontFamily;
    prevFamilyRef.current = fontFamily;
    devLog(
      `font effect: visible=${visible} family="${fontFamily}" size=${fontSize} familyChanged=${familyChanged} webgl=${!!webglRef.current}`,
    );
    let cancelled = false;
    const apply = () => {
      if (cancelled || !termRef.current) return;
      term.options.fontFamily = fontFamily;
      term.options.fontSize = fontSize;
      const fam = primaryFamily(fontFamily);
      const fp = fontProbe(fontSize, fam);
      let loaded = "unknown";
      try {
        loaded = String(document.fonts?.check?.(fp));
      } catch {
        loaded = "check-threw";
      }
      devLog(
        `font apply: primary="${fam}" probe="${fp}" browserHasFont=${loaded} familyChanged=${familyChanged} webgl=${!!webglRef.current}`,
      );
      if (familyChanged && webglRef.current) {
        // Rebuild the WebGL renderer so glyphs repaint in the new face — the
        // atlas keys on character, not font, so clearing it isn't enough.
        try {
          webglRef.current.dispose();
        } catch {
          /* already gone */
        }
        webglRef.current = null;
        loadWebglRef.current();
      } else {
        try {
          webglRef.current?.clearTextureAtlas();
        } catch {
          /* canvas fallback has no atlas */
        }
      }
      fitRef.current?.fit();
      term.refresh(0, term.rows - 1);
    };
    // Ask the browser to load the chosen family at this size, then apply. Falls
    // back to applying immediately if the Font Loading API isn't available.
    const probe = fontProbe(fontSize, primaryFamily(fontFamily));
    if (document.fonts?.load) {
      document.fonts.load(probe).then(apply, apply);
    } else {
      apply();
    }
    return () => {
      cancelled = true;
    };
  }, [fontFamily, fontSize]);

  // Attach to a session + wire autocomplete.
  useEffect(() => {
    const term = termRef.current;
    const host = hostRef.current;
    if (!term || !host || !sessionId) return;

    const unlisteners: Array<() => void> = [];
    let disposed = false;

    const resetSuggest = () => {
      suggsRef.current = [];
      selRef.current = 0;
      panelOpenRef.current = false;
      if (timerRef.current) clearTimeout(timerRef.current);
    };
    inputRef.current = "";
    dirtyRef.current = false;
    resetSuggest();

    const hideUI = () => {
      uiActiveRef.current = false;
      setGhost(null);
      setPanel(null);
    };

    // Push the model into React state for rendering (cheap; skips when nothing to show).
    // Wrapped so a positioning/DOM error can NEVER break the keystroke path below.
    const updateUI = () => {
      try {
        updateUIInner();
      } catch {
        hideUI();
      }
    };
    const updateUIInner = () => {
      const showable =
        atPromptRef.current &&
        !dirtyRef.current &&
        suggsRef.current.length > 0 &&
        inputRef.current.trim().length >= 2;
      if (!showable) {
        if (uiActiveRef.current) {
          uiActiveRef.current = false;
          setGhost(null);
          setPanel(null);
        }
        return;
      }
      const sel = suggsRef.current[selRef.current] ?? "";
      const pos = cursorPx(term, host);
      const input = inputRef.current;
      uiActiveRef.current = true;

      if (pos && sel.startsWith(input) && sel.length > input.length) {
        setGhost({ text: sel.slice(input.length), x: pos.x, y: pos.y, cellH: pos.cellH });
      } else {
        setGhost(null);
      }
      if (panelOpenRef.current && pos) {
        setPanel({
          items: suggsRef.current,
          selected: selRef.current,
          x: pos.x,
          y: pos.y + pos.cellH + 2,
        });
      } else {
        setPanel(null);
      }
    };

    const scheduleSuggest = () => {
      if (timerRef.current) clearTimeout(timerRef.current);
      const prefix = inputRef.current;
      if (dirtyRef.current || !atPromptRef.current || prefix.trim().length < 2) {
        resetSuggest();
        updateUI();
        return;
      }
      timerRef.current = setTimeout(async () => {
        try {
          const list = await getCommandSuggestions(prefix, cwdRef.current, MAX_SUGGESTIONS);
          if (inputRef.current !== prefix || dirtyRef.current) return;
          suggsRef.current = list.filter((s) => s.startsWith(prefix) && s.length > prefix.length);
          selRef.current = 0;
          if (suggsRef.current.length === 0) panelOpenRef.current = false;
          updateUI();
        } catch {
          /* ignore */
        }
      }, 110);
    };

    const feedInput = (data: string) => {
      if (data === "\r" || data === "\n" || data === "\x03" || data === "\x15") {
        inputRef.current = "";
        dirtyRef.current = false;
        resetSuggest();
        return;
      }
      if (data === "\x7f" || data === "\b") {
        inputRef.current = inputRef.current.slice(0, -1);
        return;
      }
      if (data.charCodeAt(0) === 0x1b) {
        dirtyRef.current = true; // arrows / history recall — stop tracking
        resetSuggest();
        return;
      }
      if ([...data].every((ch) => ch >= " ")) {
        inputRef.current += data;
        if (suggsRef.current.length && !suggsRef.current[selRef.current]?.startsWith(inputRef.current)) {
          resetSuggest();
        }
        return;
      }
      dirtyRef.current = true;
      resetSuggest();
    };

    const accept = () => {
      const s = suggsRef.current[selRef.current];
      if (s && s.startsWith(inputRef.current) && s.length > inputRef.current.length) {
        void writeStdin(sessionId, s.slice(inputRef.current.length));
        inputRef.current = s;
        resetSuggest();
        updateUI();
        return true;
      }
      return false;
    };

    (async () => {
      const subs = await Promise.all([
        onPtyOutput((e) => {
          if (e.id === sessionId) term.write(decodePtyBytes(e.base64Bytes));
        }),
        onSessionClosed((e) => {
          if (e.id === sessionId) term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n");
        }),
      ]);
      unlisteners.push(...subs);
      if (disposed) return;

      // Keep the ghost pinned to the cursor as the screen repaints.
      const renderHandler = term.onRender(() => {
        try {
          if (uiActiveRef.current || panelOpenRef.current) updateUI();
        } catch {
          hideUI();
        }
      });
      unlisteners.push(() => renderHandler.dispose());

      const dataHandler = term.onData((data) => {
        // Autocomplete interception. Any failure here must fall through to typing,
        // never swallow the key — a buggy suggestion can't break the terminal.
        try {
          const down = data === "\x1b[B" || data === "\x1bOB";
          const up = data === "\x1b[A" || data === "\x1bOA";
          const right = data === "\x1b[C" || data === "\x1bOC";
          const tab = data === "\t";
          const enter = data === "\r" || data === "\n";
          const esc = data === "\x1b";
          const haveSugg =
            atPromptRef.current && !dirtyRef.current && suggsRef.current.length > 0;

          // ↓ opens / walks the options panel.
          if (haveSugg && down) {
            panelOpenRef.current = true;
            selRef.current = Math.min(selRef.current + 1, suggsRef.current.length - 1);
            updateUI();
            return;
          }
          // ↑ walks up; closes at the top (then ↑ again is normal shell history).
          if (panelOpenRef.current && up) {
            if (selRef.current > 0) selRef.current -= 1;
            else panelOpenRef.current = false;
            updateUI();
            return;
          }
          // Accept: → always; Tab/Enter only while the panel is open.
          if (haveSugg && (right || ((tab || enter) && panelOpenRef.current))) {
            if (accept()) return;
          }
          // Esc dismisses the suggestion UI.
          if (esc && uiActiveRef.current) {
            resetSuggest();
            updateUI();
            return;
          }
        } catch {
          hideUI();
          // fall through and forward the key
        }

        void writeStdin(sessionId, data);
        feedInput(data);
        try {
          scheduleSuggest();
        } catch {
          hideUI();
        }
      });
      unlisteners.push(() => dataHandler.dispose());

      const resizeHandler = term.onResize(({ cols, rows }) => void resizePty(sessionId, cols, rows));
      unlisteners.push(() => resizeHandler.dispose());

      void resizePty(sessionId, term.cols, term.rows);
      term.focus();
    })();

    return () => {
      disposed = true;
      if (timerRef.current) clearTimeout(timerRef.current);
      unlisteners.forEach((u) => u());
      uiActiveRef.current = false;
      setGhost(null);
      setPanel(null);
    };
  }, [sessionId]);

  return (
    <div
      className={`terminal-host ${ageClass}`}
      style={{ display: visible ? "block" : "none" }}
      ref={hostRef}
    >
      <div className="terminal-grid" />
      {project && (
        <div className="terminal-project-accent" style={{ background: projectColor }} aria-hidden />
      )}
      {blocksReady && sessionId && termRef.current && trackerRef.current && hostRef.current && (
        <BlockLayer
          term={termRef.current}
          tracker={trackerRef.current}
          host={hostRef.current}
          visible={visible}
          sendInput={(d) => void writeStdin(sessionId, d)}
        />
      )}
      {visible && state === "IDLE" && (
        <button
          className="cmd-help-fab"
          title="Show this command's --help flags"
          onClick={() => {
            setHelpCommand(inputRef.current.trim());
            setHelpOpen(true);
          }}
        >
          ⚑ flags
        </button>
      )}
      {helpOpen && sessionId && (
        <CommandHelp
          cwd={cwd}
          command={helpCommand}
          onClose={() => {
            setHelpOpen(false);
            termRef.current?.focus();
          }}
          onPick={(token) => {
            const prefix = inputRef.current.endsWith(" ") || inputRef.current === "" ? "" : " ";
            void writeStdin(sessionId, `${prefix}${token} `);
            inputRef.current = `${inputRef.current}${prefix}${token} `;
          }}
        />
      )}
      {ghost && visible && (
        <span
          className="ac-ghost"
          style={{
            left: ghost.x,
            top: ghost.y,
            height: ghost.cellH,
            lineHeight: `${ghost.cellH}px`,
            fontFamily,
            fontSize,
          }}
          aria-hidden
        >
          {ghost.text}
        </span>
      )}
      {panel && visible && (
        <div className="ac-panel" style={{ left: panel.x, top: panel.y }}>
          {panel.items.map((it, i) => (
            <div key={i} className={`ac-opt ${i === panel.selected ? "ac-opt--sel" : ""}`}>
              {it}
            </div>
          ))}
          <div className="ac-panel-foot">↑↓ navigate · ↵ or → fill · esc close</div>
        </div>
      )}
    </div>
  );
}
