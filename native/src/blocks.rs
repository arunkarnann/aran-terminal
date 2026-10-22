//! Warp-style command blocks over the alacritty grid — port of the Tauri build's
//! `lib/blocks.ts`.
//!
//! A block = one shell prompt + the command typed at it + that command's output,
//! delimited by the OSC 133 markers the shell integration emits (A prompt, C command
//! start, D command end; 633;E carries the command line, OSC 7 the cwd).
//!
//! xterm.js gave the Tauri build line markers that followed the scrollback. Here a
//! block records *absolute* line numbers instead: `scrolled` (lines ever pushed into
//! history, dropped ones included) plus the cursor's screen line. `TermHandler` wraps
//! the terminal's escape handler and keeps `scrolled` exact by mirroring every path
//! through which alacritty scrolls the primary grid (linefeeds, wrapping, SU, DL,
//! `clear`). A block's grid line is then `abs - scrolled`, valid until it drops out
//! of history.
//!
//! Alternate-screen apps (vim, less, Claude Code's TUI) suspend block creation: the
//! launching command stays one block whose output is the whole TUI session.

use std::collections::VecDeque;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, GridCell};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    self, Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, NamedPrivateMode, PrivateMode, Rgb,
    ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};
use base64::Engine;

/// Blocks kept per session (older ones are forgotten first).
const MAX_BLOCKS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    PromptStart,
    CommandStart,
    CommandEnd(Option<i32>),
    CommandText(String),
    Cwd(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Prompt shown, no command run yet (the live input line).
    Active,
    Running,
    Success,
    Error,
    /// Session/command ended without a clean exit code.
    Aborted,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Active => "prompt",
            Status::Running => "running",
            Status::Success => "exit 0",
            Status::Error => "failed",
            Status::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: u64,
    pub command: Option<String>,
    /// Absolute line of the prompt (133;A) — the visual top of the block.
    pub prompt: i64,
    /// Absolute line where output begins (just after 133;C).
    pub output: Option<i64>,
    /// Absolute line of command completion (133;D) — exclusive end of the output.
    pub end: Option<i64>,
    pub exit: Option<i32>,
    pub status: Status,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub cwd: Option<String>,
}

pub struct Blocks {
    /// Lines ever scrolled off the top of the screen into history (dropped ones too).
    /// Screen line 0 is absolute line `scrolled`.
    pub scrolled: i64,
    pub list: VecDeque<Block>,
    pending_cmd: Option<String>,
    pending_cwd: Option<String>,
    next_id: u64,
    /// Mirror of alacritty's scroll region (start, end-exclusive), in screen lines.
    region: (i32, i32),
    rows: i32,
}

impl Blocks {
    pub fn new(rows: usize) -> Self {
        Blocks {
            scrolled: 0,
            list: VecDeque::new(),
            pending_cmd: None,
            pending_cwd: None,
            next_id: 1,
            region: (0, rows as i32),
            rows: rows as i32,
        }
    }

    fn current(&mut self) -> Option<&mut Block> {
        self.list.back_mut()
    }

