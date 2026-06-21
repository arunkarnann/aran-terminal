//! Per-session attention state machine (PRD §6.1). Combines three signals:
//!   1. OSC 133 markers (primary, high confidence) — drives RUNNING/IDLE precisely.
//!   2. Silence heuristic — a command that's executing (saw C, no D) and has gone
//!      quiet for `t_wait` is blocked on input → WAITING.
//!   3. Prompt-pattern match — a known blocking question shortens the wait to
//!      `t_wait_pattern` and works even in degraded (no-OSC) mode.
//!
//! Design bias: precision over recall (PRD Principle 3). At an idle shell prompt,
//! silence is NEVER WAITING — only an in-flight command (or a matched pattern in
//! degraded mode) can be.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::osc::{Marker, OscScanner};
use super::{Detector, StateEvent};
use crate::ipc::{AttentionState, SessionId, StateSource};

pub struct DetectionConfig {
    pub t_wait: Duration,
    pub t_wait_pattern: Duration,
    /// Lowercased blocking-question fragments (PRD §6.1 Signal 3). Conservative.
    pub patterns: Vec<String>,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        DetectionConfig {
            t_wait: Duration::from_secs(8),
            t_wait_pattern: Duration::from_secs(2),
            patterns: default_patterns(),
        }
    }
}

fn default_patterns() -> Vec<String> {
    [
        "(y/n)", "[y/n]", "[y/n]", "[y/n]", "(yes/no)", "do you want to",
        "press enter to continue", "press any key", "overwrite?", "continue?",
        "proceed?", "? [y", "? (y", "continue? [", "[y/n]:",
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

        // Recovery: fresh output means the process is alive again — clear WAITING.
        if s.state == AttentionState::Waiting {
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
            let pattern_hit = matches_pattern(&s.tail, &cfg.patterns);

            // Pattern path: works while a command runs OR in degraded (no-OSC) mode.
            let eligible_pattern =
                pattern_hit && silent >= cfg.t_wait_pattern && (s.in_command || !s.has_osc133);
            // Heuristic path: an in-flight command gone quiet. Idle prompts excluded.
            let eligible_heuristic = s.in_command && silent >= cfg.t_wait;

            let source = if eligible_pattern {
                Some(StateSource::Pattern)
            } else if eligible_heuristic {
                Some(StateSource::Heuristic)
            } else {
                None
            };

            if let Some(source) = source {
                s.state = AttentionState::Waiting;
                events.push(ev(id, s.state, source, false, None, None));
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

    // AC2 variant: silent in-flight command with no known pattern -> heuristic WAITING at T_wait.
    #[test]
    fn ac2_silent_command_goes_waiting_via_heuristic() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"working", t + Duration::from_secs(1));
        assert!(e.tick(t + Duration::from_secs(5)).is_empty()); // <8s after last output
        let w = e.tick(t + Duration::from_secs(10));
        assert_eq!(states(&w), vec![AttentionState::Waiting]);
        assert_eq!(w[0].source, StateSource::Heuristic);
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

    // Recovery: new output after WAITING returns the session to RUNNING.
    #[test]
    fn recovery_new_output_clears_waiting() {
        let mut e = engine();
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07", t);
        e.ingest(&SID.into(), b"working", t + Duration::from_secs(1));
        assert_eq!(
            states(&e.tick(t + Duration::from_secs(10))),
            vec![AttentionState::Waiting]
        );
        let back = e.ingest(&SID.into(), b"more output\n", t + Duration::from_secs(11));
        assert_eq!(states(&back), vec![AttentionState::Running]);
    }

    #[test]
    fn t_wait_threshold_is_tunable() {
        let mut e = engine();
        assert_eq!(e.t_wait_secs(), 8);
        e.set_t_wait_secs(20);
        assert_eq!(e.t_wait_secs(), 20);

        // With a 20s threshold, a silent command at +10s must NOT be WAITING yet.
        let t = Instant::now();
        e.ingest(&SID.into(), b"\x1b]133;C\x07working", t);
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
