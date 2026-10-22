//! Focus View — port of the Tauri build's `components/focus/*` + `useFocus.ts`.
//!
//! A Pomodoro-style deep-work loop bound to one session: a hero countdown ring,
//! a daily goal ring with streak, per-session timer tiles, and a break prompt when
//! a block lands. Every timer is timestamp-derived (`started_at + planned_ms - now`),
//! the DB owns the anchor, so the clock is drift-free and survives relaunch.
//! Same `focus_block` table and db functions as the Tauri build.

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CornerRadius, Frame, Key, Margin, Pos2, RichText, Sense, Stroke, Vec2};
use rusqlite::Connection;

use crate::db;
use crate::ipc::{AttentionState, FocusBlock, FocusDay, SessionId};
use crate::session::{lock, now_ms};
use crate::theme;
use crate::timefmt::{fmt_duration, start_of_day};

const FOCUS_GOAL_KEY: &str = "focus.daily_goal_ms";
const DEFAULT_GOAL_MS: i64 = 2 * 60 * 60 * 1000;
/// Focus-block presets, in minutes.
const DURATIONS: [i64; 3] = [25, 50, 90];
/// Break presets, in minutes.
const BREAKS: [i64; 2] = [5, 15];
/// Daily goal presets, in minutes.
const GOALS: [i64; 4] = [60, 120, 180, 240];
const MINUTE: i64 = 60_000;

/// What the app needs to know about each open session.
pub struct SessionInfo {
    pub id: SessionId,
    pub label: String,
    pub project: Option<String>,
    pub task_label: Option<String>,
    pub state: AttentionState,
    pub created_at: i64,
    pub state_since: i64,
}

/// Side effects for the app: a focus block mutes notifications for other sessions.
pub enum FocusEvent {
    Started { session: Option<SessionId> },
    Ended,
}

/// What just finished, so the view can show the after-block prompt.
struct Completed {
    planned_ms: i64,
    task_label: Option<String>,
}

pub struct FocusPanel {
    db: Arc<Mutex<Connection>>,
    active: Option<FocusBlock>,
    day: Option<FocusDay>,
    day_at: i64,
    completed: Option<Completed>,
    // Composer state.
    session: Option<SessionId>,
    minutes: i64,
    custom: String,
    intention: String,
    editing_goal: bool,
}

