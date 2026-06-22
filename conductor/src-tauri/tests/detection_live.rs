//! End-to-end moat proof: drive a REAL zsh through a PTY with the real shell-integration
//! applied, capture its actual bytes, and feed them through the REAL DetectionEngine.
//! This closes the gap the unit tests can't: it proves emission + parsing AGREE, including
//! the "command blocked on input -> WAITING" path that is the entire product thesis.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use conductor_lib::db::{self, DbState};
use conductor_lib::detection::{DetectionEngine, Detector};
use conductor_lib::ipc::AttentionState;
use conductor_lib::shell_integration;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// Spawn `shell` with OSC 133 integration, run `commands`, return all PTY bytes captured.
// Real-shell tests share PTYs + CPU; serialize them so timing stays robust under
// cargo's parallel test threads.
static SHELL_LOCK: Mutex<()> = Mutex::new(());

fn run_shell_capture(shell: &str, commands: &[&str], settle_ms: u64) -> Vec<u8> {
    run_shell_capture_env(shell, &[], commands, settle_ms)
}

fn run_shell_capture_env(
    shell: &str,
    env: &[(&str, &str)],
    commands: &[&str],
    settle_ms: u64,
) -> Vec<u8> {
    let _guard = SHELL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    for (k, v) in env {
        cmd.env(k, v);
    }
    shell_integration::apply(&mut cmd, shell); // the real injector under test

    let mut child = pair.slave.spawn_command(cmd).expect("spawn shell");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    let buf = Arc::new(Mutex::new(Vec::new()));
    let buf2 = buf.clone();
    let handle = thread::spawn(move || {
        let mut tmp = [0u8; 4096];
        while let Ok(n) = reader.read(&mut tmp) {
            if n == 0 {
                break;
            }
            buf2.lock().unwrap().extend_from_slice(&tmp[..n]);
        }
    });

    // Readiness: wait for the integration's first prompt marker, so the shell is fully
    // started + config-sourced before we type (deterministic, not a guessed sleep).
    wait_for_marker(&buf, b"\x1b]133;A", 6000);
    for c in commands {
        writer.write_all(c.as_bytes()).unwrap();
        writer.write_all(b"\n").unwrap();
        writer.flush().unwrap();
        // Wait until this command produces output and then goes quiet.
        wait_quiescent(&buf, settle_ms.max(250), 6000);
    }
    // Kill so a blocked shell still EOFs the reader (the `read` case never exits on its own).
    let _ = child.kill();
    let _ = handle.join();
    let out = buf.lock().unwrap().clone();
    out
}

fn buf_contains(buf: &Arc<Mutex<Vec<u8>>>, needle: &[u8]) -> bool {
    buf.lock()
        .unwrap()
        .windows(needle.len())
        .any(|w| w == needle)
}

/// Block until the buffer contains `needle`, or `deadline_ms` elapses.
fn wait_for_marker(buf: &Arc<Mutex<Vec<u8>>>, needle: &[u8], deadline_ms: u64) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(deadline_ms) {
        if buf_contains(buf, needle) {
            return;
        }
        thread::sleep(Duration::from_millis(40));
    }
}

/// Block until the buffer has grown and then stayed quiet for `quiet_ms`, or `deadline_ms`.
fn wait_quiescent(buf: &Arc<Mutex<Vec<u8>>>, quiet_ms: u64, deadline_ms: u64) {
    let start = std::time::Instant::now();
    let initial = buf.lock().unwrap().len();
    let mut last_len = initial;
    let mut last_change = std::time::Instant::now();
    loop {
        thread::sleep(Duration::from_millis(40));
        let len = buf.lock().unwrap().len();
        if len != last_len {
            last_len = len;
            last_change = std::time::Instant::now();
        }
        // Only declare quiet once the command actually produced something.
        if len > initial && last_change.elapsed() >= Duration::from_millis(quiet_ms) {
            break;
        }
        if start.elapsed() >= Duration::from_millis(deadline_ms) {
            break;
        }
    }
}

fn run_zsh_capture(commands: &[&str], settle_ms: u64) -> Vec<u8> {
    run_shell_capture("/bin/zsh", commands, settle_ms)
}

