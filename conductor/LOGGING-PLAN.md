# Implementation Plan — Diagnostic Logging & Telemetry

Scoped for implementation agents. Covers offline structured logging on both
sides (Rust + TypeScript), a log viewer in the UI, and a future path to
server-side crash reporting.

---

## Problem

Terminal issues are invisible until a user complains:
- Scroll feels sluggish (renderer stalls, large PTY bursts)
- TUI input fields (OpenCode, Claude) don't respond (stdin routing, escape
  sequence parsing)
- Tamil / wide Unicode renders wrong (font atlas gaps, WebGL glyph cache)
- Detection false-positives / missed transitions

There is **zero structured logging** today — only a TEMP `dev_log` that does
`eprintln!`.

---

## Approach — two layers

### Layer 1: Structured ring-buffer log (offline, always-on)

A fixed-size in-memory ring buffer (~2 MB, ~10 000 entries) on each side
(Rust + TS). Entries are `{ ts, level, module, event, fields }`. When the
buffer is full the oldest entry is dropped. On app quit the buffer is
flushed to a JSONL file on disk (`~/Library/Application
Support/studio.gearup.conductor/logs/<date>.log`). Old log files (>7 days)
are pruned on startup.

This is cheap (no I/O per log call), always-on, and survives crashes
(we flush on SIGTERM / app shutdown hook; for hard crashes we accept loss
of the in-memory tail).

### Layer 2: Server-side reporting (future, opt-in)

A "Send diagnostics" toggle in Settings. When enabled, the app POSTs the
day's JSONL to a collector endpoint on a 24h timer (or on demand). No
PII is logged — only anonymous session IDs, event names, and numeric
measurements. The endpoint URL is a constant; no third-party SDK needed.

This plan covers Layer 1 fully and stubs Layer 2.

---

## Backend — `src-tauri/src/log.rs` (new file)

### Struct

```rust
use std::sync::Mutex;
use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Level { Debug, Info, Warn, Error }

#[derive(Serialize)]
pub struct Entry {
    pub ts: i64,           // epoch ms
    pub level: Level,
    pub module: &'static str,  // e.g. "pty", "detection", "git"
    pub event: &'static str,   // e.g. "pty_read_error", "webgl_context_lost"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<i64>,       // duration measurement
}
```

### Global ring buffer

```rust
pub struct LogBuffer {
    entries: Vec<Entry>,   // pre-allocated to CAPACITY
    write_idx: usize,
    full: bool,
}
static LOG: Mutex<LogBuffer> = ...;  // or pass via Tauri manage()

pub fn log(level: Level, module: &'static str, event: &'static str, ...) { ... }
pub fn flush_to_disk() { ... }       // called on app shutdown
pub fn drain_since(ts: i64) -> Vec<Entry> { ... }  // for the UI / server POST
```

### Call sites (backend)

| Location | Event | Level |
|----------|-------|-------|
| `pty.rs` reader thread — read error | `pty_read_error` | Error |
| `pty.rs` reader thread — EOF | `pty_eof` | Info |
| `pty.rs` `create_session` — PTY created | `session_created` | Info |
| `pty.rs` `create_session` — PTY failed | `session_create_failed` | Error |
| `detection/engine.rs` — state transition | `state_change` | Debug |
| `detection/engine.rs` — WAITING detected | `waiting_detected` | Debug |
| `git.rs` — command failed | `git_cmd_error` | Warn |
| `git.rs` — network command succeeded | `git_sync_ok` | Info |
| `db.rs` — reconcile orphans count | `orphan_reconcile` | Info |
| `lib.rs` startup — DB open | `db_open` | Info |
| `lib.rs` startup — DB fallback | `db_fallback_memory` | Warn |

### Tauri command

```rust
#[tauri::command]
pub fn get_logs(since: i64) -> Vec<LogEntry> {
    crate::log::drain_since(since)
}
```

Register in `lib.rs` invoke_handler.

---

## Frontend — `src/lib/logger.ts` (new file)

### Client-side ring buffer

Same shape as the Rust side but in TypeScript. Fixed capacity ~5000 entries.
Flushed to the Rust side via a periodic `dev_log`-style IPC call (batch),
or kept local for the in-app viewer.

```ts
interface LogEntry {
  ts: number;
  level: "debug" | "info" | "warn" | "error";
  module: string;
  event: string;
  sessionId?: string;
  detail?: string;
  ms?: number;
}

class Logger {
  private buf: LogEntry[] = [];
  private capacity = 5000;

  log(level, module, event, fields?) { ... }
  debug(module, event, fields?) { ... }
  info(module, event, fields?) { ... }
  warn(module, event, fields?) { ... }
  error(module, event, fields?) { ... }
  drain(): LogEntry[] { ... }
}

export const logger = new Logger();
```

