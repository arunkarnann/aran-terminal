# Implementation Plan — Billable Time Tracking

Scoped for implementation agents. Covers the fix for the broken per-project
timer, a segment-based recording model, the SQLite schema, the IPC contract,
and the calendar-led Time page.

Target: `conductor` @ v2.7.0. New migration: `0004_time_segment.sql`.
Backfill: **none** — see [Migration](#migration).

---

## Problem

The per-project timer reports the **same number for every project**.
Reproduced against the live database at
`~/Library/Application Support/studio.gearup.conductor/conductor.db`:

```
p                           n   mins_each
--------------------------  --  ---------
kalkyo                      15  1024.8
SportsIntelligence-backend  12  1024.8
ssh                         10  1024.8
weding-studionxt-main        9  1024.8
arun                         9  1024.8
work-projects                8  1024.8
hipages_scraper              6  1024.8
academy-coach-mobile         4  1024.8
OutssMonoRepo                3  1024.8
unvoxd                       2  1024.8
macrosx                      2  1024.8
client                       2  1024.8
```

1024.8 min = 17.1 h = exactly `now − local midnight`. Every project is being
credited the entire elapsed day.

There are four independent defects stacked on top of each other.

### 1. Time is derived from tab lifetime, not work

`compute_per_project_union` (`src-tauri/src/db.rs:599`) defines a project's
time as `session.created_at → COALESCE(session.closed_at, now)`. That is the
interval during which a **tab existed**. Five tabs left open all day means five
projects each showing the whole day. Nothing in the calculation touches
keystrokes, commands, or output.

### 2. Sessions are almost never marked closed — this is the amplifier

`db::mark_session_closed` has exactly one caller — `src-tauri/src/pty.rs:244`,
inside the PTY reader thread, reached only on clean EOF. Quit the app or crash
it and the thread dies without running. `reconcile_orphans` (`db.rs:235`) would
have repaired this but is marked `#[allow(dead_code)]` and is no longer called.

**79 of 637 sessions have `closed_at IS NULL`**, the oldest dating to
2026-07-20. Because the query treats an unclosed session as running until
`until`, each of those 79 zombies is credited the *full window* — today,
tomorrow, and every day after, forever. That is why the numbers are not merely
wrong but identical.

### 3. Attribution is rewritten retroactively

`pty.rs:293` calls `db::update_project` on every cwd change, which does an
in-place `UPDATE session SET project_path = …`. One tab, and you `cd` from
client A to client B at 4pm: the whole session — including the six hours spent
on A — is silently reassigned to B. Only the last `cd` survives.

### 4. Projects are keyed on a basename

`project_name` is `Path::file_name(cwd)`. The database already contains three
collisions where one name maps to two distinct paths (`wedding-client`,
`hipages_scraper`, `OutssApp`), plus entries like `arun` (home directory) and
`ssh` being treated as billable projects. Two clients each with an `api/`
folder merge into one invoice line and one color.

### 5. Structural: union vs partition

`merge_intervals` computes a per-project **union**. That correctly answers "how
much of my day touched project X," but summed across projects it can exceed
24 hours. You cannot invoice 14 hours out of an 8-hour day. Billing needs a
**partition** of the timeline: at any instant, time belongs to exactly one
project.

---

## Approach — record segments, never derive them

Stop computing time after the fact. Append immutable rows as time passes,
closing and reopening whenever attribution changes. This kills defect 3 for
free — attribution is captured when it was true, rather than being overwritten
by the last `cd` — and it makes every minute an object the user can inspect and
edit, which is the requirement that separates a billing tool from an activity
dashboard.

### The billable cursor

At any instant, at most one *cursor* is open: the pair
`(active session, its project_path)`. A segment is the lifetime of one cursor
value. Because only one is ever open, daily totals partition the day and can
never sum past wall-clock.

**Cursor changes** (close current segment, open a new one):

- Active tab switches to a session with a different `project_path`
- The active session's cwd crosses a project boundary
- The user manually relabels the running segment

**Cursor closes** (no segment open):

- Presence idle beyond `idle_grace` (default 5 min) — closed at the timestamp
  of the *last evidence*, not at detection time
- Display sleep, screen lock, or app quit
- The user pauses tracking explicitly

### Two kinds of evidence

This is a terminal for supervising AI agents, so the usual "was there output?"
heartbeat is wrong. An agent that runs for forty minutes produces continuous
PTY output whether you are watching it or at lunch. Split the signals:

| Signal | Source | Effect |
| --- | --- | --- |
| **Presence** | keystroke into any PTY, app window focused, pointer activity in-app | Segment runs, marked `attended`. Resets the idle clock. |
| **Machine** | PTY output with no presence signal | Segment runs, marked `unattended`. Does *not* reset the idle clock; capped by `unattended_max` (default 30 min). |
| **Neither** | silence past `idle_grace` | Segment closes at last evidence. An idle gap is recorded for review. |

Unattended time is still recorded and still shown — hatched, in both the
calendar bar and the day rail — because supervising a long agent run is real
work for some clients and not for others. Whether it bills is a per-client
setting, not a hardcoded rule.

### Idle is a prompt, never a silent subtraction

When the user returns, the Time page shows a review banner: *"Idle 47 min from
14:20 — keep as [project] / discard / split."* Silently deleting time destroys
trust the first time the detector is wrong, and it will be wrong.

### Crash-safe by heartbeat

The open segment carries `last_beat_at`, rewritten every 30 s. On launch, close
any segment still `ended_at IS NULL` at its `last_beat_at` — **not** at
`now()`. That is the structural fix for defect 2: a crash can lose at most
30 seconds, never three weeks.

### Attribution backstop when shell integration is off

`ev.cwd` only arrives via OSC 133. Without shell integration installed, every
segment lands in `Unknown` — fatal for a billing tool. Add a 5-second poll of
the PTY child's foreground process group cwd (`libproc` /
`proc_pidinfo(PROC_PIDVNODEPATHINFO)` on macOS) as a fallback source. Record
which source resolved the path (`cwd_source`) so low-confidence attribution can
be flagged in review.

