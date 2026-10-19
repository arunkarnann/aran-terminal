//! Terminal widget: paints an `alacritty_terminal` grid with egui's painter and turns
//! egui input into the byte sequences a terminal expects. Replaces xterm.js.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use eframe::egui::{self, Color32, CornerRadius, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::session::{GridSize, Session};
use crate::theme;

/// Per-widget state kept between frames.
#[derive(Default)]
pub struct TermViewState {
    /// Fractional scroll accumulated from trackpads (one line = one cell height).
    scroll_acc: f32,
    selecting: bool,
}

pub struct CellMetrics {
    pub font: FontId,
    pub w: f32,
    pub h: f32,
}

impl CellMetrics {
    pub fn new(ctx: &egui::Context, size: f32) -> Self {
        let font = FontId::monospace(size);
        let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        CellMetrics { font, w: w.max(1.0), h: h.ceil().max(1.0) }
    }

    pub fn grid_for(&self, size: Vec2) -> GridSize {
        GridSize {
            cols: ((size.x / self.w).floor() as usize).max(2),
            rows: ((size.y / self.h).floor() as usize).max(1),
        }
    }
}

/// Draw the session and handle its input. Returns true if the user typed into it.
pub fn show(ui: &mut egui::Ui, session: &mut Session, st: &mut TermViewState, m: &CellMetrics) -> bool {
    let avail = ui.available_rect_before_wrap();
    let pad = 6.0;
    let inner = avail.shrink(pad);
    session.resize(m.grid_for(inner.size()));

    let id = ui.scope_id().with("terminal");
    let resp = ui.interact(avail, id, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() || !ui.ctx().memory(|mem| mem.focused().is_some()) {
        resp.request_focus();
    }
    // Keep Tab / arrows / Escape inside the terminal instead of moving egui focus.
    ui.memory_mut(|mem| {
        mem.set_focus_lock_filter(
            id,
            egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
        )
    });
    let focused = resp.has_focus();

    let typed = if focused { handle_keys(ui, session) } else { false };
    handle_mouse(ui, &resp, session, st, m, inner);
    paint(ui, session, m, inner, avail, focused);
    typed
}

// ---------------- painting ----------------

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn resolve(color: Color, colors: &alacritty_terminal::term::color::Colors, bold: bool) -> Color32 {
    match color {
        Color::Spec(c) => rgb(c),
        Color::Indexed(i) => {
            let i = if bold && i < 8 { i + 8 } else { i };
            colors[i as usize].map(rgb).unwrap_or_else(|| theme::indexed(i))
        }
        Color::Named(n) => {
            let n = if bold { n.to_bright() } else { n };
            colors[n].map(rgb).unwrap_or_else(|| theme::named(n))
        }
    }
}

fn paint(ui: &egui::Ui, session: &Session, m: &CellMetrics, inner: Rect, outer: Rect, focused: bool) {
    let painter = ui.painter_at(outer);
    painter.rect_filled(outer, CornerRadius::ZERO, theme::t().bg);

    let term = session.term.lock();
    let content = term.renderable_content();
    let offset = content.display_offset;
    let colors = content.colors;
    let selection = content.selection;
    let cols = session.size.cols;

    let origin = inner.min;
    let cell_pos = |row: usize, col: usize| Pos2::new(origin.x + col as f32 * m.w, origin.y + row as f32 * m.h);

    // Text runs are batched per row so one galley covers many cells.
    let mut run = String::new();
    let mut run_start = (0usize, 0usize);
    let mut run_style: Option<(Color32, bool, bool, bool)> = None;
    let mut cur_row = usize::MAX;

    let flush = |run: &mut String, start: (usize, usize), style: Option<(Color32, bool, bool, bool)>| {
        if run.trim().is_empty() {
            run.clear();
            return;
        }
        if let Some((fg, _bold, underline, strike)) = style {
            let pos = cell_pos(start.0, start.1);
            painter.text(pos, egui::Align2::LEFT_TOP, run.as_str(), m.font.clone(), fg);
            let width = run.chars().count() as f32 * m.w;
            if underline {
                let y = pos.y + m.h - 1.5;
                painter.line_segment([Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)], Stroke::new(1.0, fg));
            }
            if strike {
                let y = pos.y + m.h * 0.5;
                painter.line_segment([Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)], Stroke::new(1.0, fg));
            }
        }
        run.clear();
    };

    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(offset, indexed.point) else { continue };
        let (row, col) = (vp.line, vp.column.0);
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        if row != cur_row {
            flush(&mut run, run_start, run_style);
            cur_row = row;
            run_style = None;
        }

        let bold = cell.flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, colors, bold);
        let mut bg = resolve(cell.bg, colors, false);
        let default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));
        let mut has_bg = !default_bg;
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            has_bg = true;
        }
        if cell.flags.contains(Flags::DIM) {
            fg = fg.gamma_multiply(0.6);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        let selected = selection.is_some_and(|s| s.contains(indexed.point));
        if selected {
            bg = theme::t().selection;
            has_bg = true;
        }

        let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        if has_bg {
            let p = cell_pos(row, col);
            painter.rect_filled(
                Rect::from_min_size(p, Vec2::new(m.w * width as f32, m.h)),
                CornerRadius::ZERO,
                bg,
            );
        }

        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        let strike = cell.flags.contains(Flags::STRIKEOUT);
        let style = (fg, bold, underline, strike);
        let c = cell.c;
        // ASCII joins the current run; anything else is placed on its own cell so a
        // fallback font with different advance widths can't shift the rest of the row.
        if c.is_ascii() && width == 1 {
            let contiguous = run_style == Some(style) && run_start.1 + run.chars().count() == col;
            if !contiguous {
                flush(&mut run, run_start, run_style);
                run_start = (row, col);
                run_style = Some(style);
            }
            run.push(if c == '\0' { ' ' } else { c });
        } else {
            flush(&mut run, run_start, run_style);
            run_style = None;
            let p = cell_pos(row, col);
            let center = Pos2::new(p.x + m.w * width as f32 / 2.0, p.y + m.h / 2.0);
            let mut s = String::from(c);
            if let Some(zw) = cell.zerowidth() {
                s.extend(zw.iter());
            }
            painter.text(center, egui::Align2::CENTER_CENTER, s, m.font.clone(), fg);
        }
    }
    flush(&mut run, run_start, run_style);

    // Cursor (only when the app shows it and we're not scrolled back).
    if content.mode.contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden {
        if let Some(vp) = point_to_viewport(offset, content.cursor.point) {
            if vp.column.0 < cols {
                let p = cell_pos(vp.line, vp.column.0);
                let full = Rect::from_min_size(p, Vec2::new(m.w, m.h));
                let color = theme::t().cursor;
                match (focused, content.cursor.shape) {
                    (false, _) | (true, CursorShape::HollowBlock) => {
                        painter.rect_stroke(full, CornerRadius::ZERO, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                    }
                    (true, CursorShape::Beam) => {
                        painter.rect_filled(Rect::from_min_size(p, Vec2::new(2.0, m.h)), CornerRadius::ZERO, color);
                    }
                    (true, CursorShape::Underline) => {
                        painter.rect_filled(
                            Rect::from_min_size(Pos2::new(p.x, p.y + m.h - 2.0), Vec2::new(m.w, 2.0)),
                            CornerRadius::ZERO,
                            color,
                        );
                    }
                    (true, _) => {
                        painter.rect_filled(full, CornerRadius::ZERO, color);
                        let ch = term.grid()[content.cursor.point].c;
                        if ch != ' ' && ch != '\0' {
                            painter.text(p, egui::Align2::LEFT_TOP, ch, m.font.clone(), theme::t().bg);
                        }
                    }
                }
                // Tell the OS where the text caret is so IME popups land next to it.
                ui.ctx().output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Terminal,
                        rect: outer,
                        cursor_rect: full,
                        should_interrupt_composition: false,
                    })
                });
            }
        }
    }

    // Scrollback indicator.
    if offset > 0 {
        let txt = format!("↑ {offset} lines");
        painter.text(
            Pos2::new(outer.max.x - 10.0, outer.min.y + 6.0),
            egui::Align2::RIGHT_TOP,
            txt,
            FontId::proportional(11.0),
            theme::t().muted,
        );
    }
}

