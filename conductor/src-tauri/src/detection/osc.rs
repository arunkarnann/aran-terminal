//! Incremental OSC / escape scanner (PRD §6.1 Signal 1).
//!
//! Bytes arrive in arbitrary chunks, so a single escape sequence can be split
//! across `feed` calls. `OscScanner` keeps a tiny state machine + partial buffer
//! across calls. It extracts OSC 133 (prompt/command markers) and OSC 7 / 1337
//! (cwd), and strips ANSI/control noise so the leftover printable text can be
//! pattern-matched (Signal 3) without escape codes polluting it.

use base64::Engine;

#[derive(Debug, Clone, PartialEq)]
pub enum Marker {
    PromptStart,            // OSC 133;A
    PromptEnd,              // OSC 133;B
    CommandStart,           // OSC 133;C
    CommandEnd(Option<i32>),// OSC 133;D[;exit]
    Cwd(String),            // OSC 7 (file://host/path) or OSC 1337;CurrentDir=
    CommandText(String),    // OSC 633;E;<base64 of the command line>
}

enum Mode {
    Normal,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

pub struct OscScanner {
    mode: Mode,
    osc: Vec<u8>,
}

impl Default for OscScanner {
    fn default() -> Self {
        OscScanner {
            mode: Mode::Normal,
            osc: Vec::new(),
        }
    }
}

const MAX_OSC: usize = 4096;
const MAX_TAIL: usize = 1024;

impl OscScanner {
    /// Feed a chunk. Appends printable text to `tail` (bounded) and returns markers.
    pub fn feed(&mut self, bytes: &[u8], tail: &mut Vec<u8>) -> Vec<Marker> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.mode {
                Mode::Normal => {
                    if b == 0x1b {
                        self.mode = Mode::Esc;
                    } else {
                        push_printable(tail, b);
                    }
                }
                Mode::Esc => match b {
                    b']' => {
                        self.mode = Mode::Osc;
                        self.osc.clear();
                    }
                    b'[' => self.mode = Mode::Csi,
                    0x1b => {} // consecutive ESC, stay
                    _ => self.mode = Mode::Normal,
                },
                Mode::Csi => {
                    // CSI params run until a final byte in 0x40..=0x7e.
                    if (0x40..=0x7e).contains(&b) {
                        self.mode = Mode::Normal;
                    }
                }
                Mode::Osc => match b {
                    0x07 => {
                        if let Some(m) = parse_osc(&self.osc) {
                            out.push(m);
                        }
                        self.mode = Mode::Normal;
                    }
                    0x1b => self.mode = Mode::OscEsc, // possible ST (ESC \)
                    _ => {
                        if self.osc.len() < MAX_OSC {
                            self.osc.push(b);
                        }
                    }
                },
                Mode::OscEsc => {
                    if b == b'\\' {
                        if let Some(m) = parse_osc(&self.osc) {
                            out.push(m);
                        }
                    }
                    // malformed ESC-in-OSC: drop and resync to Normal
                    self.mode = Mode::Normal;
                }
            }
        }
        out
    }
}

fn push_printable(tail: &mut Vec<u8>, b: u8) {
    if b >= 0x20 || b == b'\n' || b == b'\t' {
        tail.push(b);
        if tail.len() > MAX_TAIL {
            let drop = tail.len() - MAX_TAIL;
            tail.drain(0..drop);
        }
    }
}

fn parse_osc(buf: &[u8]) -> Option<Marker> {
    let s = String::from_utf8_lossy(buf);
    let mut parts = s.split(';');
    match parts.next()? {
        "133" => match parts.next()? {
            "A" => Some(Marker::PromptStart),
            "B" => Some(Marker::PromptEnd),
            "C" => Some(Marker::CommandStart),
            "D" => Some(Marker::CommandEnd(
                parts.next().and_then(|e| e.trim().parse::<i32>().ok()),
            )),
            _ => None,
        },
        "7" => {
            // file://host/path  -> /path
            let url = parts.collect::<Vec<_>>().join(";");
            let rest = url.strip_prefix("file://")?;
            let idx = rest.find('/')?;
            Some(Marker::Cwd(rest[idx..].to_string()))
        }
        "1337" => {
            let rest = parts.collect::<Vec<_>>().join(";");
            rest.strip_prefix("CurrentDir=")
                .map(|p| Marker::Cwd(p.to_string()))
        }
        "633" => match parts.next()? {
            // OSC 633;E;<base64 command line> — captured at precmd from shell history.
            "E" => {
                let b64 = parts.collect::<Vec<_>>().join(";");
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
                let text = String::from_utf8(bytes).ok()?;
                let text = text.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(Marker::CommandText(text.to_string()))
                }
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(input: &[u8]) -> (Vec<Marker>, String) {
        let mut sc = OscScanner::default();
        let mut tail = Vec::new();
        let markers = sc.feed(input, &mut tail);
        (markers, String::from_utf8_lossy(&tail).to_string())
    }

    #[test]
    fn parses_osc133_bel_and_st_terminators() {
        let (m, _) = feed_all(b"\x1b]133;A\x07\x1b]133;C\x1b\\");
        assert_eq!(m, vec![Marker::PromptStart, Marker::CommandStart]);
    }

    #[test]
    fn parses_command_end_with_and_without_exit() {
        let (m, _) = feed_all(b"\x1b]133;D;0\x07\x1b]133;D\x07");
        assert_eq!(
            m,
            vec![Marker::CommandEnd(Some(0)), Marker::CommandEnd(None)]
        );
    }

    #[test]
    fn extracts_cwd_from_osc7_and_osc1337() {
        let (m, _) = feed_all(b"\x1b]7;file://host/Users/arun/proj\x07");
        assert_eq!(m, vec![Marker::Cwd("/Users/arun/proj".into())]);
        let (m, _) = feed_all(b"\x1b]1337;CurrentDir=/tmp/x\x07");
        assert_eq!(m, vec![Marker::Cwd("/tmp/x".into())]);
    }

    #[test]
    fn marker_split_across_two_chunks_is_detected() {
        let mut sc = OscScanner::default();
        let mut tail = Vec::new();
        assert!(sc.feed(b"\x1b]133", &mut tail).is_empty());
        let m = sc.feed(b";C\x07", &mut tail);
        assert_eq!(m, vec![Marker::CommandStart]);
    }

    #[test]
    fn strips_csi_and_keeps_printable_text() {
        let (_, text) = feed_all(b"\x1b[31mProceed? (y/n)\x1b[0m");
        assert_eq!(text, "Proceed? (y/n)");
    }
}