### Call sites (frontend)

| Location | Event | Level |
|----------|-------|-------|
| `TerminalView.tsx` — WebGL context lost | `webgl_context_lost` | Warn |
| `TerminalView.tsx` — WebGL addon create failed | `webgl_create_failed` | Error |
| `TerminalView.tsx` — DOM fallback | `dom_renderer_fallback` | Info |
| `TerminalView.tsx` — font load | `font_loaded` | Debug |
| `TerminalView.tsx` — font load failed | `font_load_failed` | Warn |
| `TerminalView.tsx` — `term.onData` error | `input_handler_error` | Error |
| `TerminalView.tsx` — resize fit | `term_fit` | Debug |
| `TerminalView.tsx` — scroll performance (if measurable) | `scroll_lag` | Warn |
| `useSessionManager.ts` — session poll slow (>500ms) | `poll_slow` | Warn |
| `GitPanel.tsx` — git action error | `git_action_error` | Error |
| `api.ts` — IPC invoke error | `ipc_error` | Error |

---

## Log viewer — Settings dialog or dedicated panel

Add a "Logs" tab in the Settings dialog (or a ⌘L shortcut panel):
- Fetches `get_logs(since)` from Rust (last 1 hour)
- Merges with frontend buffer
- Renders a scrollable list: `[HH:MM:SS] [LEVEL] [module] event {fields}`
- Filter by level (checkboxes) and module (dropdown)
- "Copy all" button → clipboard
- "Export…" → save JSONL to Downloads

---

## File changes summary

| File | Action |
|------|--------|
| `src-tauri/src/log.rs` | **New** — ring buffer, `log()`, `flush_to_disk()`, `drain_since()`, prune old files |
| `src-tauri/src/lib.rs` | Add `mod log;`, call `log::flush_to_disk()` on shutdown, register `get_logs` command |
| `src-tauri/src/commands.rs` | Add `get_logs` command |
| `src-tauri/src/pty.rs` | Add `log::log(...)` calls at key points |
| `src-tauri/src/detection/engine.rs` | Add `log::log(...)` on state transitions (optional, Debug level) |
| `src-tauri/src/git.rs` | Add `log::log(...)` on command failures |
| `src-tauri/Cargo.toml` | No new deps (use `serde_json` already present for JSONL serialization) |
| `src/lib/logger.ts` | **New** — TS ring buffer + singleton |
| `src/components/TerminalView.tsx` | Replace `devLog` calls with `logger.*`, add WebGL/font/input logging |
| `src/ipc/api.ts` | Add `getLogs()` wrapper |
| `src/ipc/types.ts` | Add `LogEntry` type |
| `src/components/Settings.tsx` | Add Logs tab with viewer |

---

## Future: server-side reporting (Layer 2)

- Add a `settings` key `diagnostics.enabled` (bool, default false)
- A Tauri command `upload_logs` that POSTs `drain_since(last_upload)` to a
  hardcoded collector URL (e.g. `https://collector.arunterminal.dev/logs`)
- Runs on a 24h timer (or on demand from Settings → "Send now")
- No PII: session IDs are UUIDs, no cwd paths, no command text in logs
- Collector is a simple POST endpoint (Cloudflare Worker / Fly.io) that
  appends to a file or S3 bucket

---

## Gotchas

- **Ring buffer capacity**: 2 MB is ~10 000 entries at ~200 bytes each.
  If logging is too chatty (Debug-level state transitions), entries get
  evicted fast. Gate Debug behind a `RUST_LOG`-style env var for dev builds.
- **Crash loss**: in-memory buffer is lost on hard crash (SIGKILL, OOM).
  Acceptable for v1; `flush_to_disk` on graceful exit covers quit/restart.
- **Thread safety**: the ring buffer must be `Mutex`-protected. The PTY
  reader thread logs from a separate thread; all other log calls are on
  the main thread. A `parking_lot::Mutex` or `std::sync::Mutex` is fine.
- **Log file pruning**: delete files older than 7 days on startup. Don't
  prune on every write (waste of I/O).
- **Unicode in logs**: Tamil text in `detail` fields is fine — JSON handles
  it natively. The log *viewer* must use a Unicode-capable font (already
  does — Menlo/Fira Code have Tamil fallback).