impl FocusPanel {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        let mut p = FocusPanel {
            db,
            active: None,
            day: None,
            day_at: 0,
            completed: None,
            session: None,
            minutes: DURATIONS[0],
            custom: String::new(),
            intention: String::new(),
            editing_goal: false,
        };
        // Rehydrate: an active block survives a relaunch, but one that already elapsed
        // ran down while the app was closed — abandon it rather than credit phantom focus.
        let block = db::active_focus_block(&lock(&p.db));
        match block {
            Some(b) if now_ms() >= b.started_at + b.planned_ms => {
                db::end_focus_block(&lock(&p.db), &b.id, now_ms(), "abandoned");
            }
            Some(b) => p.active = Some(b),
            None => {}
        }
        p.refresh_day();
        p
    }

    pub fn active(&self) -> Option<&FocusBlock> {
        self.active.as_ref()
    }

    pub fn remaining_ms(&self, now: i64) -> i64 {
        self.active.as_ref().map(|b| (b.started_at + b.planned_ms - now).max(0)).unwrap_or(0)
    }

    fn goal_ms(&self) -> i64 {
        db::get_setting(&lock(&self.db), FOCUS_GOAL_KEY).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_GOAL_MS)
    }

    fn refresh_day(&mut self) {
        let goal = self.goal_ms();
        self.day = Some(db::focus_day(&lock(&self.db), start_of_day(now_ms()), goal));
        self.day_at = now_ms();
    }

    /// Once a frame: complete an elapsed block, abandon one whose session closed.
    pub fn tick(&mut self, now: i64, sessions: &[SessionInfo]) -> Vec<FocusEvent> {
        let mut events = Vec::new();
        let Some(b) = self.active.clone() else { return events };
        if now >= b.started_at + b.planned_ms {
            db::end_focus_block(&lock(&self.db), &b.id, now, "completed");
            self.active = None;
            if b.kind == "focus" {
                chime();
                self.completed = Some(Completed { planned_ms: b.planned_ms, task_label: b.task_label.clone() });
            }
            self.refresh_day();
            events.push(FocusEvent::Ended);
        } else if b.kind == "focus" && b.session_id.as_ref().is_some_and(|id| !sessions.iter().any(|s| &s.id == id)) {
            db::end_focus_block(&lock(&self.db), &b.id, now, "abandoned");
            self.active = None;
            self.refresh_day();
            events.push(FocusEvent::Ended);
        }
        events
    }

    fn start(&mut self, session: Option<SessionId>, planned_ms: i64, task_label: Option<String>, kind: &str) -> FocusEvent {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        {
            let conn = lock(&self.db);
            db::insert_focus_block(&conn, &id, session.as_deref(), task_label.as_deref(), kind, now, planned_ms.max(0));
            self.active = db::get_focus_block(&conn, &id);
        }
        self.completed = None;
        FocusEvent::Started { session: if kind == "focus" { session } else { None } }
    }

    /// Finish early: `complete` counts it as a landed block, else it's abandoned.
    fn stop(&mut self, complete: bool) -> Option<FocusEvent> {
        let b = self.active.take()?;
        db::end_focus_block(&lock(&self.db), &b.id, now_ms(), if complete { "completed" } else { "abandoned" });
        self.refresh_day();
        Some(FocusEvent::Ended)
    }

    fn extend(&mut self, add_ms: i64) {
        if let Some(b) = self.active.as_mut() {
            b.planned_ms += add_ms;
            db::extend_focus_block(&lock(&self.db), &b.id, add_ms);
        }
    }

    fn set_goal(&mut self, ms: i64) {
        db::set_setting(&lock(&self.db), FOCUS_GOAL_KEY, &ms.max(0).to_string());
        self.refresh_day();
    }

    /// Compact live countdown for the header. Returns true when clicked.
    pub fn pill(&self, ui: &mut egui::Ui, now: i64) -> bool {
        let Some(b) = &self.active else { return false };
        let th = theme::t();
        let is_break = b.kind == "break";
        let color = if is_break { th.idle } else { th.primary };
        let text = format!("{} {}", fmt_countdown(self.remaining_ms(now)), if is_break { "break" } else { "focus" });
        let btn = egui::Button::new(RichText::new(text).size(12.0).color(theme::ink_on(color))).fill(color);
        ui.add(btn).on_hover_text(if is_break { "Break in progress" } else { "Focus block in progress" }).clicked()
    }

    /// The pane. Returns (session to activate, events for the app).
    pub fn show(&mut self, ui: &mut egui::Ui, sessions: &[SessionInfo], active_id: Option<&SessionId>, now: i64) -> (Option<SessionId>, Vec<FocusEvent>) {
        let mut events = Vec::new();
        let mut activate = None;
        if now - self.day_at > 60_000 {
            self.refresh_day();
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.active.is_some() {
                self.running_hero(ui, sessions, now, &mut events);
            } else {
                self.composer(ui, sessions, active_id, &mut events);
            }
            ui.add_space(14.0);
            self.goal_ring(ui, now);
            ui.add_space(14.0);
            activate = self.tiles(ui, sessions, active_id, now);
        });
        (activate, events)
    }

    fn running_hero(&mut self, ui: &mut egui::Ui, sessions: &[SessionInfo], now: i64, events: &mut Vec<FocusEvent>) {
        let th = theme::t();
        let Some(b) = self.active.clone() else { return };
        let elapsed = now - b.started_at;
        let progress = if b.planned_ms > 0 { elapsed as f32 / b.planned_ms as f32 } else { 1.0 };
        let is_break = b.kind == "break";
        let session = sessions.iter().find(|s| Some(&s.id) == b.session_id.as_ref());
        let who = session.map(|s| s.label.clone()).unwrap_or_else(|| if is_break { "Break".into() } else { "Focus".into() });
        let color = if is_break { th.idle } else { th.primary };

        ui.vertical_centered(|ui| {
            ring(ui, 200.0, 12.0, progress.clamp(0.0, 1.0), color, &[
                (fmt_countdown(self.remaining_ms(now)), 34.0, th.text),
                ((if is_break { "break" } else { "deep work" }).to_string(), 12.0, th.muted),
            ]);
            ui.add_space(6.0);
            ui.label(RichText::new(who).strong().size(14.0));
            if let Some(t) = &b.task_label {
                ui.label(RichText::new(t).color(th.muted).size(12.0));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("+5 min").clicked() {
                    self.extend(5 * MINUTE);
                }
                if !is_break && ui.button(RichText::new("Done").color(th.running)).on_hover_text("Finish now and count this block").clicked() {
                    events.extend(self.stop(true));
                }
                let stop = if is_break { "End break" } else { "Stop" };
                if ui.button(RichText::new(stop).color(th.error)).on_hover_text(if is_break { "End break" } else { "Stop without counting it" }).clicked() {
                    events.extend(self.stop(false));
                }
            });
        });
    }

    fn composer(&mut self, ui: &mut egui::Ui, sessions: &[SessionInfo], active_id: Option<&SessionId>, events: &mut Vec<FocusEvent>) {
        let th = theme::t();
        // Default the target to the active session, and keep it valid as sessions change.
        if self.session.as_ref().is_none_or(|id| !sessions.iter().any(|s| &s.id == id)) {
            self.session = active_id.cloned().or_else(|| sessions.first().map(|s| s.id.clone()));
        }
        let planned_min = self.custom.trim().parse::<i64>().ok().filter(|m| *m > 0).unwrap_or(self.minutes);
        let target = sessions.iter().find(|s| Some(&s.id) == self.session.as_ref());

        ui.vertical_centered(|ui| {
            ring(ui, 200.0, 12.0, 0.0, th.primary, &[
                (format!("{planned_min:02}:00"), 34.0, th.muted),
                ("ready".to_string(), 12.0, th.muted),
            ]);
        });
        ui.add_space(8.0);
        if sessions.is_empty() {
            ui.label(RichText::new("Open a terminal first, then start a focus block.").color(th.muted).size(12.0));
            return;
        }
        let mut start = false;
        egui::Grid::new("focus-composer").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Session").color(th.muted).size(11.0));
            let sel = target.map(|s| s.label.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("focus-session").selected_text(RichText::new(sel).size(12.0)).width(ui.available_width()).show_ui(ui, |ui| {
                for s in sessions {
                    let on = Some(&s.id) == self.session.as_ref();
                    if ui.selectable_label(on, RichText::new(&s.label).size(12.0)).clicked() {
                        self.session = Some(s.id.clone());
                    }
                }
            });
            ui.end_row();

            ui.label(RichText::new("Duration").color(th.muted).size(11.0));
            ui.horizontal(|ui| {
                for m in DURATIONS {
                    let on = self.custom.trim().is_empty() && self.minutes == m;
                    if ui.add(egui::Button::selectable(on, RichText::new(format!("{m}m")).size(11.5)).small()).clicked() {
                        self.minutes = m;
                        self.custom.clear();
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.custom).hint_text("custom").desired_width(52.0).font(egui::TextStyle::Small));
            });
            ui.end_row();

            ui.label(RichText::new("Intention").color(th.muted).size(11.0));
            let hint = target.and_then(|s| s.task_label.clone()).unwrap_or_else(|| "What will you focus on?".into());
            let r = ui.add(egui::TextEdit::singleline(&mut self.intention).hint_text(hint).desired_width(ui.available_width()));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                start = true;
            }
            ui.end_row();
        });
        ui.add_space(6.0);
        let can_start = self.session.is_some() && planned_min > 0;
        let btn = egui::Button::new(RichText::new(format!("Start {planned_min}-minute focus")).color(theme::ink_on(th.primary))).fill(th.primary);
        if ui.add_enabled(can_start, btn).clicked() {
            start = true;
        }
        if start && can_start {
            let label = Some(self.intention.trim().to_string()).filter(|s| !s.is_empty()).or_else(|| target.and_then(|s| s.task_label.clone()));
            let session = self.session.clone();
            events.push(self.start(session, planned_min * MINUTE, label, "focus"));
            self.intention.clear();
            self.custom.clear();
        }
    }

    fn goal_ring(&mut self, ui: &mut egui::Ui, now: i64) {
        let th = theme::t();
        // Count the in-progress focus block toward today's total, live.
        let live = self.active.as_ref().filter(|b| b.kind == "focus").map(|b| (now - b.started_at).max(0)).unwrap_or(0);
        let day = self.day.as_ref();
        let focus_ms = day.map(|d| d.focus_ms).unwrap_or(0) + live;
        let goal_ms = day.map(|d| d.goal_ms).unwrap_or(0);
        let progress = if goal_ms > 0 { focus_ms as f32 / goal_ms as f32 } else { 0.0 };
        let met = goal_ms > 0 && focus_ms >= goal_ms;
        let blocks = day.map(|d| d.blocks_completed).unwrap_or(0) + i64::from(live > 0);
        let streak = day.map(|d| d.streak_days).unwrap_or(0);

        ui.horizontal(|ui| {
            ring(ui, 96.0, 8.0, progress.clamp(0.0, 1.0), if met { th.running } else { th.primary }, &[
                (fmt_minutes(focus_ms), 15.0, th.text),
                (format!("of {}", fmt_minutes(goal_ms)), 10.0, th.muted),
            ]);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Today").strong().size(13.0));
                    if met {
                        ui.label(RichText::new("✓ goal met").color(th.running).size(11.0));
                    }
                });
                ui.label(RichText::new(format!("{blocks} {}  ·  🔥 {streak} day{}", if blocks == 1 { "block" } else { "blocks" }, if streak == 1 { "" } else { "s" })).color(th.muted).size(11.5));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for _ in 0..blocks.min(12) {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, th.primary);
                    }
                });
                if self.editing_goal {
                    ui.horizontal(|ui| {
                        for m in GOALS {
                            let on = goal_ms == m * MINUTE;
                            if ui.add(egui::Button::selectable(on, RichText::new(fmt_minutes(m * MINUTE)).size(11.0)).small()).clicked() {
                                self.set_goal(m * MINUTE);
                                self.editing_goal = false;
                            }
                        }
                    });
                } else if ui.add(egui::Button::new(RichText::new("Set goal").size(11.0).color(th.muted)).frame(false)).clicked() {
                    self.editing_goal = true;
                }
            });
        });
    }

    fn tiles(&self, ui: &mut egui::Ui, sessions: &[SessionInfo], active_id: Option<&SessionId>, now: i64) -> Option<SessionId> {
        let th = theme::t();
        if sessions.is_empty() {
            return None;
        }
        let focused = self.active.as_ref().filter(|b| b.kind == "focus").and_then(|b| b.session_id.clone());
        // Focused session first, then by urgency.
        let mut ordered: Vec<&SessionInfo> = sessions.iter().collect();
        let rank = |s: &SessionInfo| match s.state {
            AttentionState::Waiting => 0,
            AttentionState::Running => 1,
            AttentionState::Idle => 2,
        };
        ordered.sort_by_key(|s| (Some(&s.id) != focused.as_ref(), rank(s), -s.state_since));

        ui.label(RichText::new("SESSIONS").color(th.muted).size(11.0).strong());
        ui.add_space(4.0);
        let mut pick = None;
        for s in ordered {
            let is_focused = Some(&s.id) == focused.as_ref();
            let receded = focused.is_some() && !is_focused;
            let queued = receded && s.state == AttentionState::Waiting;
            let is_active = Some(&s.id) == active_id;
            let color = state_color(s.state);
            let resp = Frame::new()
                .fill(if is_active { th.border.gamma_multiply(0.5) } else { Color32::TRANSPARENT })
                .stroke(Stroke::new(1.0, if is_focused { th.primary } else { th.border.gamma_multiply(0.6) }))
                .corner_radius(CornerRadius::same(5))
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if receded {
                        ui.set_opacity(0.55);
                    }
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, color);
                        ui.add(egui::Label::new(RichText::new(&s.label).color(th.text)).truncate().selectable(false))
                            .on_hover_text(s.project.clone().unwrap_or_else(|| "Loose".into()));
                        if is_focused {
                            tag(ui, "focusing", th.primary);
                        }
                        if queued {
                            tag(ui, "queued", th.waiting);
                        }
                    });
                    if let Some(t) = &s.task_label {
                        ui.label(RichText::new(t).color(th.muted).size(11.0));
                    }
                    let state = match s.state {
                        AttentionState::Waiting => format!("waiting {}", fmt_duration(now - s.state_since)),
                        AttentionState::Running => "RUNNING".into(),
                        AttentionState::Idle => "IDLE".into(),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(state).color(color).size(10.5));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("up {}", fmt_duration(now - s.created_at))).color(th.muted).size(10.5));
                        });
                    });
                })
                .response
                .interact(Sense::click());
            if resp.clicked() {
                pick = Some(s.id.clone());
            }
        }
        pick
    }

    /// Shown right after a focus block lands — start a break or get back to it.
    pub fn break_prompt(&mut self, ctx: &egui::Context) -> Vec<FocusEvent> {
        let mut events = Vec::new();
        let Some(done) = self.completed.as_ref() else { return events };
        let th = theme::t();
        let (planned, label) = (done.planned_ms, done.task_label.clone());
        let mut close = false;
        egui::Window::new("Block complete 🎉")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let on = label.as_ref().map(|t| format!(" on “{t}”")).unwrap_or_default();
                ui.label(format!("{} of deep work{on}. Take a break?", fmt_minutes(planned)));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for m in BREAKS {
                        if ui.button(format!("{} break", fmt_minutes(m * MINUTE))).clicked() {
                            events.push(self.start(None, m * MINUTE, None, "break"));
                            close = true;
                        }
                    }
                    let b = egui::Button::new(RichText::new("Back to work").color(theme::ink_on(th.primary))).fill(th.primary);
                    if ui.add(b).clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.completed = None;
        }
        events
    }
}