---

## Schema — migration 0004

Additive only, matching the convention in `db.rs::init` — append one
`execute_batch(include_str!(…))` line. Nothing in `0001`–`0003` is edited.

```sql
-- src-tauri/migrations/0004_time_segment.sql

CREATE TABLE IF NOT EXISTS client (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    rate_cents    INTEGER,                 -- per hour, in the currency below
    currency      TEXT NOT NULL DEFAULT 'AUD',
    rounding_min  INTEGER NOT NULL DEFAULT 0,   -- 0 | 6 | 15 | 30, applied at render
    archived_at   INTEGER
);

CREATE TABLE IF NOT EXISTS project (
    path            TEXT PRIMARY KEY,      -- canonical, absolute; the real identity
    display_name    TEXT NOT NULL,         -- editable; defaults to basename
    client_id       TEXT REFERENCES client(id),
    color_index     INTEGER NOT NULL,      -- assigned sequentially, not hashed
    billable        INTEGER NOT NULL DEFAULT 1,
    bill_unattended INTEGER NOT NULL DEFAULT 1,
    rate_cents      INTEGER,               -- overrides client rate when set
    archived_at     INTEGER
);

CREATE TABLE IF NOT EXISTS time_segment (
    id            TEXT PRIMARY KEY,             -- uuid
    session_id    TEXT REFERENCES session(id),  -- NULL for manual entries
    project_path  TEXT REFERENCES project(path),
    started_at    INTEGER NOT NULL,        -- epoch ms, UTC
    ended_at      INTEGER,                 -- NULL = currently open
    last_beat_at  INTEGER NOT NULL,        -- crash recovery watermark
    attended_ms   INTEGER NOT NULL DEFAULT 0,   -- presence-backed subset of the span
    origin        TEXT NOT NULL,           -- auto | manual | split | merged
    cwd_source    TEXT,                    -- osc133 | proc_poll | manual
    note          TEXT,
    edited_at     INTEGER,
    locked_at     INTEGER                  -- set when invoiced; immutable thereafter
);

CREATE TABLE IF NOT EXISTS idle_gap (
    id            TEXT PRIMARY KEY,
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER NOT NULL,
    prior_project TEXT,                    -- what the cursor was before the gap
    resolution    TEXT NOT NULL DEFAULT 'pending'  -- pending | discarded | kept | split
);

CREATE INDEX IF NOT EXISTS idx_seg_started  ON time_segment(started_at);
CREATE INDEX IF NOT EXISTS idx_seg_open     ON time_segment(ended_at) WHERE ended_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_seg_project  ON time_segment(project_path, started_at);
CREATE INDEX IF NOT EXISTS idx_gap_pending  ON idle_gap(resolution);
```

