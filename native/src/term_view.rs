//! Terminal widget: paints an `alacritty_terminal` grid with egui's painter and turns
//! egui input into the byte sequences a terminal expects. Replaces xterm.js.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use eframe::egui::{self, Color32, CornerRadius, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::session::{GridSize, Session, SharedTerm};
use crate::suggest::{Anchor, KeyAction, Suggest};
use crate::theme;

/// Looks up command suggestions: (prefix, cwd) → full command lines.
pub type SuggestFn<'a> = &'a dyn Fn(&str, Option<&str>) -> Vec<String>;

/// Per-widget state kept between frames.
#[derive(Default)]
pub struct TermViewState {
    /// Fractional scroll accumulated from trackpads (one line = one cell height).
    scroll_acc: f32,
    selecting: bool,
    suggest: Suggest,
    /// Wheel lines not yet turned into PageUp/PageDown (full-screen apps).
    page_acc: i32,
    /// Button held while the app has mouse reporting on (for drag/release reports).
    mouse_down: Option<u8>,
    last_motion_cell: Option<(usize, usize)>,
    /// The ghost text was drawn last frame (→ only accepts what the user can see).
    ghost_shown: bool,
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
/// `suggest` = command autocomplete source (None = turned off).
pub fn show(
    ui: &mut egui::Ui,
    session: &mut Session,
    st: &mut TermViewState,
    m: &CellMetrics,
    suggest: Option<SuggestFn<'_>>,
    grab_focus: bool,
) -> bool {
    let avail = ui.available_rect_before_wrap();
    let pad = 6.0;
    let inner = avail.shrink(pad);
    session.resize(m.grid_for(inner.size()));

    let id = ui.scope_id().with("terminal");
    let resp = ui.interact(avail, id, Sense::click_and_drag());
    if grab_focus || resp.clicked() || resp.drag_started() || !ui.ctx().memory(|mem| mem.focused().is_some()) {
        resp.request_focus();
    }
    // Keep Tab / arrows / Escape inside the terminal instead of moving egui focus.
    ui.memory_mut(|mem| {
        mem.set_focus_lock_filter(
            id,
            egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
        )
    });
    let focused = resp.has_focus() || grab_focus;

    // Suggestions only at a real shell prompt: never inside full-screen apps or a
    // running command (Claude Code, vim, a REPL …).
    let suggesting = suggest.is_some()
        && session.at_prompt
        && !session.term.lock().mode().contains(TermMode::ALT_SCREEN);
    if !suggesting {
        st.suggest.reset();
    }

    let typed = if focused { handle_keys(ui, session, st, suggesting) } else { false };
    handle_mouse(ui, &resp, session, st, m, inner);

    let mut ghost = None;
    if let (true, Some(lookup)) = (suggesting, suggest) {
        let line = st.suggest.anchor().and_then(|a| line_since(&session.term, a));
        if let Some(prefix) = st.suggest.sync(line) {
            let items = lookup(&prefix, session.cwd.as_deref());
            st.suggest.set_items(&prefix, items);
        }
        ghost = st.suggest.ghost().filter(|g| room_for_ghost(&session.term, g)).map(str::to_string);
    }
    st.ghost_shown = ghost.is_some();
    let cursor_rect = paint(ui, session, m, inner, avail, focused, ghost.as_deref());
    if focused && st.suggest.panel_open() {
        if let Some(rect) = cursor_rect {
            suggestion_list(ui, id, session, st, m, rect, avail);
        }
    }
    typed
}

/// Cursor position in scrollback-independent coordinates.
fn cursor_anchor(term: &SharedTerm) -> Anchor {
    let t = term.lock();
    let g = t.grid();
    Anchor { line: g.cursor.point.line.0 as i64 + g.history_size() as i64, col: g.cursor.point.column.0 }
}

/// The text between `a` and the cursor (the command line typed after the prompt).
fn line_since(term: &SharedTerm, a: Anchor) -> Option<String> {
    let t = term.lock();
    let g = t.grid();
    let hist = g.history_size() as i64;
    let cols = g.columns();
    let start = a.line - hist;
    let cur = g.cursor.point;
    let end = cur.line.0 as i64;
    if start < -hist || start > end || (start == end && a.col > cur.column.0) || end - start > 20 {
        return None;
    }
    let mut out = String::new();
    for l in start..=end {
        let row = &g[Line(l as i32)];
        let c0 = if l == start { a.col } else { 0 };
        let c1 = if l == end { cur.column.0 } else { cols };
        for c in c0..c1.min(cols) {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            out.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
    }
    Some(out)
}

/// The cells the ghost would cover are empty (no zsh-autosuggestions text or RPROMPT
/// there) and it fits on the row.
fn room_for_ghost(term: &SharedTerm, ghost: &str) -> bool {
    let t = term.lock();
    let g = t.grid();
    let cur = g.cursor.point;
    let n = ghost.chars().count();
    if cur.column.0 + n > g.columns() {
        return true; // drawn clipped; the tail can't collide with anything visible
    }
    let row = &g[cur.line];
    (cur.column.0..cur.column.0 + n).all(|c| matches!(row[Column(c)].c, ' ' | '\0'))
}

/// The ↓ list of suggestions under (or above) the cursor. Click = accept.
fn suggestion_list(ui: &egui::Ui, id: egui::Id, session: &Session, st: &mut TermViewState, m: &CellMetrics, cursor: Rect, outer: Rect) {
    let th = theme::t();
    let items: Vec<String> = st.suggest.items().to_vec();
    let sel = st.suggest.selected();
    let row_h = m.h + 4.0;
    let height = items.len() as f32 * row_h + 10.0;
    let below = cursor.max.y + 2.0 + height <= outer.max.y;
    let y = if below { cursor.max.y + 2.0 } else { (cursor.min.y - 2.0 - height).max(outer.min.y) };
    let x = cursor.min.x.min(outer.max.x - 260.0).max(outer.min.x);
    let mut clicked = None;
    egui::Area::new(id.with("suggest"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::new(x, y))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(th.panel)
                .stroke(Stroke::new(1.0, th.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let width = (outer.max.x - x - 16.0).clamp(160.0, 640.0);
                    for (i, s) in items.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
                        if i == sel || resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), if i == sel { th.selection } else { th.border.gamma_multiply(0.4) });
                        }
                        let text = ui.painter().layout_no_wrap(s.clone(), m.font.clone(), th.text);
                        let clip = rect.shrink2(Vec2::new(6.0, 0.0));
                        ui.painter().with_clip_rect(clip).galley(Pos2::new(clip.min.x, rect.center().y - text.size().y / 2.0), text, th.text);
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                    ui.label(egui::RichText::new("→ accept · ↑↓ choose · Esc hide").size(10.0).color(th.muted));
                });
        });
    if let Some(i) = clicked {
        if let KeyAction::Accept(rest) = st.suggest.accept(i) {
            session.write(rest.as_bytes());
        }
    }
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