/// Feed captured bytes through the engine and return the distinct state arc + finish flag.
fn engine_arc(bytes: &[u8]) -> (Vec<AttentionState>, bool) {
    let mut e = DetectionEngine::default();
    let evs = e.ingest(&"s".to_string(), bytes, std::time::Instant::now());
    (
        evs.iter().map(|x| x.state).collect(),
        evs.iter().any(|x| x.command_finished == Some(0)),
    )
}

#[test]
fn live_captures_command_text() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("skipping: /bin/zsh not present");
        return;
    }
    let bytes = run_zsh_capture(&["echo conductor-capture-xyz", "exit"], 500);
    let mut e = DetectionEngine::default();
    let evs = e.ingest(&"s".to_string(), &bytes, std::time::Instant::now());
    let texts: Vec<&str> = evs
        .iter()
        .filter_map(|x| x.command_text.as_deref())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("echo conductor-capture-xyz")),
        "command text not captured from real zsh; got {texts:?}"
    );
}

/// Full pipeline: real zsh command -> capture -> persist -> history + suggestion queries.
#[test]
fn live_command_history_and_suggestions() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("skipping: /bin/zsh not present");
        return;
    }
    let bytes = run_zsh_capture(&["echo conductor-hist-zzz", "exit"], 500);

    let db = DbState::memory().unwrap();
    let conn = db.0.lock().unwrap();
    db::insert_session(&conn, "s1", None, "/bin/zsh", 0, 1);

    let mut e = DetectionEngine::default();
    let sid = "s1".to_string();
    let mut t = std::time::Instant::now();
    for chunk in bytes.chunks(64) {
        for ev in e.ingest(&sid, chunk, t) {
            db::insert_state_event(&conn, &ev.session, ev.state, 0, ev.source);
            if ev.command_started {
                db::insert_command_start(&conn, &ev.session, 0);
            }
            if let Some(code) = ev.command_finished {
                db::finish_latest_command(&conn, &ev.session, 0, code, ev.command_text.as_deref());
            }
        }
        t += Duration::from_millis(1);
    }

    let hist = db::command_history(&conn, None, 50);
    assert!(
        hist.iter().any(|h| h.cmdline.contains("echo conductor-hist-zzz")),
        "command not in history: {:?}",
        hist.iter().map(|h| &h.cmdline).collect::<Vec<_>>()
    );
    let sugg = db::command_suggestions(&conn, "echo conductor", None, 5);
    assert!(
        sugg.iter().any(|s| s.contains("conductor-hist-zzz")),
        "prefix suggestion missing: {sugg:?}"
    );
}

#[test]
fn live_bash_captures_command_text() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("skipping: /bin/bash not present");
        return;
    }
    let bytes = run_shell_capture("/bin/bash", &["echo bash-capture-xyz", "exit"], 500);
    let mut e = DetectionEngine::default();
    let evs = e.ingest(&"s".to_string(), &bytes, std::time::Instant::now());
    let texts: Vec<&str> = evs
        .iter()
        .filter_map(|x| x.command_text.as_deref())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("echo bash-capture-xyz")),
        "bash command text not captured; got {texts:?}"
    );
}

#[test]
fn live_bash_command_arc() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("skipping: /bin/bash not present");
        return;
    }
    let bytes = run_shell_capture("/bin/bash", &["echo CONDUCTOR_OK", "exit"], 500);
    let (arc, finished) = engine_arc(&bytes);
    assert!(
        arc.contains(&AttentionState::Running) && arc.contains(&AttentionState::Idle),
        "bash OSC 133 did not drive RUNNING/IDLE: {arc:?}"
    );
    assert!(finished, "bash command never reported finished(0)");
}

/// Regression guard: with a pre-existing `PROMPT_COMMAND`, the bash DEBUG-trap hook must
/// NOT emit spurious OSC 133;C markers (one per real command, not one per prompt).
#[test]
fn live_bash_prompt_command_no_spurious_c() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("skipping: /bin/bash not present");
        return;
    }
    // No `exit` (which would itself be a command); kill ends capture. Two real commands.
    let bytes = run_shell_capture_env(
        "/bin/bash",
        &[("PROMPT_COMMAND", "true")],
        &["echo one", "echo two"],
        500,
    );
    let c_markers = bytes.windows(7).filter(|w| *w == b"\x1b]133;C").count();
    assert_eq!(
        c_markers, 2,
        "expected exactly 2 command-start markers, got {c_markers} (spurious C from PROMPT_COMMAND)"
    );
}

