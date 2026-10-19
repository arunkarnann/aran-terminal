//! Command autocomplete at the shell prompt (v2's "auto keyboard fill", PRD §5.2).
//!
//! The typed command line is read back from the terminal grid: when the first key
//! is pressed at a fresh prompt we remember the cursor cell (the end of the prompt);
//! the input is then everything between that anchor and the cursor. That stays right
//! through Tab completion and ↑/↓ history recall, which a keystroke buffer can't
//! follow. Keys that move the cursor away from the end of the line (←, Home, ⌃A,
//! ⌃R …) make the line "dirty" and suggestions stay off until the next prompt.
//!
//! Suggestions come from `db::command_suggestions` (commands run in Aran, ranked by
//! same folder → frequency → recency, then the seeded shell history).

use eframe::egui::{Key, Modifiers};

pub const MAX_SUGGESTIONS: i64 = 6;
/// Shortest input that triggers suggestions.
const MIN_INPUT: usize = 2;

/// A grid cell in absolute coordinates (screen line + scrollback size), so it stays
/// put while the screen scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub line: i64,
    pub col: usize,
}

/// What a key press should do while suggestions are showing.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Not ours: forward to the shell as usual.
    Pass,
    /// Handled (panel navigation / dismiss): don't forward.
    Consumed,
    /// Type this text (the rest of the chosen suggestion) into the shell.
    Accept(String),
}

#[derive(Default)]
pub struct Suggest {
    anchor: Option<Anchor>,
    dirty: bool,
    /// Current input as last read from the grid.
    input: String,
    /// The input `items` were fetched for.
    queried: Option<String>,
    items: Vec<String>,
    sel: usize,
    panel: bool,
    dismissed: bool,
}

impl Suggest {
    /// Forget everything (new prompt, command started, Enter, ⌃C).
    pub fn reset(&mut self) {
        *self = Suggest::default();
    }

    pub fn anchor(&self) -> Option<Anchor> {
        self.anchor
    }

    /// Called before bytes reach the shell. `cursor` is where the cursor is right now
    /// (before the shell echoes the key), used to anchor the line on the first key.
    pub fn feed(&mut self, bytes: &[u8], cursor: Anchor) {
        match classify(bytes) {
            Input::Submit => self.reset(),
            Input::Edit => {
                if self.anchor.is_none() && !self.dirty {
                    self.anchor = Some(cursor);
                }
                self.dismissed = false;
            }
            Input::Move => {
                self.dirty = true;
                self.hide();
            }
        }
    }

    /// A paste: single-line text edits the line, anything multi-line leaves it alone.
    pub fn feed_paste(&mut self, text: &str, cursor: Anchor) {
        if text.contains(['\n', '\r']) {
            self.dirty = true;
            self.hide();
        } else {
            self.feed(b"x", cursor);
        }
    }

    /// Update the input from the grid. Returns the prefix to look up when it changed
    /// and is long enough; `None` means keep the current items.
    pub fn sync(&mut self, line: Option<String>) -> Option<String> {
        if self.dirty {
            return None;
        }
        let Some(line) = line else {
            self.hide();
            return None;
        };
        if line != self.input {
            self.input = line;
            self.dismissed = false;
        }
        if self.input.trim().chars().count() < MIN_INPUT {
            self.hide();
            return None;
        }
        (self.queried.as_deref() != Some(self.input.as_str())).then(|| self.input.clone())
    }

    /// Results for `prefix` (stale results for an older input are ignored).
    pub fn set_items(&mut self, prefix: &str, items: Vec<String>) {
        if prefix != self.input {
            return;
        }
        self.queried = Some(prefix.to_string());
        self.items = items.into_iter().filter(|s| s.starts_with(prefix) && s.len() > prefix.len()).collect();
        self.sel = 0;
        if self.items.is_empty() {
            self.panel = false;
        }
    }

    fn hide(&mut self) {
        self.items.clear();
        self.queried = None;
        self.sel = 0;
        self.panel = false;
    }

