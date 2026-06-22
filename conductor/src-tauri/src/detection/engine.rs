//! Per-session attention state machine (PRD §6.1).
//!
//! WAITING requires POSITIVE evidence that the session is blocked on the user — a
//! recognized blocking prompt (e.g. "(y/n)", "do you want to …") that has then gone
//! quiet for `t_wait`. Pure silence is deliberately NOT enough: long-lived
//! interactive programs (AI agents, REPLs, vim) keep the shell's command "in flight"
//! (OSC 133 `C` with no matching `D`) for their entire lifetime and sit quietly
//! whenever they are merely idle — so a silence-only rule flags every idle agent as
//! WAITING. That was the dominant false-positive source.
//!
//! Signals:
//!   1. OSC 133 markers — drive RUNNING/IDLE precisely and tell us whether a command
//!      is in flight, which gates the prompt heuristic so a *bare shell prompt* is
//!      never WAITING (its stale scrollback can't trip a match).
//!   2. Prompt-pattern match — the sole trigger for WAITING. Works while a command
//!      runs and in degraded (no-OSC) mode.
//!
//! Design bias: precision over recall (PRD Principle 3). The known cost is that a
//! command blocked on a `read` with no recognizable prompt text won't be flagged.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::osc::{Marker, OscScanner};
use super::{Detector, StateEvent};
use crate::ipc::{AttentionState, SessionId, StateSource};

pub struct DetectionConfig {
    /// How long a recognized prompt must stay quiet before we flag WAITING.
    pub t_wait: Duration,
    /// Lowercased blocking-question fragments (PRD §6.1 Signal 2). Conservative.
    pub patterns: Vec<String>,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        DetectionConfig {
            t_wait: Duration::from_secs(3),
            patterns: default_patterns(),
        }
    }
}

fn default_patterns() -> Vec<String> {
    [
        "(y/n)", "[y/n]", "[y/n]:", "(y/n/a)", "[y/n/a]", "(yes/no)", "y/n/all",
        "do you want to", "press enter to continue", "press any key",
        "overwrite?", "continue?", "proceed?", "? [y", "? (y", "continue? [",
        "do you trust the files",
    ]
    .iter()
    .map(|s| s.to_lowercase())
    .collect()
}

struct Sess {
    state: AttentionState,
    scanner: OscScanner,
    tail: Vec<u8>,
    has_osc133: bool,
    in_command: bool,
    last_output: Instant,
    pending_command_text: Option<String>,
}

impl Sess {
    fn new(at: Instant) -> Self {
        Sess {
            state: AttentionState::Running,
            scanner: OscScanner::default(),
            tail: Vec::new(),
            has_osc133: false,
            in_command: false,
            last_output: at,
            pending_command_text: None,
        }
    }
}

pub struct DetectionEngine {
    cfg: DetectionConfig,
    sessions: HashMap<SessionId, Sess>,
}

impl Default for DetectionEngine {
    fn default() -> Self {
        DetectionEngine::new(DetectionConfig::default())
    }
}

impl DetectionEngine {
    pub fn new(cfg: DetectionConfig) -> Self {
        DetectionEngine {
            cfg,
            sessions: HashMap::new(),
        }
    }

    pub fn remove(&mut self, session: &SessionId) {
        self.sessions.remove(session);
    }

    /// Tune the silence threshold (PRD §14.1 false-positive tuning). Clamped to >= 1s.
    pub fn set_t_wait_secs(&mut self, secs: u64) {
        self.cfg.t_wait = Duration::from_secs(secs.max(1));
    }

    pub fn t_wait_secs(&self) -> u64 {
        self.cfg.t_wait.as_secs()
    }
}

fn ev(
    session: &SessionId,
    state: AttentionState,
    source: StateSource,
    started: bool,
    finished: Option<i32>,
    cwd: Option<String>,
) -> StateEvent {
    StateEvent {
        session: session.clone(),
        state,
        source,
        cwd,
        command_started: started,
        command_finished: finished,
        command_text: None,
    }
}

