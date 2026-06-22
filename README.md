<div align="center">

# Aran Terminal

**A lightweight, Rust-built terminal manager with a focus dashboard for running many AI
coding agents at once.**

Aran Terminal is a fast, native macOS app — built on a **Rust** backend — that wraps a
genuine PTY-backed terminal and *watches* each session to tell you which one actually needs
you, so a wall of agent terminals stops being a wall of noise. It's deliberately lightweight:
a Rust core and a small WebView UI, shipping as a ~4 MB DMG.

![platform](https://img.shields.io/badge/platform-macOS-000000?style=flat-square)
![built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB?style=flat-square)
![frontend](https://img.shields.io/badge/frontend-React%20%2B%20TypeScript-0A7EA4?style=flat-square)
![backend](https://img.shields.io/badge/backend-Rust-CE412B?style=flat-square)
![license](https://img.shields.io/badge/license-MIT-success?style=flat-square)

<br />

### [⬇️ Download for macOS (Apple Silicon) — .dmg](https://github.com/arunkarnann/aran-terminal/releases/latest/download/Aran-Terminal_1.1.0_aarch64.dmg)

[![Download .dmg](https://img.shields.io/badge/Download-.dmg%20(Apple%20Silicon)-50fa7b?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/arunkarnann/aran-terminal/releases/latest/download/Aran-Terminal_1.1.0_aarch64.dmg)

<sub>~4 MB · macOS on Apple Silicon · not yet notarized — on first launch **right-click the app → Open**</sub>

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

- **Real terminal** — xterm.js with the WebGL renderer over a `portable-pty` backend (the
  WezTerm PTY library). Full shell, not an emulated subset.
- **Attention detection engine** — a pure Rust module over the byte stream combining
  **OSC 133** shell-integration markers with an activity heuristic and a state machine to
  decide RUNNING / IDLE / WAITING. This is the core idea.
- **Focus dashboard** — a "needs you" zone that floats the WAITING sessions to the top so you
  never hunt for the blocked one.
- **Multi-session tabs** — open, close, rename, and group sessions, with a configurable
  max-session cap.
- **Command blocks** — Warp-style per-command blocks with a status gutter, sticky command
  header, and a hover toolbar (re-run, copy, etc.).
- **Shell integration** — auto-generated, non-destructive zsh/bash hooks that source your real
  config and add OSC 133 markers (never replaces your dotfiles).
- **Command history autofill** — inline ghost suggestions drawn from your real command history.
- **Focus timer** — a built-in Pomodoro-style focus/break loop with progress rings, a daily
  goal, and break prompts.
- **Git panel** — at-a-glance repo status for the active session.
- **Daily summary** — uptime, command counts, and session activity rolled up per day.
- **Native notifications** — macOS Notification Center alerts when a session finishes a long
  command or starts waiting on you.
- **Themes** — Dracula, Catppuccin Mocha, Tokyo Night Storm, Nord, One Dark, Material
  Palenight, and GitHub Dark/Light.
- **Sharp, tech-forward UI** — fully squared, terminal-native aesthetic driven by a single
  design token.
- **Local-first persistence** — SQLite (WAL mode) via `rusqlite`. Your data stays on your
  machine.

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

> **Platform:** macOS only. The app relies on WKWebView and macOS notifications.

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

- **macOS**
- [Rust toolchain](https://www.rust-lang.org/tools/install) (stable)
- [Node.js](https://nodejs.org) 18+
- Xcode Command Line Tools (`xcode-select --install`)

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