### Invariants the implementation must hold

- At most one row with `ended_at IS NULL` at any time. Enforce in the open
  helper: close the current segment before inserting.
- `attended_ms <= ended_at - started_at`. Unattended duration is the remainder.
- Any write touching a row with `locked_at IS NOT NULL` is rejected at the
  `db.rs` layer, not just hidden in the UI.
- Durations are stored as exact milliseconds. `rounding_min` is applied only
  when rendering an invoice — never baked into stored data.
- `reset_stats` must add `time_segment` and `idle_gap`, or a reset leaves
  orphaned rows that keep appearing in the calendar.
- Local dates come from the frontend as epoch-ms boundaries (the existing
  `focus_day` pattern is correct). Build the month grid with a real date API —
  never by adding `86_400_000` repeatedly, since DST days are 23 or 25 hours.

### Colors become data

`src/lib/projectColors.ts` hashes a name into a 12-entry palette and stores
overrides in `localStorage`. Once color *is* the identity in a calendar, a hash
collision is a correctness bug, and localStorage means the color is missing
from any exported report. Replace the hash with `project.color_index`, assigned
sequentially on first sight and persisted in SQLite; keep the same palette
array and the user override, but store the override on the project row.

---

## IPC contract additions

`CLAUDE.md` lists `ipc.rs` / `commands.rs` / `src/ipc/types.ts` as frozen
contracts, so these are strictly **additive**. `get_daily_summary` stays for
now and is repointed at the segment table internally, so `DailySummary.tsx`
keeps working while the Time page is built.

| Command | Args → Returns | Purpose |
| --- | --- | --- |
| `set_active_session` | `Option<SessionId> → ()` | Frontend reports tab focus. Currently the backend has no idea which tab is active — this is the load-bearing missing signal. |
| `report_presence` | `{ kind: "key"\|"focus"\|"blur"\|"pointer" } → ()` | Presence heartbeat from the window. |
| `get_month_summary` | `{ monthStartMs, monthEndMs } → MonthSummary` | Per-day, per-project rollup for the calendar grid. |
| `get_day_segments` | `{ dayStartMs, dayEndMs } → DaySegments` | Ordered segments plus gaps for the day rail. |
| `update_segment` | `{ id, startedAt?, endedAt?, projectPath?, note? } → TimeSegment` | Retime or relabel. Rejects locked rows. |
| `split_segment` | `{ id, atMs } → [TimeSegment, TimeSegment]` | Cut one segment in two at a point. |
| `merge_segments` | `{ ids: string[] } → TimeSegment` | Collapse adjacent same-project segments. |
| `create_segment` | `{ projectPath, startedAt, endedAt, note? } → TimeSegment` | Manual entry for untracked work. |
| `delete_segment` | `{ id } → ()` | Remove. Rejects locked rows. |
| `resolve_idle_gap` | `{ id, resolution, projectPath? } → ()` | Answer the idle prompt. |
| `list_projects` / `update_project_meta` | `… → Project[]` | Client mapping, rate, billable flag, color, display name. |
| `export_timesheet` | `{ sinceMs, untilMs, clientId?, format } → String` | CSV / Markdown timesheet. |
| `lock_segments` | `{ sinceMs, untilMs, clientId } → count` | Freeze an invoiced range. |

**New event:** `time://segment` — emitted on open, close, and edit, carrying
the current open segment (or `null`). The status bar subscribes so the running
project and elapsed time are always visible without polling.

---

## Time page

### Month view

The month grid is the landing surface: it answers "what did I do in July, and
what do I bill?" without a single click. Each cell carries the day total in
tabular figures and a stacked bar whose **width** encodes the total against the
month's longest day, and whose **fill** encodes the project mix. Two variables,
one mark, no legend lookup for magnitude.

