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
universal .dmg/.zip and the Linux .tar.gz / .deb / .rpm / .AppImage
(`scripts/bundle-linux.sh`) and attaches them to the GitHub Release.

Data lives in `~/Library/Application Support/studio.gearup.conductor/native.db`
(separate from the Tauri app's `conductor.db`, so both can run side by side).

## Shortcuts

⌘T new tab · ⌘W close · ⌘1–9 switch · ⌘⇧[ ⌘⇧] prev/next · ⌘B dashboard · ⌘J tasks ·
⌘Y history · ⌘+ / ⌘− / ⌘0 font size · ⌘K clear scrollback ·
⌘I issues · ⌘⇧G git · ⌘⇧F focus timer · ⌘⇧T reopen closed tab · ⌘↑ / ⌘↓ previous/next command block ·
⌘C / ⌘V copy/paste · double-click tab to rename · Shift/Option+Enter = newline (Claude Code)

Command suggestions (at the shell prompt): grey ghost text from your history —
→ accept · ↓ open the list · ↑↓ choose · Tab/Enter accept from the list · Esc hide.
Toggle in Dashboard → Settings. Hold Shift while scrolling/selecting to bypass an app's
mouse mode.

## Ported so far

Two-row tab groups per project folder (custom colours, reveal in Finder), Tasks sidebar,
**Issues** sidebar for the project's local Beads (`bd`) tracker, **Git** panel (incl. merge,
cherry-pick from another branch's log, delete branch), command **History**, over-the-air
updates (v3.0.1+), command autocomplete at the prompt, mouse reporting to TUI apps (wheel/clicks
go to Claude Code, vim, htop …), all 8 themes, session cap + override dialog, live
RUNNING/IDLE/WAITING detection, "Needs you" list, sessions grouped by project with
uptime/commands/memory, today's stats, WAITING + long-command notifications,
wait-threshold and cap settings.

v3.1: **Warp-style command blocks** (`blocks.rs` — OSC 133 anchored to absolute grid
lines, with a handler wrapper that keeps the scroll count exact), **Focus View** timers
(`focus.rs`), **session restore with scrollback** (`scrollback.rs` serializes the grid
with SGR colours; snapshots every 30 s and on quit; ⌘⇧T reopens the last closed tab),
per-session **task labels**, macOS **Full Disk Access onboarding** (`perms.rs`), and
Linux **.deb / .rpm / .AppImage** packaging (`scripts/bundle-linux.sh`).

## Not ported yet

GitHub Issues sidebar (v3 has the local Beads tracker instead), command `--help` flag
palette, CJK font fallback.
