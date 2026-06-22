# Implementation Plan — Aran Terminal fixes

Scoped per issue with exact files, root-cause findings, and approach. Intended to be
handed to implementation agents working in parallel.

> Note: `@tauri-apps/plugin-opener` is already installed, so no new backend command is
> needed for the Finder reveal.

---

## Issue 1 — Per-tab "Open in Finder" button
**Goal:** each terminal tab gets a button that reveals its working folder in Finder.

- **File:** `src/components/TabStrip.tsx` (the `Tab` component, ~line 129–192). Add a
  button next to `tab-close`. Mirror it into the `TabGroups`/vertical `Tab` variant if
  grouped tabs should have it too.
- **API:** use the already-installed JS plugin —
  `import { revealItemInDir } from "@tauri-apps/plugin-opener"`. On click:
  `if (session.cwd) revealItemInDir(session.cwd)`. Call `stopPropagation()` so it
  doesn't switch/rename the tab.
- **Permissions:** confirm `revealItemInDir` is allowed in
  `src-tauri/capabilities/*.json` (opener permission
  `opener:allow-reveal-item-in-dir`). If missing, add it — most likely gotcha.
- **Disabled state:** `session.cwd` can be null before the first OSC-7 cwd arrives;
  disable/hide the button until `cwd` is known.
- **CSS:** add a `.tab-reveal` style mirroring `.tab-close` (`src/App.css` near 605+).

---

## Issue 2 — Resizable side panels (Dashboard, Git, …)
**Goal:** drag-resize the Dashboard and Git panels instead of fixed widths.

- **Current:** `.dashboard { width: 320px }` (`src/App.css:900`),
  `.git-panel { min-width:360px; max-width:480px }` (~734). Both fixed.
- **Approach:** add a thin drag handle on each panel's inner edge. Track width in React
  state, persist to `localStorage` (`conductor-dashboard-width`, `conductor-git-width`),
  clamp to min/max. Apply as inline `style={{ width }}` and drop the fixed CSS width.
- **Files:** `src/components/Dashboard.tsx` (root `<aside className="dashboard">`,
  line 65) and `src/components/GitPanel.tsx`. A small shared
  `useResizable(side, key, min, max)` hook in `src/lib/` keeps both DRY (pointer events:
  `pointerdown` on handle → `pointermove` delta → set width → `pointerup` persist).
- **Gotcha:** the terminal (`xterm` + fit addon) must refit when panel width changes.
  There's already a resize path in `TerminalView.tsx` — trigger a window `resize` event
  (or the existing fit hook) on `pointerup`/width-change so the terminal reflows.

---

## Issue 3 — Move Today / Settings / Dashboard into the tab row, fixed width
**Goal:** relocate the action buttons into the terminal-tab section with consistent
fixed-width buttons.

- **Current:** `.topbar-actions` (Today, Settings, Group, Dashboard) sits in
  `<header className="topbar">` beside `TabStrip` (`src/App.tsx:130–151`).
- **Approach:** give `.tb-btn` a fixed width (e.g. `width: 84px; justify-content:center`)
  in the `src/App.css:62` block so they stop reflowing. Keep `.topbar-actions` pinned
  right (`margin-left:auto`) and **not** part of the horizontally-scrolling tab list, so
  tabs scroll under a stable action cluster.
- **Decision needed (small):** "terminal tab section" is ambiguous — confirm whether the
  buttons should be (a) right-aligned on the same row as the tabs (minimal change,
  recommended) or (b) inside the scrolling `.tab-strip` itself. Recommend (a).

---

## Issue 4 — "Today only" stats + Reset button
**Goal:** Today view shows only today (it already queries `todayMidnight → now`,
`DailySummary.tsx:11–13` — correct) and gains a Reset.

- **Reset button:** add to `DailySummary.tsx` `dialog-actions` (line 61). Needs a new
  backend command `reset_stats` in `commands.rs` + registration in `lib.rs`
  invoke_handler + a wrapper in `src/ipc/api.ts`.
- **What reset does (pick one):**
  - **Recommended:** purge *historical* rows that aren't live —
    `DELETE FROM state_event; DELETE FROM command; DELETE FROM focus_block;
    DELETE FROM session WHERE closed_at IS NOT NULL;` Keeps currently-open sessions,
    wipes accumulated junk.
  - Heavier: wipe everything and re-seed. Add a JS `confirm()` first — destructive.
- Re-fetch the summary after reset so the dialog updates.

---

## Issue 5 — The "arun = 616 hours" bug  ⭐ root cause confirmed
Reproduced against the live DB
(`~/Library/Application Support/studio.gearup.conductor/conductor.db`): project **arun**
reports **616.7h today** from **45 sessions, 44 still `closed_at IS NULL`**. Two
compounding defects:

### 5a. Orphaned sessions never reconciled
`mark_session_closed` only runs when a PTY reader thread ends (`pty.rs:238`). On app
quit/crash the threads die without it, so `closed_at` stays NULL — 63 of 77 sessions are
orphaned. The summary then counts each as "open until now."

**Fix:** on startup, before any new session is created, reconcile. Add
`db::reconcile_orphans(conn)` and call it in `lib.rs` `.setup()` right after `open_db`:

```sql
UPDATE session
   SET closed_at = COALESCE(
         (SELECT MAX(at) FROM state_event WHERE session_id = session.id),
         created_at)
 WHERE closed_at IS NULL;
```

A session can't outlive the process that owned its PTY, so closing prior-run sessions at
their last-activity time is correct. This alone drops the stale 44 out of *today's*
window.

### 5b. Per-project time SUMS concurrent sessions
`db.rs:452–468` does `SUM(min(closed_at,until) - max(created_at,since))` grouped by
project — so 5 terminals open in the same folder for 1h report 5h. That's the "adding up
all the folders" you noticed.

**Fix:** report the **wall-clock union** of session-open intervals per project, not the
sum. Cleanest in Rust:
`SELECT project, max(created_at,since) AS s, min(COALESCE(closed_at,until),until) AS e
FROM session WHERE <overlaps window>`, then in `daily_summary` group by project, sort
intervals by start, merge overlaps, sum merged durations. (Pure-SQL interval-union is
possible but far less readable.)

**Tests:** update `persists_and_summarizes` (`db.rs:486`) to assert union, and add a
2-overlapping-sessions case proving no double-count.

**Net effect:** 5a fixes the impossible totals (stale sessions leave today's window);
5b fixes concurrent-session inflation. Both are needed.

---

## Suggested order & ownership for parallel agents
1. **Backend/data agent** → Issue 5 (5a + 5b) and Issue 4's `reset_stats` command. These
   share `db.rs` — keep on one agent to avoid conflicts.
2. **Frontend agent A** → Issue 1 (Finder) + Issue 3 (button layout). Both touch
   `TabStrip.tsx` / `App.tsx` / `App.css`.
3. **Frontend agent B** → Issue 2 (resizable panels) — isolated to `Dashboard.tsx`,
   `GitPanel.tsx`, a new hook, `App.css`.

**Decisions to confirm before starting:**
- Issue 3 placement: right-aligned same row (recommended) vs. inside the scrolling strip.
- Issue 4 reset scope: purge-historical (recommended) vs. full-wipe.

---

## Already done (context, not a task)
The WAITING detection accuracy fix is already implemented in
`src-tauri/src/detection/engine.rs`: WAITING now requires a printed prompt pattern (no
more pure-silence flagging of idle long-lived agents). Open follow-up: confirm the
`default_patterns()` list covers the prompt strings of whichever agents are actually run
(Claude Code's "Do you want to…" is covered).