```
July 2026 · 138.4 h tracked          [Month] Week  Day   All clients ▾  Export
────────────────────────────────────────────────────────────────────────────────
 MON        TUE        WED        THU        FRI        SAT        SUN
┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐
│ 29      │ 30      │  1  6.2 │  2  7.4 │  3  4.1 │  4    — │  5  1.3 │
│         │         │ ███▓▓   │ ████▒░  │ ██▒▒    │         │ █       │
├─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│  6  7.8 │  7  8.4 │  8  6.9 │  9  5.2 │ 10  3.6 │ 11    — │ 12    — │
│ ████▒▒▓ │ █████▒▒ │ ▒▒⁄⁄▒░  │ ▒▒▒░░   │ ░░▒     │         │         │
│         │         │ 2.0h un │         │         │         │         │
└─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘
  ■ kalkyo             48.6   $7,290      ■ hipages_scraper   15.4   $1,848
  ■ SportsIntelligence 36.2   $5,430      ■ macrosx            7.3   non-billable
  ■ wedding-studionxt  27.9   $3,348      ■ Other (4)          3.0   —
```

Bar width = day total against the month's longest day (8.4 h). `⁄⁄` = hatched
fill for unattended agent-supervision time. The footer totals are the invoice
lines — the number the user actually came for, present without a click.

**Rules that keep the grid readable:**

- Cap each cell at the **top 4 projects** by duration; the remainder collapses
  into one neutral "other" band. 30 projects would otherwise render as confetti.
- One legend for the whole month, in the footer — never per cell.
- Days with zero tracked time get no bar and no border fill, so the working
  rhythm is legible as negative space.
- Color alone never carries meaning: every cell states its total numerically,
  and the day detail names every project in text.
- Client filter in the header rescopes the whole grid — the natural motion
  before exporting one client's invoice.

### Day detail

Clicking a day opens the editing surface. This is where the tracker earns the
right to be invoiced from: an hour rail showing what was actually recorded,
with every segment draggable, splittable, and relabelable.

```
Wed 22 Jul · 7.2 h · 4.6 h attended       [+ Add entry] [Copy timesheet] [1 needs review]
────────────────────────────────────────────────────────────────────────────────
 08   09   10   11   12   13   14   15   16   17   18   19
┌────────────────────────────────────────────────────────────┐
│ ▓▓▓▓▓ ⁄⁄⁄⁄⁄⁄ ┄idle┄ ▓▓▓▓▓▓▓ ░░░░░ ⁄⁄ ▒▒▒▒                  │
└────────────────────────────────────────────────────────────┘

 08:18 – 10:04  ■ SportsIntelligence-backend            1:46   split  edit
 10:04 – 12:10  ■ SportsIntelligence-backend·unattended 2:06   bill   drop
 12:10 – 13:08  ┄ Idle — no activity detected           0:58   keep   discard  split
 13:08 – 15:22  ■ SportsIntelligence-backend            2:14   split  edit
 15:22 – 16:48  ■ wedding-studionxt-main                1:26   split  edit
 17:12 – 18:28  ■ macrosx · non-billable                1:16   split  edit
```

Dashed block = an unresolved idle gap; it contributes nothing to totals until
answered. Hatched fill = unattended. Locked (invoiced) segments render the same
but with actions replaced by a lock glyph.

**Interactions:**

- **Drag a segment edge** to retime; the adjacent segment absorbs the change so
  the day stays a partition with no accidental gaps.
- **Click a rail position** on a selected segment to split at that minute.
- **Shift-click two adjacent same-project segments** to merge.
- **Drag on empty rail** to create a manual entry — the escape hatch for
  whiteboard sessions, calls, and work done off-terminal.
- Every edit stamps `edited_at` and sets `origin`, so a client dispute can be
  answered with "this entry was auto-captured" vs "this was entered by hand."

### Where the page lives

New route `src/components/time/TimePage.tsx` with `MonthGrid.tsx`,
`DayRail.tsx`, `SegmentList.tsx`, and `ProjectSettings.tsx`.
`DailySummary.tsx` becomes the "Today" tab inside it rather than a separate
dialog. The status bar gains a live pill showing the running project and
elapsed time, with a click-to-pause control — the tracker must be visible and
stoppable, or people stop trusting it.

---

## Migration

**Do not backfill.** The existing 637 sessions do not contain the information
needed to reconstruct real work time. Any attempt to synthesize segments from
`created_at`/`closed_at` produces plausible fiction — the worst possible
failure mode for data you invoice from. Start `time_segment` empty.

Instead, on first launch after the upgrade:

