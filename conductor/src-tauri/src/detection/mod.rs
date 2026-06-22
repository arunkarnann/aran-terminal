//! Detection engine — the moat (PRD §6.1). Pure module over a byte stream: knows
//! nothing about PTYs, Tauri, or the DB. Terminal-core feeds PTY chunks into
//! `ingest`; a timer drives `tick` for the prompt-quiet (T_wait) heuristic. The engine
//! returns state transitions; the caller persists/emits them.
//!
//! This boundary (IMPLEMENTATION-PLAN.md §2.3) is what lets terminal-core (Agent A)
//! and detection (Agent B/architect) work without editing each other's files.

pub mod engine;
pub mod osc;

pub use engine::{DetectionConfig, DetectionEngine};

use crate::ipc::{AttentionState, SessionId, StateSource};
use std::time::Instant;

/// A detected transition or fact about a session. `at` (wall-clock) is stamped by
/// the caller when it builds the `ipc::StateEventPayload` — the engine works in
/// `Instant` time only, so it stays testable with a synthetic clock.
#[derive(Debug, Clone, PartialEq)]
pub struct StateEvent {
    pub session: SessionId,
    pub state: AttentionState,
    pub source: StateSource,
    /// cwd extracted from OSC 7 / OSC 1337, if seen (drives the `project` field).
    pub cwd: Option<String>,
    /// True on the event marking an OSC 133;C (command execution start).
    pub command_started: bool,
    /// `Some(exit)` on the event marking an OSC 133;D (command finished).
    pub command_finished: Option<i32>,
    /// The command line that just finished (OSC 633;E), paired with `command_finished`.
    pub command_text: Option<String>,
}

pub trait Detector: Send {
    fn ingest(&mut self, session: &SessionId, bytes: &[u8], at: Instant) -> Vec<StateEvent>;
    fn tick(&mut self, now: Instant) -> Vec<StateEvent>;
}

/// No-op detector — the Phase 0 default still wired in `pty.rs` until the live
/// integration step swaps in `DetectionEngine` (after Phase 1 tabs land).
#[derive(Default)]
pub struct NoopDetector;

impl Detector for NoopDetector {
    fn ingest(&mut self, _session: &SessionId, _bytes: &[u8], _at: Instant) -> Vec<StateEvent> {
        Vec::new()
    }
    fn tick(&mut self, _now: Instant) -> Vec<StateEvent> {
        Vec::new()
    }
}