fn tag(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new().fill(color.gamma_multiply(0.18)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::symmetric(4, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).color(color).size(9.5));
    });
}

fn state_color(s: AttentionState) -> Color32 {
    let th = theme::t();
    match s {
        AttentionState::Running => th.running,
        AttentionState::Idle => th.idle,
        AttentionState::Waiting => th.waiting,
    }
}

/// A progress ring with centred text lines: (text, size, colour).
fn ring(ui: &mut egui::Ui, size: f32, stroke: f32, progress: f32, color: Color32, lines: &[(String, f32, Color32)]) {
    let th = theme::t();
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size / 2.0 - stroke / 2.0;
    p.circle_stroke(c, r, Stroke::new(stroke, th.border.gamma_multiply(0.6)));
    if progress > 0.0 {
        let n = (64.0 * progress).ceil().max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = -TAU / 4.0 + TAU * progress * i as f32 / n as f32;
                Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(stroke, color)));
    }
    let total: f32 = lines.iter().map(|(_, s, _)| s * 1.25).sum();
    let mut y = c.y - total / 2.0;
    for (text, sz, col) in lines {
        p.text(Pos2::new(c.x, y + sz * 0.62), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(*sz), *col);
        y += sz * 1.25;
    }
}

/// "25:00" / "1:05:09".
pub fn fmt_countdown(ms: i64) -> String {
    let total = (ms.max(0) + 999) / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Whole minutes, "90m" / "1h 30m" style for goals/totals.
pub fn fmt_minutes(ms: i64) -> String {
    let m = (ms as f64 / MINUTE as f64).round() as i64;
    if m < 60 {
        return format!("{m}m");
    }
    let (h, rem) = (m / 60, m % 60);
    if rem == 0 {
        format!("{h}h")
    } else {
        format!("{h}h {rem}m")
    }
}

/// A short chime when a block lands (system sound; silent if unavailable).
fn chime() {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("afplay", &["/System/Library/Sounds/Glass.aiff"])
    } else {
        ("paplay", &["/usr/share/sounds/freedesktop/stereo/complete.oga"])
    };
    let _ = std::process::Command::new(cmd).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_countdown(25 * MINUTE), "25:00");
        assert_eq!(fmt_countdown(65 * MINUTE + 9_000), "1:05:09");
        assert_eq!(fmt_countdown(500), "00:01");
        assert_eq!(fmt_minutes(90 * MINUTE), "1h 30m");
        assert_eq!(fmt_minutes(45 * MINUTE), "45m");
        assert_eq!(fmt_minutes(120 * MINUTE), "2h");
    }

    #[test]
    fn lifecycle_in_memory() {
        let db = crate::db::DbState::memory().unwrap().0;
        // focus_block.session_id references session(id).
        crate::db::insert_session(&lock(&db), "s1", None, "zsh", now_ms(), 1);
        let mut p = FocusPanel::new(db);
        assert!(p.active().is_none());
        let ev = p.start(Some("s1".into()), 10 * MINUTE, Some("ship it".into()), "focus");
        assert!(matches!(ev, FocusEvent::Started { session: Some(_) }));
        assert_eq!(p.remaining_ms(p.active().unwrap().started_at + MINUTE), 9 * MINUTE);
        p.extend(5 * MINUTE);
        assert_eq!(p.active().unwrap().planned_ms, 15 * MINUTE);
        // Session closed mid-block → abandoned.
        let evs = p.tick(now_ms(), &[]);
        assert!(matches!(evs.as_slice(), [FocusEvent::Ended]));
        assert!(p.active().is_none());
        assert_eq!(p.day.as_ref().unwrap().blocks_completed, 0);
        // A block that elapses is completed and asks for a break.
        p.start(None, 0, None, "focus");
        let evs = p.tick(now_ms() + 1, &[]);
        assert!(matches!(evs.as_slice(), [FocusEvent::Ended]));
        assert!(p.completed.is_some());
        assert_eq!(p.day.as_ref().unwrap().blocks_completed, 1);
        p.set_goal(60 * MINUTE);
        assert_eq!(p.day.as_ref().unwrap().goal_ms, 60 * MINUTE);
    }
}