    pub fn visible(&self) -> bool {
        !self.dirty && !self.dismissed && !self.items.is_empty()
    }

    pub fn panel_open(&self) -> bool {
        self.visible() && self.panel
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.sel
    }

    /// Greyed-out rest of the selected suggestion, drawn after the cursor.
    pub fn ghost(&self) -> Option<&str> {
        if !self.visible() {
            return None;
        }
        self.items.get(self.sel).and_then(|s| s.strip_prefix(self.input.as_str())).filter(|g| !g.is_empty())
    }

    /// → accepts · ↓/↑ walk the list · Tab/Enter accept while the list is open · Esc hides.
    pub fn on_key(&mut self, key: Key, m: Modifiers) -> KeyAction {
        if !self.visible() || m.any() {
            return KeyAction::Pass;
        }
        match key {
            Key::ArrowDown => {
                if self.panel {
                    self.sel = (self.sel + 1).min(self.items.len().saturating_sub(1));
                } else {
                    self.panel = true;
                }
                KeyAction::Consumed
            }
            Key::ArrowUp if self.panel => {
                if self.sel > 0 {
                    self.sel -= 1;
                } else {
                    self.panel = false;
                }
                KeyAction::Consumed
            }
            Key::ArrowRight => self.accept(self.sel),
            Key::Tab | Key::Enter if self.panel => self.accept(self.sel),
            Key::Escape => {
                self.dismissed = true;
                self.panel = false;
                KeyAction::Consumed
            }
            _ => KeyAction::Pass,
        }
    }

    /// Accept item `i` (also used for mouse clicks in the list).
    pub fn accept(&mut self, i: usize) -> KeyAction {
        let Some(rest) = self.items.get(i).and_then(|s| s.strip_prefix(self.input.as_str())).map(str::to_string) else {
            return KeyAction::Pass;
        };
        if rest.is_empty() {
            return KeyAction::Pass;
        }
        self.input.push_str(&rest);
        // Don't immediately re-suggest for the accepted line.
        self.queried = Some(self.input.clone());
        self.items.clear();
        self.panel = false;
        self.sel = 0;
        KeyAction::Accept(rest)
    }
}

enum Input {
    /// Ends the line: Enter, ⌃C, ⌃U.
    Submit,
    /// Changes the text but leaves the cursor at the end: typing, Backspace, Tab
    /// completion, ↑/↓ history, →/End (zsh accepts its own suggestion), ⌃W, ⌃E, ⌃K.
    Edit,
    /// Anything that may move the cursor off the end of the line.
    Move,
}

