-- Tasks sidebar — a lightweight dev to-do list with a workflow and an event timeline.
-- Additive migration — see 0001_init.sql header. Re-run on every launch, so it must
-- stay idempotent (IF NOT EXISTS only, never ALTER).

CREATE TABLE IF NOT EXISTS task (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    notes         TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL,              -- backlog | todo | in_progress | review | blocked | done
    priority      INTEGER NOT NULL DEFAULT 2, -- 0 (urgent) .. 3 (low)
    project       TEXT,                       -- auto-tagged from the active terminal's project
    due_at        INTEGER,                    -- epoch ms (local end-of-day chosen by the frontend)
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    started_at    INTEGER,                    -- first time it entered in_progress
    completed_at  INTEGER                     -- set on entering done, cleared on leaving it
);

CREATE INDEX IF NOT EXISTS idx_task_status ON task(status);

-- Timeline: one row per creation / status change.
CREATE TABLE IF NOT EXISTS task_event (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    from_status  TEXT,                        -- null on creation
    to_status    TEXT NOT NULL,
    at           INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_event_task ON task_event(task_id, at);
