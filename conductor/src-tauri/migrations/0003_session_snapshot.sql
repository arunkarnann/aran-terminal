-- Session restore (PRD §8.1) — persists open tabs so they survive app relaunch.
-- Scrollback is a serialized xterm.js buffer (from @xterm/addon-serialize).
-- closed_at IS NULL  → active session, candidate for restore on launch.
-- closed_at IS SET   → recently closed (Cmd+Shift+T can reopen).

CREATE TABLE IF NOT EXISTS session_snapshot (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL UNIQUE,
    name          TEXT,
    project_path  TEXT,
    project_name  TEXT,
    task_label    TEXT,
    shell         TEXT NOT NULL,
    cwd           TEXT,
    tab_order     INTEGER NOT NULL DEFAULT 0,
    is_active     INTEGER NOT NULL DEFAULT 0,
    scrollback    BLOB,
    closed_at     INTEGER,
    updated_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshot_open ON session_snapshot(closed_at);