fn classify(bytes: &[u8]) -> Input {
    const KEEP_ESC: [&[u8]; 8] = [b"\x1b[A", b"\x1bOA", b"\x1b[B", b"\x1bOB", b"\x1b[C", b"\x1bOC", b"\x1b[F", b"\x1bOF"];
    match bytes {
        b"\r" | b"\n" | [0x03] | [0x15] => Input::Submit,
        [0x7f] | [0x08] | b"\t" | [0x17] | [0x05] | [0x0b] => Input::Edit,
        [0x1b, ..] if KEEP_ESC.contains(&bytes) => Input::Edit,
        [0x1b, ..] => Input::Move,
        _ if !bytes.is_empty() && bytes.iter().all(|&b| b >= 0x20) => Input::Edit,
        _ => Input::Move,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C0: Anchor = Anchor { line: 10, col: 4 };

    fn with_items(input: &str, items: &[&str]) -> Suggest {
        let mut s = Suggest::default();
        s.feed(b"g", C0);
        assert_eq!(s.sync(Some(input.into())).as_deref(), Some(input));
        s.set_items(input, items.iter().map(|x| x.to_string()).collect());
        s
    }

    #[test]
    fn anchors_on_first_key_only() {
        let mut s = Suggest::default();
        s.feed(b"g", C0);
        s.feed(b"i", Anchor { line: 10, col: 5 });
        assert_eq!(s.anchor(), Some(C0));
        s.feed(b"\r", C0);
        assert_eq!(s.anchor(), None);
    }

    #[test]
    fn ghost_and_right_arrow_accept() {
        let mut s = with_items("git s", &["git status", "git stash", "git s"]);
        assert_eq!(s.items().len(), 2, "the prefix itself is dropped");
        assert_eq!(s.ghost(), Some("tatus"));
        assert_eq!(s.on_key(Key::ArrowRight, Modifiers::NONE), KeyAction::Accept("tatus".into()));
        assert!(!s.visible());
        // Same line read back from the grid → no re-query.
        assert_eq!(s.sync(Some("git status".into())), None);
    }

    #[test]
    fn panel_navigation() {
        let mut s = with_items("ca", &["cargo build", "cargo test", "cat x"]);
        // Enter/Tab go to the shell while the list is closed.
        assert_eq!(s.on_key(Key::Enter, Modifiers::NONE), KeyAction::Pass);
        assert_eq!(s.on_key(Key::Tab, Modifiers::NONE), KeyAction::Pass);
        assert_eq!(s.on_key(Key::ArrowDown, Modifiers::NONE), KeyAction::Consumed);
        assert!(s.panel_open());
        s.on_key(Key::ArrowDown, Modifiers::NONE);
        s.on_key(Key::ArrowDown, Modifiers::NONE);
        s.on_key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(s.selected(), 2, "clamped at the last item");
        s.on_key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(s.on_key(Key::Tab, Modifiers::NONE), KeyAction::Accept("rgo test".into()));
    }

    #[test]
    fn up_at_top_closes_then_passes() {
        let mut s = with_items("ca", &["cargo build"]);
        s.on_key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(s.on_key(Key::ArrowUp, Modifiers::NONE), KeyAction::Consumed);
        assert!(!s.panel_open());
        assert_eq!(s.on_key(Key::ArrowUp, Modifiers::NONE), KeyAction::Pass, "↑ is shell history again");
    }

    #[test]
    fn escape_dismisses_until_input_changes() {
        let mut s = with_items("ca", &["cargo build"]);
        assert_eq!(s.on_key(Key::Escape, Modifiers::NONE), KeyAction::Consumed);
        assert!(!s.visible());
        s.feed(b"r", C0);
        assert_eq!(s.sync(Some("car".into())).as_deref(), Some("car"));
        s.set_items("car", vec!["cargo build".into()]);
        assert!(s.visible());
    }

    #[test]
    fn cursor_moves_make_it_dirty_until_submit() {
        let mut s = with_items("ca", &["cargo build"]);
        s.feed(b"\x1b[D", C0); // ←
        assert!(!s.visible());
        assert_eq!(s.sync(Some("ca".into())), None);
        s.feed(b"\r", C0);
        s.feed(b"c", C0);
        assert_eq!(s.sync(Some("ca".into())).as_deref(), Some("ca"));
    }

    #[test]
    fn tab_and_history_keep_tracking() {
        let mut s = with_items("ca", &["cargo build"]);
        s.feed(b"\t", C0);
        s.feed(b"\x1b[A", C0);
        assert_eq!(s.sync(Some("cargo".into())).as_deref(), Some("cargo"));
    }

    #[test]
    fn modifiers_pass_through() {
        let mut s = with_items("ca", &["cargo build"]);
        let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
        assert_eq!(s.on_key(Key::ArrowRight, ctrl), KeyAction::Pass);
    }

    #[test]
    fn short_input_and_stale_results() {
        let mut s = Suggest::default();
        s.feed(b"g", C0);
        assert_eq!(s.sync(Some("g".into())), None);
        assert_eq!(s.sync(Some("gi".into())).as_deref(), Some("gi"));
        s.feed(b"t", C0);
        s.sync(Some("git".into()));
        s.set_items("gi", vec!["gist".into()]);
        assert!(!s.visible(), "results for an older input are ignored");
    }

    #[test]
    fn multiline_paste_is_dirty() {
        let mut s = with_items("ca", &["cargo build"]);
        s.feed_paste("a\nb", C0);
        assert!(!s.visible());
    }
}
