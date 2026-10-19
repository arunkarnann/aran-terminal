# Aran Terminal — native (pure Rust)

The Aran Terminal app rebuilt without a webview or JavaScript.

| Layer | Tauri build (`conductor/`) | Native build (`native/`) |
|---|---|---|
| UI | React + TypeScript in WKWebView | egui (OpenGL / glow backend) |
| Terminal | xterm.js + WebGL | `alacritty_terminal` grid, painted by `term_view.rs` |
| PTY, detection, SQLite | Rust | **same files**, shared via `#[path]` |

Shared, unchanged modules: `ipc.rs`, `db.rs`, `detection/`, `shell_integration.rs`, `notify.rs`
(all under `conductor/src-tauri/src`). Fix them once, both builds get it.

## Run / build

```sh
cargo run --release              # binary: target/release/aran (~5 MB)
cargo test
scripts/bundle-macos.sh          # → target/bundle/Aran Terminal.app + .dmg + .zip
UNIVERSAL=1 scripts/bundle-macos.sh   # arm64 + x86_64
```

Releases: push a `v*` tag; `.github/workflows/release.yml` builds the signed macOS
universal .dmg/.zip and a Linux .tar.gz and attaches them to the GitHub Release.

Data lives in `~/Library/Application Support/studio.gearup.conductor/native.db`
(separate from the Tauri app's `conductor.db`, so both can run side by side).

## Shortcuts

⌘T new tab · ⌘W close · ⌘1–9 switch · ⌘⇧[ ⌘⇧] prev/next · ⌘B dashboard ·
⌘+ / ⌘− / ⌘0 font size · ⌘K clear scrollback · ⌘C / ⌘V copy/paste ·
double-click tab to rename · Shift/Option+Enter = newline (Claude Code)

## Ported so far

Two-row tab groups per project folder (custom colours, reveal in Finder), Tasks sidebar
(⌘J), themes (Dracula, Tokyo Night Storm, GitHub Light), session cap + override dialog, live RUNNING/IDLE/WAITING detection, "Needs you"
list, sessions grouped by project with uptime/commands/memory, today's stats,
WAITING + long-command notifications, wait-threshold and cap settings.

## Not ported yet

Git panel, GitHub Issues, Focus View/timers, command history & suggestions,
session restore with scrollback, auto-updater,
mouse reporting to TUI apps (clicks), CJK font fallback.