    /// Apply a marker seen with the cursor on screen line `cursor_line`.
    pub fn on_marker(&mut self, m: Marker, cursor_line: i32, alt_screen: bool, now: i64) {
        let abs = self.scrolled + cursor_line as i64;
        match m {
            Marker::CommandText(t) => self.pending_cmd = Some(t),
            Marker::Cwd(p) => self.pending_cwd = Some(p),
            Marker::PromptStart => {
                if alt_screen {
                    return;
                }
                // A prompt while a command is still "running" means we missed its D.
                if let Some(cur) = self.current() {
                    if cur.status == Status::Running {
                        cur.status = Status::Aborted;
                        cur.finished_at = Some(now);
                    }
                }
                let id = self.next_id;
                self.next_id += 1;
                self.list.push_back(Block {
                    id,
                    command: None,
                    prompt: abs,
                    output: None,
                    end: None,
                    exit: None,
                    status: Status::Active,
                    started_at: 0,
                    finished_at: None,
                    cwd: self.pending_cwd.clone(),
                });
                while self.list.len() > MAX_BLOCKS {
                    self.list.pop_front();
                }
            }
            Marker::CommandStart => {
                if alt_screen {
                    return;
                }
                let pending = self.pending_cmd.take();
                let cwd = self.pending_cwd.clone();
                if let Some(cur) = self.current() {
                    if pending.is_some() {
                        cur.command = pending;
                    }
                    cur.output = Some(abs);
                    cur.status = Status::Running;
                    cur.started_at = now;
                    if cwd.is_some() {
                        cur.cwd = cwd;
                    }
                }
            }
            Marker::CommandEnd(exit) => {
                let pending = self.pending_cmd.take();
                // A D with no running command is shell-startup noise (no preceding C).
                if let Some(cur) = self.current().filter(|c| c.status == Status::Running) {
                    cur.end = Some(abs);
                    cur.exit = exit;
                    cur.status = match exit {
                        None => Status::Aborted,
                        Some(0) => Status::Success,
                        Some(_) => Status::Error,
                    };
                    cur.finished_at = Some(now);
                    if cur.command.is_none() {
                        cur.command = pending;
                    }
                }
            }
        }
    }

    /// Forget blocks whose prompt line has left the scrollback.
    pub fn prune(&mut self, history_size: usize) {
        let oldest = self.scrolled - history_size as i64;
        while self.list.front().is_some_and(|b| b.prompt < oldest) {
            self.list.pop_front();
        }
    }

    /// Everything after a terminal reset or a reflow is unanchored.
    pub fn clear(&mut self) {
        self.list.clear();
        self.pending_cmd = None;
    }

    /// Grid line for an absolute line number.
    pub fn to_line(&self, abs: i64) -> Line {
        Line((abs - self.scrolled) as i32)
    }

    /// Absolute line shown at the top of the viewport for a display offset.
    pub fn top_abs(&self, display_offset: usize) -> i64 {
        self.scrolled - display_offset as i64
    }

    /// Called from `Session::resize` after the grid was resized: rows moving between
    /// history and screen shift the origin; a column change reflows every line, so the
    /// anchors are dropped.
    pub fn on_resize(&mut self, cols_changed: bool, rows: usize, hist_before: usize, hist_after: usize) {
        self.rows = rows as i32;
        self.region = (0, self.rows);
        self.scrolled += hist_after as i64 - hist_before as i64;
        if cols_changed {
            self.clear();
        }
    }

    fn set_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom.unwrap_or(self.rows as usize);
        if top >= bottom {
            return;
        }
        let start = (top as i32 - 1).min(self.rows);
        let end = (bottom as i32).min(self.rows);
        self.region = (start, end);
    }

    /// A scroll-up of `n` lines whose region starts at `origin`: history only grows
    /// when the region starts at the top of the screen (alacritty `Grid::scroll_up`).
    fn note_scroll(&mut self, origin: i32, n: usize, alt_screen: bool) {
        if alt_screen || origin != 0 || n == 0 {
            return;
        }
        self.scrolled += n as i64;
    }
}

// ---------------------------------------------------------------- byte scanner