// ---------------- mouse ----------------

fn point_at(pos: Pos2, inner: Rect, m: &CellMetrics, size: GridSize, offset: usize) -> (Point, Side) {
    let x = ((pos.x - inner.min.x) / m.w).max(0.0);
    let y = ((pos.y - inner.min.y) / m.h).max(0.0);
    let col = (x as usize).min(size.cols - 1);
    let row = (y as usize).min(size.rows - 1);
    let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
    (viewport_to_point(offset, Point::new(row, Column(col))), side)
}

fn handle_mouse(
    ui: &egui::Ui,
    resp: &egui::Response,
    session: &Session,
    st: &mut TermViewState,
    m: &CellMetrics,
    inner: Rect,
) {
    let size = session.size;

    // Wheel / trackpad.
    if resp.hovered() {
        // Raw wheel events (not egui's smoothed delta, which keeps repainting).
        let dy: f32 = ui.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => match unit {
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Line => delta.y * m.h,
                        egui::MouseWheelUnit::Page => delta.y * m.h * size.rows as f32,
                    },
                    _ => 0.0,
                })
                .sum()
        });
        if dy != 0.0 {
            st.scroll_acc += dy / m.h;
            let lines = st.scroll_acc.trunc() as i32;
            if lines != 0 {
                st.scroll_acc -= lines as f32;
                let mut term = session.term.lock();
                let mode = *term.mode();
                if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                    // Full-screen apps (less, vim, Claude Code) get arrow keys instead.
                    let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
                    drop(term);
                    for _ in 0..lines.unsigned_abs().min(10) {
                        session.write(seq);
                    }
                } else {
                    term.scroll_display(Scroll::Delta(lines));
                }
            }
        }
    }

    // Selection: drag = simple, double-click = word, triple-click = line.
    let Some(pos) = resp.interact_pointer_pos() else {
        if !ui.input(|i| i.pointer.primary_down()) {
            st.selecting = false;
        }
        return;
    };
    let mut term = session.term.lock();
    let offset = term.grid().display_offset();
    let (point, side) = point_at(pos, inner, m, size, offset);

    if resp.triple_clicked() {
        term.selection = Some(Selection::new(SelectionType::Lines, point, side));
    } else if resp.double_clicked() {
        term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
    } else if resp.drag_started() {
        term.selection = Some(Selection::new(SelectionType::Simple, point, side));
        st.selecting = true;
    } else if resp.dragged() && st.selecting {
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, side);
        }
    } else if resp.clicked() {
        term.selection = None;
    }
}

