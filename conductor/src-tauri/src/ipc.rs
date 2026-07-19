//! FROZEN IPC CONTRACT (Phase 0.5).
//!
//! This is the single source of truth for the Rust<->TS boundary. The TS mirror
//! lives in `src/ipc/types.ts` — keep them in lockstep. Changing anything here is
//! a *blocking* event for all agents (see IMPLEMENTATION-PLAN.md §5). Add fields
//! additively; do not repurpose existing ones.

use serde::{Deserialize, Serialize};

pub type SessionId = String;

/// Per-session attention classification (PRD §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AttentionState {
    Running,
    Idle,
    Waiting,
}

/// Which signal produced a state transition (PRD §8 state_event.source).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateSource {
    Osc133,
    Heuristic,
    Pattern,
}

/// Session metadata surfaced to the dashboard (PRD §6.2 / §8 session).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: SessionId,
    pub name: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub task_label: Option<String>,
    pub shell: String,
    pub created_at: i64,
    pub open_count: i64,
    pub command_count: i64,
    pub state: AttentionState,
    /// Resident memory (KB) of this session's whole tree — the shell plus every
    /// descendant, summed by ppid (not pgid: foreground jobs run in their own group).
    pub rss_kb: Option<i64>,
    /// Most recent command run (drives the auto tab name).
    pub last_command: Option<String>,
}

/// Tauri event channel names (Rust -> frontend). Frontend listens on these.
pub mod events {
    pub const PTY_OUTPUT: &str = "pty://output";
    pub const SESSION_STATE: &str = "session://state";
    pub const SESSION_META: &str = "session://meta";
    pub const SESSION_CLOSED: &str = "session://closed";
    pub const CAP_REACHED: &str = "cap://reached";
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputEvent {
    pub id: SessionId,
    /// Raw PTY bytes, base64-encoded (PTY output is not guaranteed valid UTF-8).
    pub base64_bytes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionClosedEvent {
    pub id: SessionId,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEventPayload {
    pub id: SessionId,
    pub state: AttentionState,
    pub source: StateSource,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapReachedEvent {
    pub limit: usize,
    pub current: usize,
}

/// Daily analytics (PRD §6.6 / §10). Times are milliseconds; the window is [since, until).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub since: i64,
    pub until: i64,
    pub sessions_opened: i64,
    pub commands_run: i64,
    /// Cumulative time sessions spent WAITING — the metric that proves the product's value.
    pub agent_blocked_ms: i64,
    pub cap_overrides: i64,
    pub per_project: Vec<ProjectTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTime {
    pub project: String,
    pub active_ms: i64,
}

/// A deep-work (or break) interval bound to one session — the Focus View timer.
/// v1 has no pause/resume, so `remaining = started_at + planned_ms - now` holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusBlock {
    pub id: String,
    /// None for breaks (a break is not bound to a session).
    pub session_id: Option<SessionId>,
    pub task_label: Option<String>,
    /// "focus" | "break".
    pub kind: String,
    pub started_at: i64,
    pub planned_ms: i64,
    pub ended_at: Option<i64>,
    /// "active" | "completed" | "abandoned".
    pub status: String,
}

/// Rolled-up focus stats for one calendar day, plus the rolling streak.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusDay {
    /// Local-midnight epoch ms that anchors the day window.
    pub date: i64,
    /// Completed focus time within the day (break time excluded).
    pub focus_ms: i64,
    pub blocks_completed: i64,
    pub goal_ms: i64,
    /// Consecutive days (ending on `date`) with at least one completed focus block.
    pub streak_days: i64,
}

/// One past command, for the history view (PRD §5.2 command history).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub cmdline: String,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub exit_code: Option<i32>,
}

/// Persisted session snapshot for restore-on-launch (PRD §8.1).
/// The scrollback BLOB travels separately (not JSON-encoded).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub session_id: SessionId,
    pub name: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub task_label: Option<String>,
    pub shell: String,
    pub cwd: Option<String>,
    pub tab_order: i32,
    pub is_active: bool,
    pub updated_at: i64,
}

/// A snapshot plus its serialized xterm.js scrollback (separate from JSON payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshotWithScrollback {
    pub snapshot: SessionSnapshot,
    /// base64-encoded serialized xterm.js buffer, or null.
    pub scrollback_base64: Option<String>,
}