1. Run a one-time repair:
   `UPDATE session SET closed_at = COALESCE((SELECT MAX(at) FROM state_event WHERE session_id = session.id), created_at) WHERE closed_at IS NULL`
   — this is exactly `reconcile_orphans`, which already exists at `db.rs:235`.
   Un-`allow(dead_code)` it and call it from `init`. It stops the 79 zombies
   from poisoning the legacy view.
2. Seed the `project` table from `SELECT DISTINCT project_path FROM session`,
   assigning `color_index` in first-seen order and defaulting `display_name` to
   the basename.
3. Show a one-time notice on the Time page: *"Time tracking starts today.
   Earlier data was recorded by a method that couldn't distinguish work from an
   open tab."* Honesty here is cheaper than a wrong invoice.
4. Mark home directory and non-repo paths (`arun`, `ssh`) as `billable = 0` by
   default; a path is billable by default only if it contains a `.git`
   directory.

---

## Ship order

Each phase is independently useful and independently verifiable. Do not start
the calendar before the segment recorder is trustworthy — a beautiful view over
bad data is the situation we are already in.

### Phase A — Stop the bleeding

One evening. No new UI.

- Call `reconcile_orphans` from `init`; drop the `#[allow(dead_code)]`.
- Also mark sessions closed in the app-exit handler, not only on PTY EOF.
- Add a "summary is derived from tab lifetime" caveat line to
  `DailySummary.tsx` until Phase C lands.

### Phase B — The recorder

The load-bearing work. Everything else is a view over this.

- Migration `0004`; `project` seeding; `reset_stats` updated.
- `set_active_session` + `report_presence` wired from `App.tsx` (tab switch,
  window focus/blur, keydown on the terminal).
- Cursor state machine in a new `src-tauri/src/timetrack.rs`, kept free of
  Tauri types the way `detection/` is — so it is unit-testable.
- 30 s heartbeat writer; crash recovery closes at `last_beat_at`.
- cwd poll fallback via `libproc`.
- **Tests**: totals never exceed wall-clock; exactly one open segment; tab
  switch splits attribution; idle closes at last evidence; crash recovery
  truncates rather than extends.

### Phase C — Month view + day rail

- `get_month_summary`, `get_day_segments`; `TimePage.tsx`, `MonthGrid`,
  `DayRail`.
- `projectColors.ts` repointed at `project.color_index`.
- Status-bar live pill with pause.
- Repoint `get_daily_summary` at segments; fold `DailySummary` in as the Today
  tab.

### Phase D — Editing + idle review

- Segment CRUD, split, merge, manual entry, drag-to-retime.
- Idle gap prompt on return; `idle_gap` resolution flow.
- Unattended time review controls.

### Phase E — Billing

- `client` mapping, rates, currency, rounding rule.
- CSV / Markdown timesheet export, grouped by client then project then day.
- `lock_segments` after invoicing; locked rows immutable at the db layer.

---

## Files touched

| Path | Change |
| --- | --- |
| `src-tauri/migrations/0004_time_segment.sql` | new |
| `src-tauri/src/timetrack.rs` | new — cursor state machine, no Tauri deps |
| `src-tauri/src/db.rs` | segment open/close/edit, month + day rollups, project/client CRUD; retire `compute_per_project_union`; extend `reset_stats`; call `reconcile_orphans` in `init` |
| `src-tauri/src/pty.rs` | ~line 288 — cwd change closes and reopens a segment instead of mutating the session row; keystroke path emits presence |
| `src-tauri/src/commands.rs` | new commands (additive) |
| `src-tauri/src/ipc.rs` | new payload types + `time://segment` event (additive) |
| `src-tauri/src/lib.rs` | register commands, spawn heartbeat ticker |
| `src/ipc/types.ts`, `src/ipc/api.ts` | mirror the new contract |
| `src/components/time/*` | new — TimePage, MonthGrid, DayRail, SegmentList, ProjectSettings |
| `src/lib/projectColors.ts` | read `color_index` from db; drop the name hash |
| `src/components/DailySummary.tsx` | fold into TimePage as Today |
| `src/App.tsx` | report active tab + window focus/blur + keydown presence |
| `src/components/StatusBar.tsx` | live running-project pill with pause |

---

## Definition of done

On a day where you worked 8 hours across three projects, the Time page shows
three numbers that **sum to roughly 8** and **differ from each other**.

That is the whole test. It is currently failed by both halves.
