//! Headless regression guard for the riskiest native path: spawn a PTY, write a
//! command to it, read the output back. Covers PTY spawn/read/write only — NOT the
//! Tauri event emit or the xterm.js render (those need the GUI / a user check).

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[test]
fn pty_echoes_written_command() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    // Use the user's shell the same way pty.rs does.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");

    let mut child = pair.slave.spawn_command(cmd).expect("spawn shell");
    drop(pair.slave); // parent must drop the slave or the reader never sees EOF

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    // A unique marker so we're not matching shell-prompt noise.
    writer
        .write_all(b"echo CONDUCTOR_PTY_OK_4242\n")
        .expect("write");
    writer.flush().expect("flush");

    let mut acc = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                // The echoed *output* line (not just the typed command) confirms the
                // shell executed and the PTY streamed it back.
                if acc.matches("CONDUCTOR_PTY_OK_4242").count() >= 2 {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = writer.write_all(b"exit\n");
    let _ = child.kill();

    assert!(
        acc.contains("CONDUCTOR_PTY_OK_4242"),
        "expected marker in PTY output, got:\n{acc}"
    );
}
