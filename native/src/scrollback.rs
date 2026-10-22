//! Scrollback serialization for session restore — the native stand-in for
//! xterm.js's `@xterm/addon-serialize`. The primary grid (history + screen) is
//! turned back into bytes a terminal understands: text with SGR colour/attribute
//! escapes, one `\r\n` per line. Feeding those bytes into a fresh `Term` repaints
//! the old session, colours included, before the new shell prints its prompt.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// Lines kept per session (history + screen). Bounds the DB row and restore time.
pub const MAX_LINES: usize = 3_000;

/// Shown under the restored text so the user knows where the previous session ended.
pub const DIVIDER: &[u8] = b"\r\n\x1b[2m\xe2\x80\x94\xe2\x80\x94\xe2\x80\x94 restored from previous session \xe2\x80\x94\xe2\x80\x94\xe2\x80\x94\x1b[0m\r\n";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Style {
    fn default_style() -> Self {
        Style { fg: Color::Named(NamedColor::Foreground), bg: Color::Named(NamedColor::Background), flags: Flags::empty() }
    }
}

/// SGR parameter(s) selecting `c` as foreground (`fg == true`) or background.
fn color_params(c: Color, fg: bool) -> Option<String> {
    let base = if fg { 30 } else { 40 };
    match c {
        Color::Named(n) => {
            let i = n as usize;
            match i {
                0..=7 => Some(format!("{}", base + i)),
                8..=15 => Some(format!("{}", base + 60 + i - 8)),
                _ if n == NamedColor::Foreground && fg => Some("39".into()),
                _ if n == NamedColor::Background && !fg => Some("49".into()),
                // Dim/bright variants of the base colours.
                _ => {
                    let idx = match n {
                        NamedColor::DimBlack => 0,
                        NamedColor::DimRed => 1,
                        NamedColor::DimGreen => 2,
                        NamedColor::DimYellow => 3,
                        NamedColor::DimBlue => 4,
                        NamedColor::DimMagenta => 5,
                        NamedColor::DimCyan => 6,
                        NamedColor::DimWhite => 7,
                        _ => return if fg { Some("39".into()) } else { Some("49".into()) },
                    };
                    Some(format!("{}", base + idx))
                }
            }
        }
        Color::Indexed(i) => Some(format!("{};5;{i}", base + 8)),
        Color::Spec(rgb) => Some(format!("{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b)),
    }
}

/// Emit the SGR sequence that turns `from` into `to` (a full reset + re-apply keeps it simple).
fn sgr(from: Style, to: Style, out: &mut Vec<u8>) {
    if from == to {
        return;
    }
    let mut params: Vec<String> = vec!["0".into()];
    let f = to.flags;
    if f.contains(Flags::BOLD) {
        params.push("1".into());
    }
    if f.contains(Flags::DIM) {
        params.push("2".into());
    }
    if f.contains(Flags::ITALIC) {
        params.push("3".into());
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        params.push("4".into());
    }
    if f.contains(Flags::INVERSE) {
        params.push("7".into());
    }
    if f.contains(Flags::HIDDEN) {
        params.push("8".into());
    }
    if f.contains(Flags::STRIKEOUT) {
        params.push("9".into());
    }
    if let Some(p) = color_params(to.fg, true) {
        if p != "39" {
            params.push(p);
        }
    }
    if let Some(p) = color_params(to.bg, false) {
        if p != "49" {
            params.push(p);
        }
    }
    out.extend_from_slice(format!("\x1b[{}m", params.join(";")).as_bytes());
}

/// Serialize the primary grid: at most `max_lines` lines ending at the last non-blank
/// line. Returns None when there is nothing worth keeping.
pub fn serialize<L: EventListener>(term: &Term<L>, max_lines: usize) -> Option<Vec<u8>> {
    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines() as i32;
    let hist = grid.history_size() as i32;

    // Last line with any visible content.
    let mut last: Option<i32> = None;
    for l in (-hist..rows).rev() {
        let row = &grid[Line(l)];
        if (0..cols).any(|c| {
            let cell = &row[Column(c)];
            !matches!(cell.c, ' ' | '\0' | '\t') && !cell.flags.contains(Flags::WIDE_CHAR_SPACER)
        }) {
            last = Some(l);
            break;
        }
    }
    let last = last?;
    let first = (last + 1 - max_lines as i32).max(-hist);

    let mut out = Vec::new();
    let mut style = Style::default_style();
    for l in first..=last {
        let row = &grid[Line(l)];
        // Trim trailing blanks so restored lines don't carry a full row of spaces.
        let mut end = cols;
        while end > 0 {
            let cell = &row[Column(end - 1)];
            if matches!(cell.c, ' ' | '\0') && cell.bg == Color::Named(NamedColor::Background) && !cell.flags.contains(Flags::INVERSE) {
                end -= 1;
            } else {
                break;
            }
        }
        let mut col = 0;
        while col < end {
            let cell = &row[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                col += 1;
                continue;
            }
            let st = Style { fg: cell.fg, bg: cell.bg, flags: cell.flags & (Flags::BOLD | Flags::DIM | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::INVERSE | Flags::HIDDEN | Flags::STRIKEOUT) };
            sgr(style, st, &mut out);
            style = st;
            let c = if cell.c == '\0' { ' ' } else { cell.c };
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            if let Some(zw) = cell.zerowidth() {
                for z in zw {
                    out.extend_from_slice(z.encode_utf8(&mut buf).as_bytes());
                }
            }
            col += if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        }
        // A wrapped line continues on the next row without a newline, so the restored
        // grid re-wraps it to the new width instead of leaving a hard break.
        let wrapped = end == cols && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if !wrapped {
            sgr(style, Style::default_style(), &mut out);
            style = Style::default_style();
            out.extend_from_slice(b"\r\n");
        }
    }
    sgr(style, Style::default_style(), &mut out);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn term_with(bytes: &[u8]) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &TermSize::new(20, 5), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut t, bytes);
        t
    }

    #[test]
    fn round_trips_text_and_colour() {
        let t = term_with(b"hello \x1b[31mred\x1b[0m\r\nline two\r\n");
        let bytes = serialize(&t, 100).unwrap();
        let s = String::from_utf8(bytes.clone()).unwrap();
        assert!(s.contains("hello "));
        assert!(s.contains("\x1b[0;31mred"));
        assert!(s.contains("line two\r\n"));
        // Feeding it back reproduces the screen.
        let t2 = term_with(&bytes);
        let g = t2.grid();
        let row: String = (0..9).map(|c| g[Line(1)][Column(c)].c).collect();
        assert_eq!(row.trim_end(), "line two");
        assert_eq!(g[Line(0)][Column(6)].fg, Color::Named(NamedColor::Red));
    }

    #[test]
    fn empty_grid_serializes_to_none_and_lines_are_capped() {
        let t = term_with(b"");
        assert!(serialize(&t, 100).is_none());
        let mut text = Vec::new();
        for i in 0..50 {
            text.extend_from_slice(format!("l{i}\r\n").as_bytes());
        }
        let t = term_with(&text);
        let s = String::from_utf8(serialize(&t, 3).unwrap()).unwrap();
        assert_eq!(s.matches("\r\n").count(), 3);
        assert!(s.contains("l49"));
        assert!(!s.contains("l46"));
    }
}
