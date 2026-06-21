# Aran Terminal — app

This directory contains the **Aran Terminal** Tauri app. See the
[project README](../README.md) for an overview, features, and the stack.

## Develop

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build   # → src-tauri/target/release/bundle/
```

## Layout

- `src/` — React + TypeScript frontend (dashboard, terminal, focus view, settings)
- `src-tauri/` — Rust backend (PTY, detection engine, SQLite, notifications)

Recommended IDE setup: [VS Code](https://code.visualstudio.com/) +
[Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) +
[rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer).
