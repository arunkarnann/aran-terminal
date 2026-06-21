// Warp-style command "blocks" over xterm.js.
//
// A block = one shell prompt + the command typed at it + that command's output,
// delimited by OSC 133 markers (the same shell-integration sequences the Rust
// detection engine consumes — see src-tauri/src/shell_integration.rs). The bytes
// reach the frontend untouched via `pty://output`, so we register our own OSC
// handlers here and anchor xterm `IMarker`s at the boundaries.
//
// Boundaries (our shell-integration emits A/C/D, NOT B):
//   133;A         prompt start      -> top of a new block
//   633;E;<b64>   command text      -> the command about to run / that just ran
//   133;C         command start     -> output begins on the next line
//   133;D[;exit]  command end       -> output ends; exit code closes the block
//   7;file://...  cwd               -> attached to the active block
//
// Naming note: `FocusBlock` (pomodoro) already exists in the IPC types, so the
// terminal concept is `CommandBlock` throughout.
//
// xterm.js limits worth knowing (we degrade, never crash):
//   - Markers are disposed when their line scrolls out of scrollback (default
//     1000 lines); we drop the block's UI on dispose. True unlimited history
//     ("like Warp") needs backend-persisted output — a deliberate follow-up.
//   - During the alternate screen buffer (vim, less, htop, Claude Code's TUI…)
//     there is no durable scrollback, so we suspend block creation. The launching
//     command stays a single block whose output is the whole TUI session: its
//     133;C fires before alt-screen entry and 133;D after exit, both in the
//     normal buffer, so the boundaries remain valid.

import type { IDisposable, IMarker, Terminal } from "@xterm/xterm";

export type BlockStatus =
  | "active" // prompt shown, no command run yet (the live input line)
  | "running" // command executing
  | "success" // exited 0
  | "error" // exited non-zero
  | "aborted"; // session/command ended without a clean exit code

export interface CommandBlock {
  id: number;
  command: string | null;
  /** Line of the prompt (133;A) — the visual top of the block. */
  promptMarker: IMarker;
  /** Line where output begins (just after 133;C). Null until the command runs. */
  outputMarker: IMarker | null;
  /** Line of command completion (133;D). Null while running. */
  endMarker: IMarker | null;
  exitCode: number | null;
  status: BlockStatus;
  startedAt: number; // ms epoch, set at 133;C
  finishedAt: number | null; // ms epoch, set at 133;D
  cwd: string | null;
}

function decodeBase64Utf8(b64: string): string {
  try {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder().decode(bytes);
  } catch {
    return "";
  }
}

/**
 * Tracks command blocks for a single xterm `Terminal`. Pure (no React): owns the
 * block list + markers and notifies subscribers when the *set or status* of
 * blocks changes (not on scroll — positions are recomputed from live markers by
 * the renderer). Construct once per terminal; call `dispose()` on teardown.
 */
export class BlockTracker {
  private term: Terminal;
  private blocks: CommandBlock[] = [];
  private nextId = 1;
  private pendingCommand: string | null = null;
  private pendingCwd: string | null = null;
  private listeners = new Set<() => void>();
  private disposables: IDisposable[] = [];

  constructor(term: Terminal) {
    this.term = term;
    const osc = (code: number, fn: (data: string) => void) => {
      const d = term.parser.registerOscHandler(code, (data) => {
        try {
          fn(data);
        } catch {
          /* a malformed marker must never break the terminal */
        }
        return false; // let xterm proceed (it discards these OSCs anyway)
      });
      this.disposables.push(d);
    };

    osc(133, (data) => {
      const sep = data.indexOf(";");
      const kind = sep === -1 ? data : data.slice(0, sep);
      const arg = sep === -1 ? "" : data.slice(sep + 1);
      switch (kind) {
        case "A":
          this.onPromptStart();
          break;
        case "C":
          this.onCommandStart();
          break;
        case "D":
          this.onCommandEnd(arg === "" ? null : Number.parseInt(arg, 10));
          break;
        // "B" (prompt end) is never emitted by our integration; ignore if seen.
      }
    });

    // 633;E;<base64 command line>
    osc(633, (data) => {
      const sep = data.indexOf(";");
      if (sep === -1 || data.slice(0, sep) !== "E") return;
      const cmd = decodeBase64Utf8(data.slice(sep + 1)).trim();
      if (cmd) this.pendingCommand = cmd;
    });

    // 7;file://host/path
    osc(7, (data) => {
      const path = data.startsWith("file://")
        ? decodeURIComponent(data.replace(/^file:\/\/[^/]*/, ""))
        : data;
      if (path) this.pendingCwd = path;
    });

    // Refresh subscribers when the screen buffer flips (alt-screen enter/exit) so
    // overlays hide/show; positions themselves are recomputed by the renderer.
    this.disposables.push(term.buffer.onBufferChange(() => this.emit()));
  }