#[derive(Default)]
enum ScanMode {
    #[default]
    Normal,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// Finds complete OSC 133 / 633;E / 7 sequences in a byte stream (state survives
/// chunk boundaries) and reports where each one ends.
#[derive(Default)]
pub struct MarkerScanner {
    mode: ScanMode,
    osc: Vec<u8>,
}

const MAX_OSC: usize = 4096;

impl MarkerScanner {
    /// Split `bytes` at marker boundaries: each segment is fed to the terminal, then
    /// its marker (if any) is applied with the cursor exactly where the shell had it.
    pub fn split<'a>(&mut self, bytes: &'a [u8]) -> Vec<(&'a [u8], Option<Marker>)> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let mut done: Option<Option<Marker>> = None;
            match self.mode {
                ScanMode::Normal => {
                    if b == 0x1b {
                        self.mode = ScanMode::Esc;
                    }
                }
                ScanMode::Esc => match b {
                    b']' => {
                        self.mode = ScanMode::Osc;
                        self.osc.clear();
                    }
                    b'[' => self.mode = ScanMode::Csi,
                    0x1b => {}
                    _ => self.mode = ScanMode::Normal,
                },
                ScanMode::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.mode = ScanMode::Normal;
                    }
                }
                ScanMode::Osc => match b {
                    0x07 => {
                        done = Some(parse_osc(&self.osc));
                        self.mode = ScanMode::Normal;
                    }
                    0x1b => self.mode = ScanMode::OscEsc,
                    _ => {
                        if self.osc.len() < MAX_OSC {
                            self.osc.push(b);
                        }
                    }
                },
                ScanMode::OscEsc => {
                    if b == b'\\' {
                        done = Some(parse_osc(&self.osc));
                    }
                    self.mode = ScanMode::Normal;
                }
            }
            if let Some(Some(m)) = done {
                out.push((&bytes[start..=i], Some(m)));
                start = i + 1;
            }
        }
        if start < bytes.len() || out.is_empty() {
            out.push((&bytes[start..], None));
        }
        out
    }
}