#[test]
fn live_normal_command_runs_then_idles() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("skipping: /bin/zsh not present");
        return;
    }
    let bytes = run_zsh_capture(&["echo CONDUCTOR_OK", "exit"], 500);

    let mut e = DetectionEngine::default();
    let sid = "live".to_string();
    let evs = e.ingest(&sid, &bytes, Instant::now());
    let states: Vec<_> = evs.iter().map(|x| x.state).collect();

    assert!(
        states.contains(&AttentionState::Running),
        "real zsh markers never drove RUNNING: {states:?}"
    );
    assert!(
        states.contains(&AttentionState::Idle),
        "real zsh markers never drove IDLE: {states:?}"
    );
    assert!(
        evs.iter().any(|x| x.command_finished == Some(0)),
        "no command_finished(0) from a real `echo`"
    );
}

#[test]
fn diag_fk_and_orphan_insert() {
    let db = DbState::memory().unwrap();
    let c = db.0.lock().unwrap();
    let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    // Simulate the create_session race: a command/state row written before the session row.
    db::insert_command_start(&c, "ghost", 1);
    db::insert_state_event(
        &c,
        "ghost",
        AttentionState::Running,
        1,
        conductor_lib::ipc::StateSource::Osc133,
    );
    let cmds: i64 = c
        .query_row("SELECT count(*) FROM command", [], |r| r.get(0))
        .unwrap();
    let evs: i64 = c
        .query_row("SELECT count(*) FROM state_event", [], |r| r.get(0))
        .unwrap();
    eprintln!("DIAG foreign_keys={fk} orphan_command_rows={cmds} orphan_state_rows={evs}");
}

/// Deterministic regression guard for the FK-ordering bug: with the session row inserted
/// FIRST (as `create_session` now does), an OSC 133 C->output->D arc — fed in tiny chunks
/// that split markers — must persist exactly one finished command row. (Real-zsh emission
/// is already proven by the other live tests; synthetic bytes keep THIS test timing-free.)
#[test]
fn chunked_command_persists_to_db() {
    let db = DbState::memory().unwrap();
    let conn = db.0.lock().unwrap();
    db::insert_session(&conn, "s1", None, "/bin/zsh", 0, 1); // session row exists first

    let stream: &[u8] = b"\x1b]133;A\x07\x1b]133;C\x07build log line\n\x1b]133;D;0\x07\x1b]133;A\x07";
    let mut e = DetectionEngine::default();
    let sid = "s1".to_string();
    let mut t = std::time::Instant::now();
    for chunk in stream.chunks(4) {
        for ev in e.ingest(&sid, chunk, t) {
            db::insert_state_event(&conn, &ev.session, ev.state, 0, ev.source);
            if ev.command_started {
                db::insert_command_start(&conn, &ev.session, 0);
            }
            if let Some(code) = ev.command_finished {
                db::finish_latest_command(&conn, &ev.session, 0, code, ev.command_text.as_deref());
            }
        }
        t += Duration::from_millis(1);
    }

    let commands: i64 = conn
        .query_row("SELECT count(*) FROM command", [], |r| r.get(0))
        .unwrap();
    let finished: i64 = conn
        .query_row(
            "SELECT count(*) FROM command WHERE finished_at IS NOT NULL AND exit_code = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(commands, 1, "expected exactly one command row");
    assert_eq!(finished, 1, "command not marked finished with exit 0");
}

#[test]
fn live_blocking_read_prompt_goes_waiting() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("skipping: /bin/zsh not present");
        return;
    }
    // A prompted `read` starts a command (OSC 133;C), prints its prompt, then blocks
    // (no OSC 133;D). WAITING now requires that printed prompt — bare silence won't do.
    let bytes = run_zsh_capture(&["read \"?Proceed? (y/n) \" foo"], 1200);

    let mut e = DetectionEngine::default();
    let sid = "live".to_string();
    let t = Instant::now();
    e.ingest(&sid, &bytes, t); // leaves the session mid-command (saw C, no D)

    // After T_wait of silence the matched prompt must flag the blocked command.
    let w = e.tick(t + Duration::from_secs(9));
    let states: Vec<_> = w.iter().map(|x| x.state).collect();
    assert!(
        states.contains(&AttentionState::Waiting),
        "blocked prompted `read` did not become WAITING (got {states:?}); captured {} bytes",
        bytes.len()
    );
}