  /** True while a full-screen TUI owns the screen — block creation is suspended. */
  private get inAltScreen(): boolean {
    return this.term.buffer.active.type === "alternate";
  }

  private current(): CommandBlock | undefined {
    return this.blocks[this.blocks.length - 1];
  }

  private onPromptStart() {
    if (this.inAltScreen) return;
    // A prompt while a command is still "running" means we missed its D — close it.
    const cur = this.current();
    if (cur && cur.status === "running") {
      cur.status = "aborted";
      cur.finishedAt = Date.now();
    }
    const marker = this.term.registerMarker(0);
    if (!marker) return;
    const block: CommandBlock = {
      id: this.nextId++,
      command: null,
      promptMarker: marker,
      outputMarker: null,
      endMarker: null,
      exitCode: null,
      status: "active",
      startedAt: 0,
      finishedAt: null,
      cwd: this.pendingCwd,
    };
    marker.onDispose(() => this.removeBlock(block.id));
    this.blocks.push(block);
    this.trim();
    this.emit();
  }

  private onCommandStart() {
    if (this.inAltScreen) return;
    const cur = this.current();
    if (!cur) return;
    cur.command = this.pendingCommand ?? cur.command;
    cur.outputMarker = this.term.registerMarker(0) ?? null;
    cur.status = "running";
    cur.startedAt = Date.now();
    if (this.pendingCwd) cur.cwd = this.pendingCwd;
    this.pendingCommand = null;
    this.emit();
  }

  private onCommandEnd(exit: number | null) {
    const cur = this.current();
    // A D with no running command is shell-startup noise (no preceding C) — skip.
    if (!cur || cur.status !== "running") return;
    cur.endMarker = this.term.registerMarker(0) ?? null;
    cur.exitCode = exit;
    cur.status = exit == null ? "aborted" : exit === 0 ? "success" : "error";
    cur.finishedAt = Date.now();
    if (cur.command == null && this.pendingCommand) cur.command = this.pendingCommand;
    this.pendingCommand = null;
    this.emit();
  }

  private removeBlock(id: number) {
    const i = this.blocks.findIndex((b) => b.id === id);
    if (i === -1) return;
    this.blocks.splice(i, 1);
    this.emit();
  }

  /** Drop blocks whose anchor scrolled out of scrollback, keeping markers tidy. */
  private trim() {
    this.blocks = this.blocks.filter((b) => !b.promptMarker.isDisposed);
  }

  // ---- Public API ----

  /** Snapshot of current blocks, oldest first. */
  list(): CommandBlock[] {
    return this.blocks.slice();
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    for (const fn of this.listeners) {
      try {
        fn();
      } catch {
        /* ignore subscriber errors */
      }
    }
  }

  /** The command line for a block (what was typed at the prompt). */
  commandText(b: CommandBlock): string {
    return b.command ?? "";
  }

  /** Output rows of a block, read live from the xterm buffer (trailing blank
   *  lines trimmed). Empty while in alt-screen or if markers are gone. */
  outputText(b: CommandBlock): string {
    if (!b.outputMarker || b.outputMarker.isDisposed) return "";
    const buf = this.term.buffer.active;
    // 133;C fires with the cursor already on the first output line (the command
    // echo + newline is on the prompt line above), so output starts here.
    const start = b.outputMarker.line;
    const end =
      b.endMarker && !b.endMarker.isDisposed
        ? b.endMarker.line // exclusive: D lands on the next prompt's line
        : buf.length;
    const lines: string[] = [];
    for (let i = start; i < end && i < buf.length; i++) {
      const line = buf.getLine(i);
      lines.push(line ? line.translateToString(true) : "");
    }
    while (lines.length && lines[lines.length - 1].trim() === "") lines.pop();
    return lines.join("\n");
  }

  /** Command + output, the way Warp's "copy block" works. */
  fullText(b: CommandBlock): string {
    const cmd = this.commandText(b);
    const out = this.outputText(b);
    return out ? `${cmd}\n${out}` : cmd;
  }

  dispose() {
    for (const d of this.disposables) {
      try {
        d.dispose();
      } catch {
        /* ignore */
      }
    }
    this.disposables = [];
    this.listeners.clear();
    this.blocks = [];
  }
}
