-- GitHub issues sidebar. Additive migration — see 0001_init.sql header. Re-run on every
-- launch, so it must stay idempotent (IF NOT EXISTS only, never ALTER).
-- No tokens are stored here: they live in gh's keyring or the macOS Keychain.

-- A selected repo ("owner/name") or Projects v2 board (node id) for one account.
CREATE TABLE IF NOT EXISTS gh_source (
    id              TEXT PRIMARY KEY,           -- "<account>|<kind>|<key>"
    account         TEXT NOT NULL,
    kind            TEXT NOT NULL,              -- repo | project
    key             TEXT NOT NULL,
    title           TEXT NOT NULL,
    url             TEXT,
    status_options  TEXT NOT NULL DEFAULT '[]', -- JSON array, project status column order
    synced_at       INTEGER,
    error           TEXT,
    added_at        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_gh_source_account ON gh_source(account);

-- Cached items, replaced wholesale per source on each sync. `data` is the GhItem JSON.
CREATE TABLE IF NOT EXISTS gh_item (
    source_id   TEXT NOT NULL,
    item_key    TEXT NOT NULL,
    data        TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (source_id, item_key)
);

-- Account settings (selected account, pasted-token logins) live in the settings kv table.