/// Paint the grid. Returns the cursor cell's rect when it's on screen.
fn paint(ui: &egui::Ui, session: &Session, m: &CellMetrics, inner: Rect, outer: Rect, focused: bool, ghost: Option<&str>) -> Option<Rect> {
    let mut cursor_rect = None;
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

    // Autocomplete ghost: the rest of the suggestion, greyed, from the cursor on.
    let ghost_first = ghost.and_then(|g| {
        let vp = point_to_viewport(offset, content.cursor.point)?;
        let room = cols.saturating_sub(vp.column.0);
        let shown: String = g.chars().take(room).collect();
        if offset == 0 && !shown.is_empty() {
            let p = cell_pos(vp.line, vp.column.0);
            painter.text(p, egui::Align2::LEFT_TOP, &shown, m.font.clone(), theme::t().muted.gamma_multiply(0.9));
        }
        shown.chars().next()
    });

    // Cursor (only when the app shows it and we're not scrolled back).
    if content.mode.contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden {
        if let Some(vp) = point_to_viewport(offset, content.cursor.point) {
            if vp.column.0 < cols {
                let p = cell_pos(vp.line, vp.column.0);
                let full = Rect::from_min_size(p, Vec2::new(m.w, m.h));
                cursor_rect = Some(full);
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
                        let mut ch = term.grid()[content.cursor.point].c;
                        if ch == ' ' || ch == '\0' {
                            ch = ghost_first.unwrap_or(ch);
                        }
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
    cursor_rect
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
    let mode = *session.term.lock().mode();
    // Shift forces Aran's own selection/scrollback even when the app wants the mouse
    // (same convention as iTerm2 / Terminal.app).
    let shift = ui.input(|i| i.modifiers.shift);
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !shift;
    let cell_at = |pos: Pos2| -> (usize, usize) {
        let col = (((pos.x - inner.min.x) / m.w).max(0.0) as usize).min(size.cols - 1);
        let row = (((pos.y - inner.min.y) / m.h).max(0.0) as usize).min(size.rows - 1);
        (col, row)
    };

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
                let alt = mode.contains(TermMode::ALT_SCREEN);
                if reporting {
                    // The app asked for the mouse (Claude Code, vim, htop …): send real
                    // wheel events so it scrolls its own view. Never arrow keys — those
                    // recall prompt history in chat-style TUIs.
                    let (col, row) = ui.input(|i| i.pointer.hover_pos()).map(cell_at).unwrap_or((0, 0));
                    let button = if lines > 0 { 64 } else { 65 };
                    let mods = ui.input(|i| i.modifiers);
                    for _ in 0..lines.unsigned_abs().min(10) {
                        if let Some(b) = mouse_report(mode, button + mouse_mods(mods), col, row, true) {
                            session.write(&b);
                        }
                    }
                } else if alt && !shift {
                    // Full-screen app without mouse support (less, man …): page keys,
                    // one per half screen of wheel travel, so the content scrolls
                    // instead of a prompt's history being walked.
                    st.page_acc += lines;
                    let step = (size.rows as i32 / 2).max(1);
                    while st.page_acc.abs() >= step {
                        let up = st.page_acc > 0;
                        session.write(if up { b"\x1b[5~" } else { b"\x1b[6~" });
                        st.page_acc -= if up { step } else { -step };
                    }
                } else {
                    st.page_acc = 0;
                    session.term.lock().scroll_display(Scroll::Delta(lines));
                }
            }
        }
    }

    // Clicks / drags go to the app when it asked for them.
    if reporting {
        let mods = ui.input(|i| i.modifiers);
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::PointerButton { pos, button, pressed, .. } if inner.expand(6.0).contains(pos) || !pressed => {
                    let code = match button {
                        egui::PointerButton::Primary => 0,
                        egui::PointerButton::Middle => 1,
                        egui::PointerButton::Secondary => 2,
                        _ => continue,
                    };
                    if !pressed && st.mouse_down.is_none() {
                        continue; // release of a press that started elsewhere
                    }
                    st.mouse_down = pressed.then_some(code);
                    let (col, row) = cell_at(pos);
                    if let Some(b) = mouse_report(mode, code + mouse_mods(mods), col, row, pressed) {
                        session.write(&b);
                    }
                }
                egui::Event::PointerMoved(pos) if inner.contains(pos) => {
                    let (col, row) = cell_at(pos);
                    if st.last_motion_cell == Some((col, row)) {
                        continue;
                    }
                    let code = match st.mouse_down {
                        Some(b) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => b + 32,
                        None if mode.contains(TermMode::MOUSE_MOTION) => 35,
                        _ => continue,
                    };
                    st.last_motion_cell = Some((col, row));
                    if let Some(b) = mouse_report(mode, code + mouse_mods(mods), col, row, true) {
                        session.write(&b);
                    }
                }
                _ => {}
            }
        }
        session.term.lock().selection = None;
        return;
    }
    st.mouse_down = None;

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

