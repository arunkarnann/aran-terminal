//! Persistence (PRD §6.6, §8). SQLite via rusqlite (WAL). One `Mutex<Connection>` shared
//! across the reader threads + ticker; writes are infrequent (transitions/commands, never
//! per-byte) so serializing them is fine.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::ipc::{AttentionState, FocusBlock, FocusDay, HistoryEntry, ProjectTime, StateSource, Summary};

pub struct DbState(pub Arc<Mutex<Connection>>);

impl DbState {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        init(&conn)?;
        Ok(DbState(Arc::new(Mutex::new(conn))))
    }

    /// In-memory fallback so the app still runs if the data dir is unwritable (and for tests).
    pub fn memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(DbState(Arc::new(Mutex::new(conn))))
    }
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(include_str!("../migrations/0001_init.sql"))?;
    // Focus View — deep-work timers (additive migration).
    conn.execute_batch(include_str!("../migrations/0002_focus.sql"))?;
    // Seeded suggestions learned from the user's existing shell history (auto-learn).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hist_seed (cmdline TEXT PRIMARY KEY, freq INTEGER NOT NULL DEFAULT 1);",
    )
}

/// True if we've already imported the user's shell history.
pub fn is_seeded(conn: &Connection) -> bool {
    conn.query_row("SELECT COUNT(*) FROM hist_seed", [], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(true)
}

/// Import command lines (most-recent first) into the seed table, counting frequency.
pub fn seed_history(conn: &Connection, lines: &[String]) {
    let _ = conn.execute_batch("BEGIN");
    for line in lines {
        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }
        let _ = conn.execute(
            "INSERT INTO hist_seed (cmdline, freq) VALUES (?1, 1) \
             ON CONFLICT(cmdline) DO UPDATE SET freq = freq + 1",
            params![cmd],
        );
    }
    let _ = conn.execute_batch("COMMIT");
}

fn state_str(s: AttentionState) -> &'static str {
    match s {
        AttentionState::Running => "RUNNING",
        AttentionState::Idle => "IDLE",
        AttentionState::Waiting => "WAITING",
    }
}

fn source_str(s: StateSource) -> &'static str {
    match s {
        StateSource::Osc133 => "osc133",
        StateSource::Heuristic => "heuristic",
        StateSource::Pattern => "pattern",
    }
}

pub fn insert_session(
    conn: &Connection,
    id: &str,
    name: Option<&str>,
    shell: &str,
    created_at: i64,
    open_count: i64,
) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO session (id, name, shell, created_at, open_count) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, shell, created_at, open_count],
    );
}

pub fn mark_session_closed(conn: &Connection, id: &str, closed_at: i64) {
    let _ = conn.execute(
        "UPDATE session SET closed_at = ?2 WHERE id = ?1 AND closed_at IS NULL",
        params![id, closed_at],
    );
}

/// On startup, close any sessions that were left open (app quit/crash without cleanup).
/// Uses the last state_event timestamp, or created_at as fallback.
pub fn reconcile_orphans(conn: &Connection) {
    let _ = conn.execute(
        "UPDATE session \
         SET closed_at = COALESCE( \
               (SELECT MAX(at) FROM state_event WHERE session_id = session.id), \
               created_at) \
         WHERE closed_at IS NULL",
        [],
    );
}

pub fn update_project(conn: &Connection, id: &str, path: &str, name: Option<&str>) {
    let _ = conn.execute(
        "UPDATE session SET project_path = ?2, project_name = ?3 WHERE id = ?1",
        params![id, path, name],
    );
    if let Some(n) = name {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO project_alias (path, friendly_name) VALUES (?1, ?2)",
            params![path, n],
        );
    }
}

pub fn set_task_label(conn: &Connection, id: &str, label: &str) {
    let _ = conn.execute(
        "UPDATE session SET task_label = ?2 WHERE id = ?1",
        params![id, label],
    );
}

pub fn insert_state_event(
    conn: &Connection,
    session_id: &str,
    state: AttentionState,
    at: i64,
    source: StateSource,
) {
    let _ = conn.execute(
        "INSERT INTO state_event (session_id, state, at, source) VALUES (?1, ?2, ?3, ?4)",
        params![session_id, state_str(state), at, source_str(source)],
    );
}

pub fn insert_command_start(conn: &Connection, session_id: &str, at: i64) {
    let _ = conn.execute(
        "INSERT INTO command (session_id, started_at) VALUES (?1, ?2)",
        params![session_id, at],
    );
}

