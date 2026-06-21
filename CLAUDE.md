# Conductor — Terminal Focus Dashboard

## Overview
A macOS Tauri 2 desktop app that wraps a real terminal (xterm.js + portable-pty) with a Focus Dashboard that tracks session attention states (RUNNING/IDLE/WAITING). Helps developers manage multiple AI coding agent terminals.

## Stack
- **App shell**: Tauri 2 (Rust backend, WKWebView frontend)
- **Backend**: Rust — PTY management, detection engine, SQLite persistence
- **Frontend**: React + TypeScript — dashboard, terminal, settings
- **Terminal**: xterm.js + WebGL addon (viewport renderer)
- **PTY**: portable-pty (from WezTerm ecosystem) or tauri-plugin-pty
- **Persistence**: SQLite (rusqlite, WAL mode)
- **Notifications**: macOS Notification Center via notify-rust

## Build Phases (from IMPLEMENTATION-PLAN.md)
1. **Phase 0** — Spike: one working terminal (Tauri + React + xterm.js + portable-pty)
2. **Phase 0.5** — Freeze contracts: IPC, SQLite schema, Detection trait
3. **Phase 1** — Multi-session + cap: tabs, open/close/rename, max-session enforcement
4. **Phase 2** — Metadata: project, task labels, uptime, counts → SQLite
5. **Phase 3** — Detection engine: OSC 133, heuristic, state machine (the moat)
6. **Phase 4** — Dashboard + focus: polished UI, "needs you" zone, notifications
7. **Phase 5** — Hardening + beta: perf, FP tuning, notarization

## Key Architecture Decisions
- Library-wrap approach: use portable-pty + xterm.js, don't fork maiterm
- Detection is a pure Rust module over byte stream (no Tauri/PTY coupling)
- SQLite with WAL mode for persistence
- IPC events: `pty://output`, `session://state`, `session://meta`, `session://closed`, `cap://reached`
- Detection engine: OSC 133 primary + activity heuristic fallback + optional pattern matching

## File Ownership
- `src-tauri/src/pty.rs`, `src/session.rs` — Terminal Core
- `src-tauri/src/detection/**` — Detection Engine
- `src-tauri/src/db.rs`, `migrations/**`, `notify.rs` — Persistence + Notifications
- `src/**` (React) — Frontend Dashboard
- `src-tauri/src/ipc.rs`, `lib.rs`, `commands.rs`, `src/ipc/types.ts` — Frozen contracts

## Code Standards
- Rust: clippy-clean, type-safe, no unwrap() in production paths
- TypeScript: strict mode, proper types
- React: functional components, hooks

## Key Commands
- `cargo tauri dev` — dev mode with hot reload
- `cargo build` — Rust build
- `npm run build` — frontend build
- `cargo test` — Rust tests