/// xterm mouse modifier bits: shift 4, alt/meta 8, ctrl 16.
fn mouse_mods(m: Modifiers) -> u8 {
    4 * m.shift as u8 + 8 * m.alt as u8 + 16 * m.ctrl as u8
}

/// Encode one mouse event for the app (0-based cell). SGR (`ESC[<b;x;yM/m`) when
/// the app asked for it, else the legacy X10 form (which can't address past col 223).
fn mouse_report(mode: TermMode, button: u8, col: usize, row: usize, pressed: bool) -> Option<Vec<u8>> {
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{button};{x};{y}{end}").into_bytes());
    }
    // Legacy: a release is button 3 (wheel events have no release).
    let b = if pressed { button } else { 3 + (button & !3) };
    let enc = |v: usize| -> Option<Vec<u8>> {
        let v = v + 32;
        if mode.contains(TermMode::UTF8_MOUSE) {
            char::from_u32(v as u32).filter(|_| v < 2048).map(|c| c.to_string().into_bytes())
        } else {
            u8::try_from(v).ok().map(|b| vec![b])
        }
    };
    let mut out = b"\x1b[M".to_vec();
    out.push(32u8.checked_add(b)?);
    out.extend(enc(x)?);
    out.extend(enc(y)?);
    Some(out)
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