// ---------------- keyboard ----------------

/// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn mod_param(m: Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let mp = mod_param(m);
    let csi = |final_: char| -> Vec<u8> {
        if mp > 1 {
            format!("\x1b[1;{mp}{final_}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if mp > 1 {
            format!("\x1b[{n};{mp}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let alt_prefix = |b: &[u8]| -> Vec<u8> {
        if m.alt {
            let mut v = vec![0x1b];
            v.extend_from_slice(b);
            v
        } else {
            b.to_vec()
        }
    };
    Some(match key {
        // Shift/Option+Enter = newline-without-submit in Claude Code and friends.
        Key::Enter if m.shift || m.alt => b"\x1b\r".to_vec(),
        Key::Enter => b"\r".to_vec(),
        Key::Backspace if m.ctrl => b"\x08".to_vec(),
        Key::Backspace => alt_prefix(b"\x7f"),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => csi('A'),
        Key::ArrowDown => csi('B'),
        Key::ArrowRight => csi('C'),
        Key::ArrowLeft => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ if m.ctrl => {
            let name = key.name();
            let b = if name.len() == 1 {
                let c = name.as_bytes()[0].to_ascii_lowercase();
                match c {
                    b'a'..=b'z' => c & 0x1f,
                    b'[' => 0x1b,
                    b'\\' => 0x1c,
                    b']' => 0x1d,
                    b'/' => 0x1f,
                    b'2' | b'@' => 0x00,
                    _ => return None,
                }
            } else if key == Key::Space {
                0x00
            } else {
                return None;
            };
            alt_prefix(&[b])
        }
        _ => return None,
    })
}

fn handle_keys(ui: &egui::Ui, session: &Session) -> bool {
    let events = ui.input(|i| i.events.clone());
    let app_cursor = session.term.lock().mode().contains(TermMode::APP_CURSOR);
    let mut typed = false;

    for ev in events {
        match ev {
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                // Cmd-shortcuts belong to the app (tabs, copy/paste), not the shell.
                if modifiers.command && !modifiers.ctrl {
                    continue;
                }
                if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                    session.write(&bytes);
                    typed = true;
                }
            }
            egui::Event::Text(t) => {
                session.write(t.as_bytes());
                typed = true;
            }
            egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                session.write(t.as_bytes());
                typed = true;
            }
            egui::Event::Paste(t) => {
                paste(session, &t);
                typed = true;
            }
            egui::Event::Copy => {
                if let Some(text) = session.term.lock().selection_to_string() {
                    ui.ctx().copy_text(text);
                }
            }
            _ => {}
        }
    }
    if typed {
        let mut term = session.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }
    typed
}

fn paste(session: &Session, text: &str) {
    let bracketed = session.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
    // Normalise newlines so a pasted script doesn't submit line by line.
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Strip any embedded end marker so the paste can't escape the bracket.
        let clean = text.replace("\x1b[201~", "");
        session.write(format!("\x1b[200~{clean}\x1b[201~").as_bytes());
    } else {
        session.write(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_respect_app_cursor_and_modifiers() {
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        let shift = Modifiers { shift: true, ..none };
        assert_eq!(key_bytes(Key::ArrowLeft, shift, false).unwrap(), b"\x1b[1;2D");
    }

    #[test]
    fn ctrl_letters_and_enter() {
        let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
        assert_eq!(key_bytes(Key::C, ctrl, false).unwrap(), vec![0x03]);
        assert_eq!(key_bytes(Key::Enter, Modifiers::NONE, false).unwrap(), b"\r");
        let shift = Modifiers { shift: true, ..Modifiers::NONE };
        assert_eq!(key_bytes(Key::Enter, shift, false).unwrap(), b"\x1b\r");
        assert!(key_bytes(Key::A, Modifiers::NONE, false).is_none());
    }
}