impl Detector for DetectionEngine {
    fn ingest(&mut self, session: &SessionId, bytes: &[u8], at: Instant) -> Vec<StateEvent> {
        let s = self
            .sessions
            .entry(session.clone())
            .or_insert_with(|| Sess::new(at));
        let tail_before = s.tail.len();
        s.last_output = at;

        let markers = s.scanner.feed(bytes, &mut s.tail);
        let mut events = Vec::new();
        let mut cwd_seen: Option<String> = None;

        for m in markers {
            match m {
                Marker::PromptStart => {
                    s.has_osc133 = true;
                    s.in_command = false;
                    if s.state != AttentionState::Idle {
                        s.state = AttentionState::Idle;
                        events.push(ev(session, s.state, StateSource::Osc133, false, None, None));
                    }
                }
                Marker::PromptEnd => {
                    s.has_osc133 = true;
                }
                Marker::CommandStart => {
                    s.has_osc133 = true;
                    s.in_command = true;
                    s.state = AttentionState::Running;
                    let text = s.pending_command_text.take();
                    let mut e = ev(session, s.state, StateSource::Osc133, true, None, None);
                    e.command_text = text;
                    events.push(e);
                }
                Marker::CommandEnd(code) => {
                    s.has_osc133 = true;
                    // A `D` with no preceding `C` (e.g. the first prompt after shell
                    // startup) is not a real command finish — don't report an exit.
                    let was_running = s.in_command;
                    s.in_command = false;
                    let text = s.pending_command_text.take();
                    let changed = s.state != AttentionState::Idle;
                    s.state = AttentionState::Idle;
                    if was_running || changed {
                        let finished = if was_running { Some(code.unwrap_or(0)) } else { None };
                        let mut e = ev(session, s.state, StateSource::Osc133, false, finished, None);
                        if was_running {
                            e.command_text = text;
                        }
                        events.push(e);
                    }
                }
                Marker::CommandText(t) => s.pending_command_text = Some(t),
                Marker::Cwd(p) => cwd_seen = Some(p),
            }
        }

        // Recovery: fresh *printable* output means the process is alive again — clear
        // WAITING. Guard on printable growth so cursor-blink / redraw escape sequences
        // (which carry no new content) don't flap the state back and forth.
        if s.state == AttentionState::Waiting && s.tail.len() > tail_before {
            s.state = AttentionState::Running;
            events.push(ev(session, s.state, StateSource::Heuristic, false, None, None));
        }

        // Surface a cwd change even when state didn't move (drives `project`).
        if let Some(cwd) = cwd_seen {
            if let Some(last) = events.last_mut() {
                last.cwd = Some(cwd);
            } else {
                events.push(ev(session, s.state, StateSource::Osc133, false, None, Some(cwd)));
            }
        }

        events
    }

    fn tick(&mut self, now: Instant) -> Vec<StateEvent> {
        let cfg = &self.cfg;
        let mut events = Vec::new();
        for (id, s) in self.sessions.iter_mut() {
            if s.state == AttentionState::Waiting {
                continue;
            }
            let silent = now.saturating_duration_since(s.last_output);

            // WAITING needs positive evidence: a recognized blocking prompt that has
            // then gone quiet for `t_wait`. Pure silence is never enough (see module
            // docs). The `in_command || !has_osc133` clause keeps a bare shell prompt
            // out — at an idle prompt with OSC we're not in a command, so stale
            // scrollback can't trip a match.
            let eligible = matches_pattern(&s.tail, &cfg.patterns)
                && silent >= cfg.t_wait
                && (s.in_command || !s.has_osc133);

            if eligible {
                s.state = AttentionState::Waiting;
                // Consume the matched prompt so an already-answered question (or a
                // stale match in degraded mode) can't immediately re-trigger after
                // recovery.
                s.tail.clear();
                events.push(ev(id, s.state, StateSource::Pattern, false, None, None));
            }
        }
        events
    }
}