/// Close the most recent unfinished command for a session, recording its text + exit.
pub fn finish_latest_command(
    conn: &Connection,
    session_id: &str,
    at: i64,
    exit_code: i32,
    cmdline: Option<&str>,
) {
    let _ = conn.execute(
        "UPDATE command SET finished_at = ?2, exit_code = ?3, cmdline = COALESCE(?4, cmdline) \
         WHERE id = (SELECT id FROM command WHERE session_id = ?1 AND finished_at IS NULL \
                     ORDER BY started_at DESC LIMIT 1)",
        params![session_id, at, exit_code, cmdline],
    );
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, value],
    );
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

// ---- Focus View (deep-work timers) ----

const DAY_MS: i64 = 86_400_000;

const FOCUS_COLS: &str =
    "id, session_id, task_label, kind, started_at, planned_ms, ended_at, status";

fn row_to_focus_block(r: &rusqlite::Row) -> rusqlite::Result<FocusBlock> {
    Ok(FocusBlock {
        id: r.get(0)?,
        session_id: r.get(1)?,
        task_label: r.get(2)?,
        kind: r.get(3)?,
        started_at: r.get(4)?,
        planned_ms: r.get(5)?,
        ended_at: r.get(6)?,
        status: r.get(7)?,
    })
}

/// Insert a new active focus (or break) block.
pub fn insert_focus_block(
    conn: &Connection,
    id: &str,
    session_id: Option<&str>,
    task_label: Option<&str>,
    kind: &str,
    started_at: i64,
    planned_ms: i64,
) {
    let _ = conn.execute(
        "INSERT INTO focus_block (id, session_id, task_label, kind, started_at, planned_ms, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active')",
        params![id, session_id, task_label, kind, started_at, planned_ms],
    );
}

/// End a block: `status` is "completed" or "abandoned". No-op if already ended.
pub fn end_focus_block(conn: &Connection, id: &str, ended_at: i64, status: &str) {
    let _ = conn.execute(
        "UPDATE focus_block SET ended_at = ?2, status = ?3 WHERE id = ?1 AND status = 'active'",
        params![id, ended_at, status],
    );
}

/// Lengthen an active block's planned duration (the "+5 min" control).
pub fn extend_focus_block(conn: &Connection, id: &str, add_ms: i64) {
    let _ = conn.execute(
        "UPDATE focus_block SET planned_ms = planned_ms + ?2 WHERE id = ?1 AND status = 'active'",
        params![id, add_ms],
    );
}

/// The single active block, if any (rehydrates the Focus view across reloads).
pub fn active_focus_block(conn: &Connection) -> Option<FocusBlock> {
    conn.query_row(
        &format!("SELECT {FOCUS_COLS} FROM focus_block WHERE status = 'active' ORDER BY started_at DESC LIMIT 1"),
        [],
        row_to_focus_block,
    )
    .ok()
}

pub fn get_focus_block(conn: &Connection, id: &str) -> Option<FocusBlock> {
    conn.query_row(
        &format!("SELECT {FOCUS_COLS} FROM focus_block WHERE id = ?1"),
        params![id],
        row_to_focus_block,
    )
    .ok()
}

/// True if `[day_start, day_start + DAY_MS)` holds at least one completed focus block.
fn day_has_completed_focus(conn: &Connection, day_start: i64) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM focus_block \
         WHERE kind = 'focus' AND status = 'completed' \
           AND started_at >= ?1 AND started_at < ?2",
        params![day_start, day_start + DAY_MS],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Daily focus rollup + rolling streak. `day_start` is the local-midnight epoch ms
