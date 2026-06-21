//! "What flags does this command take?" — runs `<program> --help` and parses the
//! options out, so the UI can show a searchable flag palette for whatever the
//! user typed at the prompt (great for AI-agent CLIs like `claude`).
//!
//! Safety: we run ONLY the first token of the typed line plus `--help`, never the
//! user's own arguments — appending `--help` to e.g. `rm -rf /` could be
//! catastrophic on BSD `rm`. No shell is involved, pagers are disabled (so
//! `git --help` can't hang on a pager), and a watchdog bounds the runtime.

use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpFlag {
    /// The option tokens, e.g. "-h, --help" or "--model <MODEL>".
    pub flags: String,
    pub description: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandHelp {
    pub ok: bool,
    pub command: String,
    pub flags: Vec<HelpFlag>,
    pub raw: String,
    pub error: Option<String>,
}

pub fn command_help(cwd: &str, command: &str) -> CommandHelp {
    let prog = command.split_whitespace().next().unwrap_or("").to_string();
    if prog.is_empty() {
        return CommandHelp {
            ok: false,
            command: command.to_string(),
            flags: Vec::new(),
            raw: String::new(),
            error: Some("no command typed".into()),
        };
    }
    match run_help(cwd, &prog) {
        Ok(text) => {
            let flags = parse_flags(&text);
            CommandHelp {
                ok: true,
                command: prog,
                flags,
                raw: cap(&text),
                error: None,
            }
        }
        Err(e) => CommandHelp {
            ok: false,
            command: prog,
            flags: Vec::new(),
            raw: String::new(),
            error: Some(e),
        },
    }
}

/// Run `<prog> --help` in `cwd` with pagers/color off, bounded by a 6s watchdog.
fn run_help(cwd: &str, prog: &str) -> Result<String, String> {
    let dir = if cwd.is_empty() { ".".to_string() } else { cwd.to_string() };
    let prog_owned = prog.to_string();
    let prog_for_err = prog.to_string();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = Command::new(&prog_owned)
            .arg("--help")
            .current_dir(dir)
            .env("PAGER", "cat")
            .env("GIT_PAGER", "cat")
            .env("MANPAGER", "cat")
            .env("NO_COLOR", "1")
            .output();
        let _ = tx.send(out);
    });

    match rx.recv_timeout(Duration::from_secs(6)) {
        Ok(Ok(o)) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            if s.trim().is_empty() {
                Err(format!("{prog_for_err} printed no help"))
            } else {
                Ok(s)
            }
        }
        Ok(Err(e)) => Err(format!("could not run {prog_for_err}: {e}")),
        Err(_) => Err(format!("{prog_for_err} --help timed out")),
    }
}

fn parse_flags(text: &str) -> Vec<HelpFlag> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw_line in text.lines() {
        // Tabs act as column separators in some help formats.
        let line = raw_line.replace('\t', "  ");
        let t = line.trim_start();
        if !t.starts_with('-') || t.starts_with("---") {
            continue;
        }
        // Need a real option char after the dash(es).
        let after = t.trim_start_matches('-');
        if !after.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) {
            continue;
        }
        let (flags, desc) = match t.find("  ") {
            Some(i) => (t[..i].trim().to_string(), t[i..].trim().to_string()),
            None => (t.trim().to_string(), String::new()),
        };
        if flags.starts_with('-') && seen.insert(flags.clone()) {
            out.push(HelpFlag { flags, description: desc });
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

fn cap(s: &str) -> String {
    const MAX: usize = 600;
    let mut lines: Vec<&str> = s.lines().take(MAX).collect();
    if s.lines().count() > MAX {
        lines.push("… (truncated)");
    }
    lines.join("\n")
}
