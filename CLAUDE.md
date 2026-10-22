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


<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