fn matches_pattern(tail: &[u8], patterns: &[String]) -> bool {
    let n = tail.len().min(256);
    let recent = String::from_utf8_lossy(&tail[tail.len() - n..]).to_lowercase();
    patterns.iter().any(|p| recent.contains(p.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "s1";

    fn engine() -> DetectionEngine {
        DetectionEngine::default()
    }

    fn states(evs: &[StateEvent]) -> Vec<AttentionState> {
        evs.iter().map(|e| e.state).collect()
    }

    // AC1: a long command finishing -> IDLE (immediately) with exit code.
    #[test]
    fn ac1_command_finish_goes_idle_with_exit() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t); // command starts
        e.ingest(&SID.into(), b"build output...\n", t + Duration::from_secs(1));
        let fin = e.ingest(&SID.into(), b"\x1b]133;D;0\x07", t + Duration::from_secs(3));
        assert_eq!(states(&fin), vec![AttentionState::Idle]);
        assert_eq!(fin[0].command_finished, Some(0));
    }

    // AC2: agent prints a y/n prompt and stops -> WAITING within T_wait.
    #[test]
    fn ac2_blocking_prompt_goes_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t); // agent launched (long command)
        e.ingest(&SID.into(), b"Proceed? (y/n) ", t + Duration::from_secs(1));
        // Pattern shortens the wait: WAITING by t+1+2s.
        let w = e.tick(t + Duration::from_secs(4));
        assert_eq!(states(&w), vec![AttentionState::Waiting]);
        assert_eq!(w[0].source, StateSource::Pattern);
    }

    // The reported bug: a silent in-flight command with NO recognized prompt must NOT
    // be flagged. Long-lived interactive programs (agents/REPLs/vim) keep a command
    // in flight and sit quietly while merely idle — silence alone is never WAITING.
    #[test]
    fn silent_in_flight_command_without_prompt_is_not_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"working", t + Duration::from_secs(1));
        // Even far past any threshold, no prompt means no WAITING.
        assert!(e.tick(t + Duration::from_secs(120)).is_empty());
    }

    // The same bug from the app's angle: an AI agent runs as a long-lived in-flight
    // command (saw C, never D). Idle silence stays RUNNING; only a printed question
    // flips it to WAITING.
    #[test]
    fn idle_long_lived_agent_only_waits_on_a_prompt() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t); // `claude` launches; stays in-flight
        e.ingest(&SID.into(), b"working on it\n", t + Duration::from_secs(1));
        // 5 minutes idle: still RUNNING (no prompt printed).
        assert!(e.tick(t + Duration::from_secs(300)).is_empty());
        // Now it asks a real question.
        e.ingest(&SID.into(), b"Do you want to proceed? ", t + Duration::from_secs(301));
        let w = e.tick(t + Duration::from_secs(305));
        assert_eq!(states(&w), vec![AttentionState::Waiting]);
        assert_eq!(w[0].source, StateSource::Pattern);
    }

    // AC3: a session streaming continuous output is never marked WAITING.
    #[test]
    fn ac3_continuous_output_never_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        for i in 1..30 {
            e.ingest(&SID.into(), b"chunk\n", t + Duration::from_secs(i));
            assert!(e.tick(t + Duration::from_secs(i)).is_empty());
        }
    }

    // AC4: idle at a shell prompt -> silence must NOT produce WAITING (false-positive guard).
    #[test]
    fn ac4_idle_prompt_silence_is_not_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"\x1b]133;D;0\x07", t + Duration::from_secs(1)); // back to prompt
        assert!(e.tick(t + Duration::from_secs(30)).is_empty());
    }

    // AC4 guard: a stale pattern in scrollback while idle at prompt must not trip WAITING.
    #[test]
    fn ac4_stale_pattern_at_idle_prompt_is_not_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"Proceed? (y/n) y\n", t + Duration::from_secs(1));
        e.ingest(&SID.into(), b"\x1b]133;D;0\x07", t + Duration::from_secs(2)); // answered, now idle
        assert!(e.tick(t + Duration::from_secs(30)).is_empty());
    }

    // AC5: degraded mode (no OSC 133). A matched prompt still flags WAITING...
    #[test]
    fn ac5_degraded_pattern_goes_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"Overwrite? (y/n) ", t);
        let w = e.tick(t + Duration::from_secs(3));
        assert_eq!(states(&w), vec![AttentionState::Waiting]);
        assert_eq!(w[0].source, StateSource::Pattern);
    }

    // ...but degraded mode without a pattern stays conservative (no WAITING) to protect FPs.
    #[test]
    fn ac5_degraded_no_pattern_stays_conservative() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"some plain output with no question", t);
        assert!(e.tick(t + Duration::from_secs(30)).is_empty());
    }

    // Recovery: real printable output after WAITING returns the session to RUNNING.
    #[test]
    fn recovery_new_output_clears_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"Proceed? (y/n) ", t + Duration::from_secs(1));
        assert_eq!(
            states(&e.tick(t + Duration::from_secs(5))),
            vec![AttentionState::Waiting]
        );
        let back = e.ingest(&SID.into(), b"more output\n", t + Duration::from_secs(6));
        assert_eq!(states(&back), vec![AttentionState::Running]);
    }

    // Recovery ignores content-free escape sequences (e.g. cursor blink) so a blocked
    // prompt doesn't flap between WAITING and RUNNING.
    #[test]
    fn recovery_ignores_escape_only_output() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"Proceed? (y/n) ", t + Duration::from_secs(1));
        assert_eq!(
            states(&e.tick(t + Duration::from_secs(5))),
            vec![AttentionState::Waiting]
        );
        // A bare cursor-move CSI carries no new content: stays WAITING.
        let back = e.ingest(&SID.into(), b"\x1b[2G", t + Duration::from_secs(6));
        assert!(back.is_empty());
    }

    #[test]
    fn t_wait_threshold_is_tunable() {
        let mut e = engine();
        assert_eq!(e.t_wait_secs(), 3);
        e.set_t_wait_secs(20);
        assert_eq!(e.t_wait_secs(), 20);

        // With a 20s threshold, a prompt quiet for only 10s must NOT be WAITING yet.
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07Proceed? (y/n) ", t);
        assert!(e.tick(t + Duration::from_secs(10)).is_empty());
        let w = e.tick(t + Duration::from_secs(21));
        assert_eq!(states(&w), vec![AttentionState::Waiting]);

        e.set_t_wait_secs(0); // clamps to >= 1
        assert_eq!(e.t_wait_secs(), 1);
    }

    #[test]
    fn cwd_marker_is_surfaced() {
        let mut e = engine();
        let t = Instant::now();
        let evs = e.ingest(&SID.into(), b"\x1b]7;file://host/Users/arun/p\x07", t);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].cwd, Some("/Users/arun/p".into()));
    }
}