fn parse_osc(buf: &[u8]) -> Option<Marker> {
    let s = String::from_utf8_lossy(buf);
    let mut parts = s.split(';');
    match parts.next()? {
        "133" => match parts.next()? {
            "A" => Some(Marker::PromptStart),
            "C" => Some(Marker::CommandStart),
            "D" => Some(Marker::CommandEnd(parts.next().and_then(|e| e.trim().parse().ok()))),
            _ => None,
        },
        "7" => {
            let url = parts.collect::<Vec<_>>().join(";");
            let rest = url.strip_prefix("file://")?;
            let idx = rest.find('/')?;
            Some(Marker::Cwd(rest[idx..].to_string()))
        }
        "633" => {
            if parts.next()? != "E" {
                return None;
            }
            let b64 = parts.collect::<Vec<_>>().join(";");
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
            let text = String::from_utf8(bytes).ok()?;
            let text = text.trim();
            (!text.is_empty()).then(|| Marker::CommandText(text.to_string()))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- text extraction

/// The output rows of a block as text (trailing blank lines trimmed). Empty while the
/// markers are gone or the command hasn't started.
pub fn output_text<L: EventListener>(term: &Term<L>, blocks: &Blocks, b: &Block) -> String {
    let Some(out) = b.output else { return String::new() };
    let grid = term.grid();
    let hist = grid.history_size() as i64;
    let rows = grid.screen_lines() as i64;
    let lo = blocks.scrolled - hist;
    let hi = blocks.scrolled + rows - 1;
    // D lands on the line of the next prompt, so the output ends one line above it.
    let end = b.end.map(|e| e - 1).unwrap_or(hi).min(hi);
    let start = out.max(lo);
    if end < start {
        return String::new();
    }
    let cols = grid.columns();
    let text = term.bounds_to_string(
        Point::new(blocks.to_line(start), Column(0)),
        Point::new(blocks.to_line(end), Column(cols.saturating_sub(1))),
    );
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Command + output, the way Warp's "copy block" works.
pub fn full_text<L: EventListener>(term: &Term<L>, blocks: &Blocks, b: &Block) -> String {
    let cmd = b.command.clone().unwrap_or_default();
    let out = output_text(term, blocks, b);
    if out.is_empty() {
        cmd
    } else {
        format!("{cmd}\n{out}")
    }
}

// ---------------------------------------------------------------- handler wrapper

/// The terminal's escape handler, with scroll accounting on top.
pub struct TermHandler<'a, L: EventListener> {
    term: &'a mut Term<L>,
    blocks: &'a mut Blocks,
}

impl<'a, L: EventListener> TermHandler<'a, L> {
    pub fn new(term: &'a mut Term<L>, blocks: &'a mut Blocks) -> Self {
        TermHandler { term, blocks }
    }

    fn alt(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn cursor(&self) -> (i32, usize) {
        let c = &self.term.grid().cursor;
        (c.point.line.0, c.point.column.0)
    }

    /// A linefeed scrolls when the cursor sits on the last line of the scroll region.
    fn note_linefeed(&mut self) {
        let (line, _) = self.cursor();
        let (start, end) = self.blocks.region;
        if line + 1 == end {
            let alt = self.alt();
            self.blocks.note_scroll(start, 1, alt);
        }
    }

    /// Lines `clear` (ED 2) pushes into history: alacritty's `Grid::clear_viewport`
    /// scrolls by the index of the last non-empty screen line + 1.
    fn clear_all_scroll(&self) -> usize {
        let grid = self.term.grid();
        let rows = grid.screen_lines() as i32;
        let cols = grid.columns();
        for l in (0..rows).rev() {
            let row = &grid[Line(l)];
            if (0..cols).any(|c| !row[Column(c)].is_empty()) {
                return (l + 1) as usize;
            }
        }
        // Nothing on screen: the backwards iterator stops at the first history cell
        // (no scroll) — or, with no history at all, on screen line 0.
        if grid.history_size() > 0 {
            0
        } else {
            1
        }
    }
}

macro_rules! forward {
    ($( fn $name:ident ( $( $arg:ident : $ty:ty ),* ) ; )*) => {
        $( #[inline] fn $name(&mut self $(, $arg: $ty)*) { self.term.$name($($arg),*) } )*
    };
}

impl<L: EventListener> Handler for TermHandler<'_, L> {
    fn input(&mut self, c: char) {
        // Writing past the right margin wraps (an implicit linefeed) — detect it from
        // the cursor jumping back to the start of the line.
        let (line0, col0) = self.cursor();
        self.term.input(c);
        let (line1, col1) = self.cursor();
        let (start, end) = self.blocks.region;
        if col1 < col0 && line1 == line0 && line0 + 1 == end {
            let alt = self.alt();
            self.blocks.note_scroll(start, 1, alt);
        }
    }

    fn linefeed(&mut self) {
        self.note_linefeed();
        self.term.linefeed();
    }

    fn newline(&mut self) {
        self.note_linefeed();
        self.term.newline();
    }

    fn scroll_up(&mut self, n: usize) {
        let (start, end) = self.blocks.region;
        let lines = n.min((end - start).max(0) as usize);
        let alt = self.alt();
        self.blocks.note_scroll(start, lines, alt);
        self.term.scroll_up(n);
    }

    fn delete_lines(&mut self, n: usize) {
        let (line, _) = self.cursor();
        let (start, end) = self.blocks.region;
        let rows = self.blocks.rows;
        let lines = n.min((rows - line).max(0) as usize).min((end - start).max(0) as usize);
        if lines > 0 && line >= start && line < end {
            let alt = self.alt();
            self.blocks.note_scroll(line, lines, alt);
        }
        self.term.delete_lines(n);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        if matches!(mode, ClearMode::All) && !self.alt() {
            let n = self.clear_all_scroll();
            self.blocks.note_scroll(0, n, false);
        }
        self.term.clear_screen(mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        self.blocks.set_region(top, bottom);
        self.term.set_scrolling_region(top, bottom);
    }

    fn reset_state(&mut self) {
        self.term.reset_state();
        self.blocks.region = (0, self.blocks.rows);
        self.blocks.clear();
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if matches!(mode, PrivateMode::Named(NamedPrivateMode::ColumnMode)) {
            self.blocks.region = (0, self.blocks.rows);
        }
        self.term.set_private_mode(mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if matches!(mode, PrivateMode::Named(NamedPrivateMode::ColumnMode)) {
            self.blocks.region = (0, self.blocks.rows);
        }
        self.term.unset_private_mode(mode);
    }

    forward! {
        fn set_title(t: Option<String>);
        fn set_cursor_style(s: Option<CursorStyle>);
        fn set_cursor_shape(s: CursorShape);
        fn goto(l: i32, c: usize);
        fn goto_line(l: i32);
        fn goto_col(c: usize);
        fn insert_blank(n: usize);
        fn move_up(n: usize);
        fn move_down(n: usize);
        fn identify_terminal(i: Option<char>);
        fn device_status(n: usize);
        fn move_forward(n: usize);
        fn move_backward(n: usize);
        fn move_down_and_cr(n: usize);
        fn move_up_and_cr(n: usize);
        fn put_tab(n: u16);
        fn backspace();
        fn carriage_return();
        fn bell();
        fn substitute();
        fn set_horizontal_tabstop();
        fn scroll_down(n: usize);
        fn insert_blank_lines(n: usize);
        fn erase_chars(n: usize);
        fn delete_chars(n: usize);
        fn move_backward_tabs(n: u16);
        fn move_forward_tabs(n: u16);
        fn save_cursor_position();
        fn restore_cursor_position();
        fn clear_line(m: LineClearMode);
        fn clear_tabs(m: TabulationClearMode);
        fn set_tabs(i: u16);
        fn reverse_index();
        fn terminal_attribute(a: Attr);
        fn set_mode(m: Mode);
        fn unset_mode(m: Mode);
        fn report_mode(m: Mode);
        fn report_private_mode(m: PrivateMode);
        fn set_keypad_application_mode();
        fn unset_keypad_application_mode();
        fn set_active_charset(i: CharsetIndex);
        fn configure_charset(i: CharsetIndex, c: StandardCharset);
        fn set_color(i: usize, c: Rgb);
        fn dynamic_color_sequence(p: String, i: usize, t: &str);
        fn reset_color(i: usize);
        fn clipboard_store(c: u8, d: &[u8]);
        fn clipboard_load(c: u8, t: &str);
        fn decaln();
        fn push_title();
        fn pop_title();
        fn text_area_size_pixels();
        fn text_area_size_chars();
        fn set_hyperlink(h: Option<Hyperlink>);
        fn set_mouse_cursor_icon(i: ansi::cursor_icon::CursorIcon);
        fn report_keyboard_mode();
        fn push_keyboard_mode(m: KeyboardModes);
        fn pop_keyboard_modes(n: u16);
        fn set_keyboard_mode(m: KeyboardModes, b: KeyboardModesApplyBehavior);
        fn set_modify_other_keys(m: ModifyOtherKeys);
        fn report_modify_other_keys();
        fn set_scp(p: ScpCharPath, u: ScpUpdateMode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    struct Harness {
        term: Term<VoidListener>,
        blocks: Blocks,
        scanner: MarkerScanner,
        parser: Processor,
    }

    impl Harness {
        fn new(cols: usize, rows: usize, history: usize) -> Self {
            let cfg = Config { scrolling_history: history, ..Config::default() };
            Harness {
                term: Term::new(cfg, &TermSize::new(cols, rows), VoidListener),
                blocks: Blocks::new(rows),
                scanner: MarkerScanner::default(),
                parser: Processor::new(),
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            for (seg, marker) in self.scanner.split(bytes) {
                if !seg.is_empty() {
                    self.parser.advance(&mut TermHandler::new(&mut self.term, &mut self.blocks), seg);
                }
                if let Some(m) = marker {
                    let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
                    let line = self.term.grid().cursor.point.line.0;
                    self.blocks.on_marker(m, line, alt, 1);
                }
            }
        }

        /// Below the history cap, our count must equal alacritty's history exactly.
        fn check(&self) {
            assert_eq!(self.blocks.scrolled, self.term.grid().history_size() as i64, "scroll accounting drifted");
        }
    }

    #[test]
    fn scanner_splits_at_markers_and_survives_chunks() {
        let mut sc = MarkerScanner::default();
        let parts = sc.split(b"ab\x1b]133;A\x07cd\x1b]13");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], (&b"ab\x1b]133;A\x07"[..], Some(Marker::PromptStart)));
        assert_eq!(parts[1], (&b"cd\x1b]13"[..], None));
        let parts = sc.split(b"3;D;1\x1b\\x");
        assert_eq!(parts[0].1, Some(Marker::CommandEnd(Some(1))));
        assert_eq!(parts[1], (&b"x"[..], None));
        let parts = sc.split(b"\x1b]633;E;bHMgLWxh\x07\x1b]7;file://h/tmp/x\x07");
        assert_eq!(parts[0].1, Some(Marker::CommandText("ls -la".into())));
        assert_eq!(parts[1].1, Some(Marker::Cwd("/tmp/x".into())));
    }

    #[test]
    fn counts_every_way_the_grid_scrolls() {
        let mut h = Harness::new(10, 4, 1000);
        // Plain newlines past the bottom.
        h.feed(b"a\r\nb\r\nc\r\nd\r\ne\r\nf");
        h.check();
        assert_eq!(h.blocks.scrolled, 2);
        // Auto-wrap of a long line.
        h.feed(b"0123456789012345678901234");
        h.check();
        // `clear` (cursor home + ED 2) pushes the used lines into history.
        h.feed(b"\x1b[H\x1b[2J");
        h.check();
        // CSI scroll-up and delete-lines at the top.
        h.feed(b"x\r\ny\x1b[3S");
        h.check();
        h.feed(b"\x1b[H\x1b[2M");
        h.check();
        // A sub-region never touches history.
        let before = h.blocks.scrolled;
        h.feed(b"\x1b[2;3r\x1b[3;1Hq\r\nw\r\ne\r\nr\x1b[r");
        h.check();
        assert_eq!(h.blocks.scrolled, before);
        // Alt screen: nothing counts.
        h.feed(b"\x1b[?1049h");
        for _ in 0..10 {
            h.feed(b"tui\r\n");
        }
        h.feed(b"\x1b[?1049l");
        h.check();
        // RIS clears everything.
        h.feed(b"\x1bc");
        assert_eq!(h.term.grid().history_size(), 0);
        assert!(h.blocks.list.is_empty());
    }

    #[test]
    fn blocks_follow_prompt_command_output_markers() {
        let mut h = Harness::new(40, 5, 1000);
        h.feed(b"\x1b]133;A\x07$ ");
        h.feed(b"ls\r\n\x1b]633;E;bHM=\x07\x1b]133;C\x07a.txt\r\nb.txt\r\n");
        h.feed(b"\x1b]633;E;bHM=\x07\x1b]133;D;0\x07\x1b]133;A\x07$ ");
        assert_eq!(h.blocks.list.len(), 2);
        let b = &h.blocks.list[0];
        assert_eq!(b.command.as_deref(), Some("ls"));
        assert_eq!(b.status, Status::Success);
        assert_eq!((b.prompt, b.output, b.end), (0, Some(1), Some(3)));
        assert_eq!(output_text(&h.term, &h.blocks, b), "a.txt\nb.txt");
        assert_eq!(full_text(&h.term, &h.blocks, b), "ls\na.txt\nb.txt");
        assert_eq!(h.blocks.list[1].status, Status::Active);
        // A failing command.
        h.feed(b"false\r\n\x1b]133;C\x07\x1b]633;E;ZmFsc2U=\x07\x1b]133;D;1\x07\x1b]133;A\x07$ ");
        assert_eq!(h.blocks.list[1].status, Status::Error);
        assert_eq!(h.blocks.list[1].command.as_deref(), Some("false"));
        // Scrolling keeps absolute anchors valid: the first prompt is now in history.
        assert_eq!(h.blocks.to_line(0), Line(-(h.term.grid().history_size() as i32)));
    }

    #[test]
    fn anchors_survive_a_full_history_and_prune() {
        let mut h = Harness::new(10, 3, 4);
        h.feed(b"\x1b]133;A\x07$ ");
        for _ in 0..20 {
            h.feed(b"\r\n");
        }
        // Two newlines reach the bottom row, the other 18 scroll: history holds only
        // 4 of them, and the prompt (line 0) has dropped out.
        assert_eq!(h.term.grid().history_size(), 4);
        assert_eq!(h.blocks.scrolled, 18);
        h.blocks.prune(h.term.grid().history_size());
        assert!(h.blocks.list.is_empty());
        h.feed(b"\x1b]133;A\x07$ ");
        assert_eq!(h.blocks.list[0].prompt, 20);
        assert_eq!(h.blocks.to_line(20), Line(2));
    }
}
