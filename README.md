<div align="center">

# Aran Terminal

**A lightweight, Rust-built terminal manager with a focus dashboard for running many AI
coding agents at once.**

Aran Terminal is a fast, native desktop app written **entirely in Rust** — no webview, no
JavaScript — that wraps a genuine PTY-backed terminal and *watches* each session to tell you
which one actually needs you, so a wall of agent terminals stops being a wall of noise.
Since **v3.0.0** the UI is egui + `alacritty_terminal`: a ~6 MB universal download, ~0% CPU at idle.

![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-000000?style=flat-square)
![pure rust](https://img.shields.io/badge/100%25-Rust-CE412B?style=flat-square)
![ui](https://img.shields.io/badge/UI-egui-24C8DB?style=flat-square)
![terminal](https://img.shields.io/badge/terminal-alacritty__terminal-F46623?style=flat-square)
![license](https://img.shields.io/badge/license-MIT-success?style=flat-square)

<br />

### [⬇️ Download for macOS (Apple Silicon + Intel) — .dmg](https://github.com/arunkarnann/aran-terminal/releases/latest/download/Aran-Terminal_3.0.1_universal.dmg)

[![Download .dmg](https://img.shields.io/badge/Download-.dmg%20(Universal)-50fa7b?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/arunkarnann/aran-terminal/releases/latest/download/Aran-Terminal_3.0.1_universal.dmg)

<sub>~6 MB · not notarized — on first launch allow it in **System Settings → Privacy & Security → Open Anyway**
(or `xattr -dr com.apple.quarantine "/Applications/Aran Terminal.app"`) · maintainers: see [SIGNING.md](conductor/SIGNING.md)</sub>

### [⬇️ Download for Linux (x86_64) — .tar.gz](https://github.com/arunkarnann/aran-terminal/releases/latest/download/Aran-Terminal_3.0.1_linux_x86_64.tar.gz)

<sub>Extract and run `./aran`. The v2 Tauri builds (.deb / .rpm / .AppImage) remain on the
[v2.9.0 release](https://github.com/arunkarnann/aran-terminal/releases/tag/v2.9.0).</sub>

> **Upgrading from v2.x:** v2 apps don't auto-update to v3 — download v3 once. The native
> app keeps its data in `native.db` next to the v2 database. Source: [`native/`](native/).

[Landing page](https://arunkarnann.github.io/aran-terminal/) · [All releases](https://github.com/arunkarnann/aran-terminal/releases)

</div>

---

## Why

Running several AI coding agents means juggling several terminals. Most of the time you're
staring at sessions that are busy working, while the one that's *blocked waiting for your
answer* is buried three tabs away. Aran Terminal solves the attention problem: it classifies
every session as **RUNNING**, **IDLE**, or **WAITING**, surfaces the ones that need you, and
notifies you when a session goes quiet or asks a question.

## Features

**v3 (native, pure Rust)**

- **Real terminal** — `alacritty_terminal` grid painted by egui over a `portable-pty` backend.
  Truecolor, 256 colours, Nerd Font glyphs, scrollback, selection, bracketed paste.
- **Attention detection engine** — a pure Rust module over the byte stream combining
  **OSC 133** shell-integration markers with an activity heuristic and a state machine to
  decide RUNNING / IDLE / WAITING. This is the core idea.
- **Focus dashboard** — a "needs you" zone that floats WAITING sessions to the top, sessions
  per project with uptime / commands / memory, and today's stats.
- **Tab groups** — one group per project folder, with custom colours, reveal in Finder,
  rename, and a configurable max-session cap.
- **Tasks** — a to-do list with a Backlog → To do → In progress → Review → Done workflow,
  priorities, due dates, notes and a timeline.
- **Shell integration** — auto-generated, non-destructive zsh/bash hooks that source your real
  config and add OSC 133 markers (never replaces your dotfiles).
- **Native notifications** — when a session finishes a long command or starts waiting on you.
- **Command suggestions** — grey autocomplete from your history at the shell prompt
  (→ accept, ↓ list, Esc hide). Never shows inside Claude Code or other full-screen apps.
- **Mouse reporting** — wheel and clicks go to TUI apps that ask for them (Claude Code,
  vim, htop), so scrolling scrolls the app instead of walking its prompt history.
- **Command history** pane (⌘Y) — search everything you ran, click to type it again.
- **Over-the-air updates** (from v3.0.1) — the app checks GitHub Releases and updates itself.
- **Themes** — Dracula, Tokyo Night Storm, GitHub Light, Catppuccin Mocha, GitHub Dark,
  One Dark, Material Palenight, Nord.
- **Local-first persistence** — SQLite (WAL mode) via `rusqlite`.

**v2 only (Tauri build, not yet ported to v3):** Warp-style command blocks, focus timer,
Git panel, GitHub Issues sidebar and session restore with scrollback. Get them from the
[v2.9.0 release](https://github.com/arunkarnann/aran-terminal/releases/tag/v2.9.0).

## Stack

| Layer        | Tech                                                        |
| ------------ | ---------------------------------------------------------- |
| App shell    | [Tauri 2](https://tauri.app) (Rust backend, WKWebView UI)   |
| Frontend     | React + TypeScript + Vite                                   |
| Terminal     | xterm.js + WebGL addon                                      |
| PTY          | portable-pty (WezTerm ecosystem)                           |
| Detection    | Pure Rust module over the PTY byte stream                   |
| Persistence  | SQLite (rusqlite, WAL mode)                                 |
| Notifications| macOS Notification Center via notify-rust                   |

> **Platform:** macOS and Linux. The app runs on macOS (WKWebView) and Linux (WebKitGTK).

## Repository layout

The app lives in the [`conductor/`](./conductor) subdirectory:

```
.
├── conductor/            # the Aran Terminal app
│   ├── src/              # React + TypeScript frontend
│   ├── src-tauri/        # Rust backend (PTY, detection, db, notifications)
│   └── package.json
├── docs/                 # GitHub Pages landing page
└── README.md
```

## Getting started

### Prerequisites

- **macOS** or **Linux** (Ubuntu 22.04+, Fedora, Debian, or similar)
- [Rust toolchain](https://www.rust-lang.org/tools/install) (stable)
- [Node.js](https://nodejs.org) 18+

**macOS only:** Xcode Command Line Tools (`xcode-select --install`)

**Linux only:** WebKit and GTK development libraries:
```bash
# Debian / Ubuntu
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf build-essential curl wget file libssl-dev libxdo-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel \
  librsvg2-devel patchelf openssl-devel libxdo-devel
```

### Run in development

```bash
git clone https://github.com/arunkarnann/aran-terminal.git
cd aran-terminal/conductor
npm install
npm run tauri dev
```

`npm run tauri dev` launches the Rust backend and the Vite dev server together, with hot
reload for the frontend.

### Build a release bundle

```bash
cd conductor
npm install
npm run tauri build
```

The signed/unsigned `.app` and `.dmg` are emitted under
`conductor/src-tauri/target/release/bundle/`.

### Other commands

| Command            | What it does                          |
| ------------------ | ------------------------------------- |
| `npm run dev`      | Frontend only (Vite), no backend      |
| `npm run build`    | Type-check + build the frontend       |
| `cargo test`       | Run the Rust test suite (from `src-tauri/`) |

## How attention detection works

Each session's PTY output is streamed through a Rust detection engine that never touches Tauri
or the PTY directly — it's a pure function over bytes, which keeps it testable:

1. **OSC 133 markers** (primary) — when shell integration is active, the shell emits
   prompt/command/output escape sequences. These give exact command start/end signals.
2. **Activity heuristic** (fallback) — when markers aren't available, output cadence drives a
   RUNNING ↔ IDLE classification.
3. **State machine** — debounces transitions and decides when a session is genuinely
   **WAITING** for you (and worth a notification) versus just idle.

## Contributing

Issues and PRs are welcome. The codebase aims to be clippy-clean Rust (no `unwrap()` in
production paths) and strict-mode TypeScript with functional React components.

## License

[MIT](./LICENSE) © Arun
