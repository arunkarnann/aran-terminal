-- Focus View (deep-work timers). Additive migration — see 0001_init.sql header.
-- A focus_block is a deliberate deep-work (or break) interval bound to one session.
-- v1 ships the pure two-field timer model (started_at + planned_ms); there is no
-- pause/resume, so `remaining = started_at + planned_ms - now` always holds.

CREATE TABLE IF NOT EXISTS focus_block (
    id          TEXT PRIMARY KEY,
    session_id  TEXT REFERENCES session(id),   -- nullable: a break is not bound to a session
    task_label  TEXT,
    kind        TEXT NOT NULL,                  -- focus | break
    started_at  INTEGER NOT NULL,
    planned_ms  INTEGER NOT NULL,
    ended_at    INTEGER,
    status      TEXT NOT NULL                   -- active | completed | abandoned
);

CREATE INDEX IF NOT EXISTS idx_focus_block_started ON focus_block(started_at);
CREATE INDEX IF NOT EXISTS idx_focus_block_status  ON focus_block(status);

-- Daily focus goal lives in the existing settings kv table under key 'focus.daily_goal_ms'.