/// computed by the caller (the frontend, which knows the timezone).
pub fn focus_day(conn: &Connection, day_start: i64, goal_ms: i64) -> FocusDay {
    let day_end = day_start + DAY_MS;

    // Completed focus time within the day, clipped to the window AND to the planned
    // duration. The planned-duration clamp guards against a block that was left running
    // when the app quit: on relaunch it completes with an `ended_at` hours past its start,
    // which would otherwise credit hours of phantom focus.
    let focus_ms = conn
        .query_row(
            "SELECT COALESCE(SUM( \
                 min(COALESCE(ended_at, ?2), ?2, started_at + planned_ms) - max(started_at, ?1) \
             ), 0) \
             FROM focus_block \
             WHERE kind = 'focus' AND status = 'completed' \
               AND started_at < ?2 AND COALESCE(ended_at, ?2) > ?1",
            params![day_start, day_end],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let blocks_completed = conn
        .query_row(
            "SELECT COUNT(*) FROM focus_block \
             WHERE kind = 'focus' AND status = 'completed' \
               AND started_at >= ?1 AND started_at < ?2",
            params![day_start, day_end],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Streak: consecutive days ending on `day_start` with >=1 completed focus block.
    // If today has none yet, count the run up to yesterday (don't break mid-day).
    let mut streak_days = 0i64;
    let mut d = if day_has_completed_focus(conn, day_start) {
        day_start
    } else {
        day_start - DAY_MS
    };
    while day_has_completed_focus(conn, d) {
        streak_days += 1;
        d -= DAY_MS;
        if streak_days >= 366 {
            break;
        }
    }

    FocusDay {
        date: day_start,
        focus_ms,
        blocks_completed,
        goal_ms,
        streak_days,
    }
}

/// `started_at` of the most recent unfinished command (to time a finish for notifications).
pub fn latest_open_command_started_at(conn: &Connection, session_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT started_at FROM command WHERE session_id = ?1 AND finished_at IS NULL \
         ORDER BY started_at DESC LIMIT 1",
        params![session_id],
        |r| r.get::<_, i64>(0),
    )
    .ok()
}

/// How many sessions have ever carried this name (for the reopen counter, PRD §5.2).
pub fn count_sessions_named(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM session WHERE name = ?1",
        params![name],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Escape LIKE wildcards in a user-supplied prefix (used with `ESCAPE '\'`).
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Purge all historical data (closed sessions, events, commands, focus blocks).
/// Keeps currently-open sessions so live terminals aren't affected.
pub fn reset_stats(conn: &Connection) {
    let _ = conn.execute_batch(
        "DELETE FROM state_event; \
         DELETE FROM command; \
         DELETE FROM focus_block; \
         DELETE FROM session WHERE closed_at IS NOT NULL;",
    );
}

/// Recent commands for the history view, newest first, optionally filtered by substring.
pub fn command_history(conn: &Connection, search: Option<&str>, limit: i64) -> Vec<HistoryEntry> {
    let like = search.map(|s| format!("%{}%", escape_like(s)));
    let mut stmt = match conn.prepare(
        "SELECT c.cmdline, s.project_name, s.project_path, c.finished_at, \
                (c.finished_at - c.started_at) AS dur, c.exit_code \
         FROM command c JOIN session s ON s.id = c.session_id \
         WHERE c.cmdline IS NOT NULL \
           AND (?1 IS NULL OR c.cmdline LIKE ?1 ESCAPE '\\') \
         ORDER BY c.started_at DESC LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![like, limit], |r| {
        Ok(HistoryEntry {
            cmdline: r.get(0)?,
            project: r.get(1).ok(),
            cwd: r.get(2).ok(),
            finished_at: r.get(3).ok(),
            duration_ms: r.get(4).ok(),
            exit_code: r.get(5).ok(),
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Distinct command lines starting with `prefix`, ranked by same-cwd, frequency, recency.
/// Used by the auto keyboard fill (PRD §5.2). Excludes the prefix itself.
pub fn command_suggestions(
    conn: &Connection,
    prefix: &str,
    cwd: Option<&str>,
    limit: i64,
) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let pat = format!("{}%", escape_like(prefix));
    let mut stmt = match conn.prepare(
        "SELECT c.cmdline, \
                SUM(CASE WHEN s.project_path = ?2 THEN 1 ELSE 0 END) AS cwd_hits, \
                COUNT(*) AS freq, MAX(c.started_at) AS recent \
         FROM command c JOIN session s ON s.id = c.session_id \
         WHERE c.cmdline IS NOT NULL AND c.cmdline LIKE ?1 ESCAPE '\\' AND c.cmdline <> ?3 \
         GROUP BY c.cmdline \
         ORDER BY (cwd_hits > 0) DESC, freq DESC, recent DESC LIMIT ?4",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![pat, cwd, prefix, limit], |r| r.get::<_, String>(0));
    let mut out: Vec<String> = rows.map(|r| r.flatten().collect()).unwrap_or_default();

    // Fill remaining slots from the seeded shell history (auto-learned), most-used first.
    if (out.len() as i64) < limit {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT cmdline FROM hist_seed \
             WHERE cmdline LIKE ?1 ESCAPE '\\' AND cmdline <> ?2 \
             ORDER BY freq DESC, length(cmdline) ASC LIMIT ?3",
        ) {
            if let Ok(rows) =
                stmt.query_map(params![pat, prefix, limit], |r| r.get::<_, String>(0))
            {
                for c in rows.flatten() {
                    if out.len() as i64 >= limit {
                        break;
                    }
                    if !out.contains(&c) {
                        out.push(c);
                    }
                }
            }
        }
    }
    out
}

/// Merge overlapping intervals and return total wall-clock duration.
fn merge_intervals(intervals: &[(i64, i64)]) -> i64 {
    if intervals.is_empty() {
        return 0;
    }
    let mut sorted: Vec<(i64, i64)> = intervals.to_vec();
    sorted.sort_unstable();
    let mut total = 0i64;
    let (mut cur_s, mut cur_e) = sorted[0];
    for &(s, e) in &sorted[1..] {
        if s <= cur_e {
            cur_e = cur_e.max(e);
        } else {
            total += cur_e - cur_s;
            cur_s = s;
            cur_e = e;
        }
    }
    total += cur_e - cur_s;
    total
}

/// Per-project wall-clock time using interval union (no double-counting of concurrent sessions).
fn compute_per_project_union(conn: &Connection, since: i64, until: i64) -> Vec<ProjectTime> {
    let mut stmt = match conn.prepare(
        "SELECT COALESCE(project_name, 'Unknown') AS p, \
                max(created_at, ?1) AS s, \
                min(COALESCE(closed_at, ?2), ?2) AS e \
         FROM session \
         WHERE created_at < ?2 AND COALESCE(closed_at, ?2) > ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![since, until], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    });
    let rows: Vec<(String, i64, i64)> = match rows {
        Ok(r) => r.flatten().collect(),
        Err(_) => return Vec::new(),
    };
    let mut map: std::collections::HashMap<String, Vec<(i64, i64)>> =
        std::collections::HashMap::new();
    for (project, s, e) in rows {
        if e > s {
            map.entry(project).or_default().push((s, e));
        }
    }
    let mut out: Vec<ProjectTime> = map
        .into_iter()
        .map(|(project, intervals)| ProjectTime {
            project,
            active_ms: merge_intervals(&intervals),
        })
        .collect();
    out.sort_unstable_by(|a, b| b.active_ms.cmp(&a.active_ms));
    out
}

pub fn daily_summary(conn: &Connection, since: i64, until: i64, cap_overrides: i64) -> Summary {
    let sessions_opened = conn
        .query_row(
            "SELECT COUNT(*) FROM session WHERE created_at >= ?1 AND created_at < ?2",
            params![since, until],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let commands_run = conn
        .query_row(
            "SELECT COUNT(*) FROM command WHERE started_at >= ?1 AND started_at < ?2",
            params![since, until],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Sum WAITING intervals clipped to the window. Open WAITING (no following event) counts
    // up to `until`. Uses LEAD() to find each event's successor per session.
    let agent_blocked_ms = conn
        .query_row(
            "WITH ev AS ( \
               SELECT session_id, state, at, \
                      LEAD(at) OVER (PARTITION BY session_id ORDER BY at) AS next_at \
               FROM state_event \
             ) \
             SELECT COALESCE(SUM(min(COALESCE(next_at, ?2), ?2) - max(at, ?1)), 0) \
             FROM ev \
             WHERE state = 'WAITING' AND at < ?2 AND COALESCE(next_at, ?2) > ?1",
            params![since, until],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let per_project = compute_per_project_union(conn, since, until);

    Summary {
        since,
        until,
        sessions_opened,
        commands_run,
        agent_blocked_ms,
        cap_overrides,
        per_project,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_summarizes() {
        let db = DbState::memory().unwrap();
        let c = db.0.lock().unwrap();

        insert_session(&c, "s1", Some("agent"), "/bin/zsh", 1_000, 1);
        update_project(&c, "s1", "/Users/a/proj", Some("proj"));
        insert_command_start(&c, "s1", 1_100);
        finish_latest_command(&c, "s1", 1_500, 0, Some("npm run build"));
        // WAITING from 2_000 to 5_000 = 3_000 ms of agent-blocked time.
        insert_state_event(&c, "s1", AttentionState::Waiting, 2_000, StateSource::Heuristic);
        insert_state_event(&c, "s1", AttentionState::Running, 5_000, StateSource::Heuristic);

        let s = daily_summary(&c, 0, 10_000, 2);
        assert_eq!(s.sessions_opened, 1);
        assert_eq!(s.commands_run, 1);
        assert_eq!(s.agent_blocked_ms, 3_000);
        assert_eq!(s.cap_overrides, 2);
        assert_eq!(s.per_project.len(), 1);
        assert_eq!(s.per_project[0].project, "proj");
        assert_eq!(count_sessions_named(&c, "agent"), 1);
    }

    #[test]
    fn settings_round_trip() {
        let db = DbState::memory().unwrap();
        let c = db.0.lock().unwrap();
        assert_eq!(get_setting(&c, "t_wait_secs"), None);
        set_setting(&c, "t_wait_secs", "12");
        assert_eq!(get_setting(&c, "t_wait_secs").as_deref(), Some("12"));
        set_setting(&c, "t_wait_secs", "5"); // REPLACE, not duplicate
        assert_eq!(get_setting(&c, "t_wait_secs").as_deref(), Some("5"));
    }

    #[test]
    fn focus_blocks_roll_up_and_streak() {
        let db = DbState::memory().unwrap();
        let c = db.0.lock().unwrap();
        let day: i64 = 1_000 * DAY_MS; // some local midnight, day index 1000
        insert_session(&c, "s1", None, "/bin/zsh", 0, 1); // focus blocks reference a session

        // Two completed focus blocks today: 25min + 50min.
        insert_focus_block(&c, "f1", Some("s1"), Some("plan"), "focus", day + 1, 25 * 60_000);
        end_focus_block(&c, "f1", day + 1 + 25 * 60_000, "completed");
        insert_focus_block(&c, "f2", Some("s1"), None, "focus", day + 2_000_000, 50 * 60_000);
        end_focus_block(&c, "f2", day + 2_000_000 + 50 * 60_000, "completed");
        // An abandoned block doesn't count.
        insert_focus_block(&c, "f3", Some("s1"), None, "focus", day + 9_000_000, 25 * 60_000);
        end_focus_block(&c, "f3", day + 9_100_000, "abandoned");
        // A break doesn't count toward focus time.
        insert_focus_block(&c, "b1", None, None, "break", day + 10_000_000, 5 * 60_000);
        end_focus_block(&c, "b1", day + 10_300_000, "completed");
        // Yesterday had a completed focus block -> streak should be 2.
        insert_focus_block(&c, "y1", Some("s1"), None, "focus", day - DAY_MS + 5, 25 * 60_000);
        end_focus_block(&c, "y1", day - DAY_MS + 5 + 25 * 60_000, "completed");

        // A block left running when the app quit: completes on relaunch with an `ended_at`
        // hours past its start. focus_ms must clamp it to its 25-minute planned duration,
        // not credit the 3-hour gap.
        insert_focus_block(&c, "stale", Some("s1"), None, "focus", day + 12_000_000, 25 * 60_000);
        end_focus_block(&c, "stale", day + 12_000_000 + 3 * 60 * 60_000, "completed");

        let fd = focus_day(&c, day, 120 * 60_000);
        assert_eq!(fd.focus_ms, (25 + 50 + 25) * 60_000); // stale clamped to 25m, not 3h
        assert_eq!(fd.blocks_completed, 3);
        assert_eq!(fd.goal_ms, 120 * 60_000);
        assert_eq!(fd.streak_days, 2);

        // Active block rehydration.
        insert_focus_block(&c, "live", Some("s1"), None, "focus", day + 20_000_000, 25 * 60_000);
        let active = active_focus_block(&c).unwrap();
        assert_eq!(active.id, "live");
        assert_eq!(active.status, "active");
        extend_focus_block(&c, "live", 5 * 60_000);
        assert_eq!(get_focus_block(&c, "live").unwrap().planned_ms, 30 * 60_000);
    }

    #[test]
    fn open_waiting_counts_to_window_end() {
        let db = DbState::memory().unwrap();
        let c = db.0.lock().unwrap();
        insert_session(&c, "s1", None, "/bin/zsh", 0, 1);
        // WAITING with no following event -> counts until `until`.
        insert_state_event(&c, "s1", AttentionState::Waiting, 4_000, StateSource::Pattern);
        let s = daily_summary(&c, 0, 10_000, 0);
        assert_eq!(s.agent_blocked_ms, 6_000);
    }

    #[test]
    fn overlapping_sessions_union_not_sum() {
        let db = DbState::memory().unwrap();
        let c = db.0.lock().unwrap();
        // Two sessions in the same project, overlapping: [0, 10000) and [2000, 8000).
        insert_session(&c, "a", None, "/bin/zsh", 0, 1);
        update_project(&c, "a", "/p", Some("proj"));
        mark_session_closed(&c, "a", 10_000);
        insert_session(&c, "b", None, "/bin/zsh", 2_000, 1);
        update_project(&c, "b", "/p", Some("proj"));
        mark_session_closed(&c, "b", 8_000);

        let s = daily_summary(&c, 0, 10_000, 0);
        assert_eq!(s.per_project.len(), 1);
        // Union of [0,10000) and [2000,8000) = [0,10000) = 10000ms (not 16000).
        assert_eq!(s.per_project[0].active_ms, 10_000);
    }
}