fn handle_keys(ui: &egui::Ui, session: &Session, st: &mut TermViewState, suggesting: bool) -> bool {
    let events = ui.input(|i| i.events.clone());
    let app_cursor = session.term.lock().mode().contains(TermMode::APP_CURSOR);
    let mut typed = false;
    // Every byte headed for the shell also goes through the suggestion tracker.
    let send = |st: &mut TermViewState, bytes: &[u8]| {
        if suggesting {
            st.suggest.feed(bytes, cursor_anchor(&session.term));
        }
        session.write(bytes);
    };

    for ev in events {
        match ev {
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                // Cmd-shortcuts belong to the app (tabs, copy/paste), not the shell.
                if modifiers.command && !modifiers.ctrl {
                    continue;
                }
                // → only takes a suggestion the user can actually see.
                let hidden_ghost = key == Key::ArrowRight && !st.ghost_shown && !st.suggest.panel_open();
                if suggesting && !hidden_ghost {
                    match st.suggest.on_key(key, modifiers) {
                        KeyAction::Pass => {}
                        KeyAction::Consumed => continue,
                        KeyAction::Accept(rest) => {
                            session.write(rest.as_bytes());
                            typed = true;
                            continue;
                        }
                    }
                }
                if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                    send(st, &bytes);
                    typed = true;
                }
            }
            egui::Event::Text(t) => {
                send(st, t.as_bytes());
                typed = true;
            }
            egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                send(st, t.as_bytes());
                typed = true;
            }
            egui::Event::Paste(t) => {
                if suggesting {
                    st.suggest.feed_paste(&t, cursor_anchor(&session.term));
                }
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
    fn mouse_reports() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(sgr, 64, 4, 9, true).unwrap(), b"\x1b[<64;5;10M");
        assert_eq!(mouse_report(sgr, 0, 0, 0, false).unwrap(), b"\x1b[<0;1;1m");
        let x10 = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(mouse_report(x10, 65, 0, 0, true).unwrap(), vec![0x1b, b'[', b'M', 32 + 65, 33, 33]);
        assert_eq!(mouse_report(x10, 0, 0, 0, false).unwrap(), vec![0x1b, b'[', b'M', 32 + 3, 33, 33]);
        assert!(mouse_report(x10, 0, 300, 0, true).is_none(), "X10 can't address col 301");
        let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
        assert_eq!(mouse_mods(ctrl), 16);
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
