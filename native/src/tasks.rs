//! Tasks sidebar — port of `TaskPanel.tsx`. Same tables (`task`, `task_event`) and the
//! same db functions as the Tauri build; the db owns the lifecycle stamps + timeline.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CornerRadius, Frame, Key, Margin, Modifiers, RichText, Sense, Stroke, Vec2};
use rusqlite::Connection;

use crate::db;
use crate::ipc::{Task, TaskEvent};
use crate::session::{lock, now_ms};
use crate::theme;
use crate::timefmt::{self, fmt_day, fmt_duration, fmt_stamp, start_of_day, DAY_MS};

const DONE_WINDOW_DAYS: i64 = 7;
const SECTIONS: [&str; 6] = ["in_progress", "blocked", "review", "todo", "backlog", "done"];
const ALL_STATUSES: [&str; 6] = ["backlog", "todo", "in_progress", "review", "blocked", "done"];
const KEY_COLLAPSED: &str = "native.tasks.collapsed";
const KEY_FILTER: &str = "native.tasks.this_project";

fn label(s: &str) -> &'static str {
    match s {
        "in_progress" => "In progress",
        "blocked" => "Blocked",
        "review" => "Review",
        "todo" => "To do",
        "backlog" => "Backlog",
        _ => "Done",
    }
}

fn next_status(s: &str) -> &'static str {
    match s {
        "backlog" => "todo",
        "todo" => "in_progress",
        "in_progress" => "review",
        "review" => "done",
        "blocked" => "in_progress",
        _ => "todo",
    }
}

fn status_color(s: &str) -> Color32 {
    match s {
        "in_progress" => theme::t().primary,
        "blocked" => theme::t().error,
        "review" => theme::t().waiting,
        "todo" => theme::t().text,
        "done" => theme::t().running,
        _ => theme::t().muted,
    }
}

const PRIORITY: [&str; 4] = ["Urgent", "High", "Normal", "Low"];

/// Edit buffers for the one expanded task.
struct Detail {
    id: String,
    title: String,
    notes: String,
    due: String,
    events: Vec<TaskEvent>,
    events_status: String,
    arm_delete: bool,
}

pub struct TaskPanel {
    db: Arc<Mutex<Connection>>,
    tasks: Vec<Task>,
    loaded_at: i64,
    draft: String,
    detail: Option<Detail>,
    collapsed: HashSet<String>,
    this_project: bool,
    error: Option<String>,
    focus_input: bool,
}

impl TaskPanel {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        let (collapsed, this_project) = {
            let conn = lock(&db);
            let c = db::get_setting(&conn, KEY_COLLAPSED)
                .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
                .unwrap_or_else(|| HashSet::from(["backlog".to_string()]));
            let f = db::get_setting(&conn, KEY_FILTER).as_deref() == Some("1");
            (c, f)
        };
        let mut p = TaskPanel {
            db,
            tasks: Vec::new(),
            loaded_at: 0,
            draft: String::new(),
            detail: None,
            collapsed,
            this_project,
            error: None,
            focus_input: true,
        };
        p.reload();
        p
    }

    pub fn focus_input(&mut self) {
        self.focus_input = true;
    }

    fn reload(&mut self) {
        let since = start_of_day(now_ms()) - (DONE_WINDOW_DAYS - 1) * DAY_MS;
        match db::list_tasks(&lock(&self.db), since) {
            Ok(t) => self.tasks = t,
            Err(e) => self.error = Some(e.to_string()),
        }
        self.loaded_at = now_ms();
    }

    /// Number of in-progress tasks (sidebar badge).
    pub fn active_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == "in_progress").count()
    }

    fn add(&mut self, status: &str, project: Option<String>) {
        let title = self.draft.trim().to_string();
        if title.is_empty() {
            return;
        }
        let t = Task {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            notes: String::new(),
            status: status.into(),
            priority: 2,
            project,
            due_at: None,
            created_at: 0,
            updated_at: 0,
            started_at: None,
            completed_at: None,
        };
        match db::insert_task(&lock(&self.db), &t, now_ms()) {
            Ok(saved) => {
                self.tasks.insert(0, saved);
                self.draft.clear();
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn save(&mut self, mut t: Task) {
        t.title = t.title.trim().to_string();
        t.priority = t.priority.clamp(0, 3);
        if t.title.is_empty() {
            return;
        }
        let res = db::update_task(&lock(&self.db), &t, now_ms());
        match res {
            Ok(Some(saved)) => {
                if let Some(slot) = self.tasks.iter_mut().find(|x| x.id == saved.id) {
                    *slot = saved;
                }
                self.error = None;
            }
            Ok(None) => self.error = Some("task not found".into()),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn delete(&mut self, id: &str) {
        match db::delete_task(&lock(&self.db), id) {
            Ok(()) => {
                self.tasks.retain(|t| t.id != id);
                self.detail = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn persist_prefs(&self) {
        let conn = lock(&self.db);
        let mut c: Vec<&str> = self.collapsed.iter().map(String::as_str).collect();
        c.sort_unstable();
        db::set_setting(&conn, KEY_COLLAPSED, &c.join(","));
        db::set_setting(&conn, KEY_FILTER, if self.this_project { "1" } else { "0" });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, project: Option<String>) {
        let now = now_ms();
        // Pick up changes made elsewhere (e.g. the Tauri app) once a minute.
        if now - self.loaded_at > 60_000 {
            self.reload();
        }
        let filter_on = self.this_project && project.is_some();
        let visible: Vec<Task> = self
            .tasks
            .iter()
            .filter(|t| !filter_on || t.project == project)
            .cloned()
            .collect();
        let today = start_of_day(now);
        let done_today = visible.iter().filter(|t| t.status == "done" && t.completed_at.unwrap_or(0) >= today).count();
        let in_progress = visible.iter().filter(|t| t.status == "in_progress").count();
        let overdue = visible
            .iter()
            .filter(|t| t.status != "done" && t.due_at.is_some_and(|d| d < today))
            .count();

        // Stats line.
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{in_progress} active · {done_today} done today")).color(theme::t().muted).size(12.0));
            if overdue > 0 {
                ui.label(RichText::new(format!("· {overdue} overdue")).color(theme::t().error).size(12.0));
            }
        });
        ui.add_space(6.0);

        // Add box: Enter → To do, ⌘Enter → In progress.
        let input = ui.add(
            egui::TextEdit::singleline(&mut self.draft)
                .hint_text("Add a task…")
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus_input) {
            input.request_focus();
        }
        // Plain Enter makes the field lose focus; ⌘Enter keeps it. Read ⌘ from the key
        // event itself — the global modifier state can lag behind it on macOS.
        let (enter, cmd) = ui.input(|i| {
            i.events.iter().fold((false, false), |acc, e| match e {
                egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } => (true, acc.1 || modifiers.command),
                _ => acc,
            })
        });
        let start_now = enter && cmd && input.has_focus();
        if start_now || (enter && !cmd && input.lost_focus()) {
            self.add(if start_now { "in_progress" } else { "todo" }, project.clone());
            input.request_focus();
        }
        ui.horizontal(|ui| {
            let hint = match &project {
                Some(p) => format!("↵ to do · ⌘↵ start now · {p}"),
                None => "↵ to do · ⌘↵ start now".into(),
            };
            ui.label(RichText::new(hint).color(theme::t().muted).size(11.0));
            if project.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let txt = if filter_on { "This project" } else { "All projects" };
                    if ui.small_button(txt).on_hover_text("Show only this project's tasks").clicked() {
                        self.this_project = !self.this_project;
                        self.persist_prefs();
                    }
                });
            }
        });
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::t().error).size(11.0));
        }
        ui.add_space(8.0);

        if visible.is_empty() {
            ui.label(RichText::new("Nothing here yet. Type above and press Enter.").color(theme::t().muted).size(12.0));
            return;
        }

        let mut to_save: Option<Task> = None;
        let mut to_delete: Option<String> = None;
        let mut toggle_section: Option<&str> = None;
        let mut toggle_open: Option<Task> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in SECTIONS {
                let mut list: Vec<&Task> = visible.iter().filter(|t| t.status == s).collect();
                if list.is_empty() {
                    continue;
                }
                if s == "done" {
                    list.sort_by_key(|t| std::cmp::Reverse(t.completed_at.unwrap_or(0)));
                } else {
                    list.sort_by(|a, b| {
                        a.priority
                            .cmp(&b.priority)
                            .then(a.due_at.unwrap_or(i64::MAX).cmp(&b.due_at.unwrap_or(i64::MAX)))
                            .then(b.created_at.cmp(&a.created_at))
                    });
                }
                let collapsed = self.collapsed.contains(s);
                let head = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new(if collapsed { "▸" } else { "▾" }).color(theme::t().muted));
                        dot(ui, status_color(s), 7.0);
                        let name = if s == "done" { format!("Done · last {DONE_WINDOW_DAYS} days") } else { label(s).to_string() };
                        ui.label(RichText::new(name).color(theme::t().text).size(12.0).strong());
                        ui.label(RichText::new(list.len().to_string()).color(theme::t().muted).size(11.0));
                    })
                    .response
                    .interact(Sense::click());
                if head.clicked() {
                    toggle_section = Some(s);
                }
                if collapsed {
                    ui.add_space(4.0);
                    continue;
                }
                for t in list {
                    let open = self.detail.as_ref().is_some_and(|d| d.id == t.id);
                    let (save, del, toggle) = self.row(ui, t, now, !filter_on, open);
                    if save.is_some() {
                        to_save = save;
                    }
                    if del.is_some() {
                        to_delete = del;
                    }
                    if toggle {
                        toggle_open = Some(t.clone());
                    }
                }
                ui.add_space(6.0);
            }
        });

        if let Some(s) = toggle_section {
            if !self.collapsed.remove(s) {
                self.collapsed.insert(s.to_string());
            }
            self.persist_prefs();
        }
        if let Some(t) = toggle_open {
            self.detail = if self.detail.as_ref().is_some_and(|d| d.id == t.id) {
                None
            } else {
                Some(Detail {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    notes: t.notes.clone(),
                    due: t.due_at.map(timefmt::to_ymd).unwrap_or_default(),
                    events: Vec::new(),
                    events_status: String::new(),
                    arm_delete: false,
                })
            };
        }
        if let Some(t) = to_save {
            self.save(t);
        }
        if let Some(id) = to_delete {
            self.delete(&id);
        }
    }

    /// One task row (+ its detail editor when open). Returns (save, delete, toggle-open).
    fn row(&mut self, ui: &mut egui::Ui, t: &Task, now: i64, show_project: bool, open: bool) -> (Option<Task>, Option<String>, bool) {
        let mut save = None;
        let mut delete = None;
        let mut toggle = false;
        let fill = if open { theme::t().border.gamma_multiply(0.45) } else { Color32::TRANSPARENT };
        Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // Status button: click advances the workflow.
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                let c = status_color(&t.status);
                let p = ui.painter();
                match t.status.as_str() {
                    "done" | "blocked" => {
                        p.circle_filled(rect.center(), 7.0, c);
                        let mark = if t.status == "done" { "✓" } else { "!" };
                        p.text(rect.center(), egui::Align2::CENTER_CENTER, mark, egui::FontId::proportional(10.0), theme::t().bg);
                    }
                    "in_progress" | "review" => {
                        p.circle_stroke(rect.center(), 6.5, Stroke::new(1.5, c));
                        p.circle_filled(rect.center(), 3.5, c);
                    }
                    "backlog" => {
                        p.circle_stroke(rect.center(), 6.5, Stroke::new(1.0, c.gamma_multiply(0.7)));
                    }
                    _ => {
                        p.circle_stroke(rect.center(), 6.5, Stroke::new(1.5, c));
                    }
                }
                let resp = resp.on_hover_text(format!("{} → {}", label(&t.status), label(next_status(&t.status))));
                if resp.clicked() {
                    let mut n = t.clone();
                    n.status = next_status(&t.status).into();
                    save = Some(n);
                }

                let body = ui
                    .vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            if t.priority < 2 {
                                let c = if t.priority == 0 { theme::t().error } else { theme::t().waiting };
                                ui.label(RichText::new(format!("P{}", t.priority)).color(c).size(10.0).strong());
                            }
                            let mut title = RichText::new(&t.title).color(theme::t().text);
                            if t.status == "done" {
                                title = title.strikethrough().color(theme::t().muted);
                            }
                            ui.add(egui::Label::new(title).wrap().selectable(false));
                        });
                        let mut meta: Vec<RichText> = Vec::new();
                        if let Some((txt, tone)) = due_info(t, now) {
                            meta.push(RichText::new(txt).color(tone));
                        }
                        if let Some(a) = age_text(t, now) {
                            meta.push(RichText::new(a).color(theme::t().muted));
                        }
                        if show_project {
                            if let Some(p) = &t.project {
                                meta.push(RichText::new(format!("● {p}")).color(theme::project_color(p)));
                            }
                        }
                        if !meta.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for m in meta {
                                    ui.add(egui::Label::new(m.size(11.0)).selectable(false));
                                }
                            });
                        }
                    })
                    .response
                    .interact(Sense::click());
                if body.clicked() {
                    toggle = true;
                }
            });

            if open {
                if let Some(d) = self.detail.as_mut() {
                    let (s, del) = detail_ui(ui, d, t, now, &self.db);
                    if s.is_some() {
                        save = s;
                    }
                    if del {
                        delete = Some(t.id.clone());
                    }
                }
            }
        });
        (save, delete, toggle)
    }
}

fn detail_ui(ui: &mut egui::Ui, d: &mut Detail, t: &Task, now: i64, dbc: &Arc<Mutex<Connection>>) -> (Option<Task>, bool) {
    let mut save = None;
    let mut delete = false;
    // Timeline reloads whenever the status changes.
    if d.events_status != t.status {
        d.events = db::task_events(&lock(dbc), &t.id).unwrap_or_default();
        d.events_status = t.status.clone();
    }
    ui.add_space(4.0);
    let title = ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(f32::INFINITY));
    if title.lost_focus() {
        let v = d.title.trim();
        if !v.is_empty() && v != t.title {
            save = Some(Task { title: v.to_string(), ..t.clone() });
        } else {
            d.title = t.title.clone();
        }
    }
    ui.horizontal(|ui| {
        let mut status = t.status.clone();
        egui::ComboBox::from_id_salt(("status", &t.id))
            .selected_text(label(&status))
            .width(100.0)
            .show_ui(ui, |ui| {
                for s in ALL_STATUSES {
                    ui.selectable_value(&mut status, s.to_string(), label(s));
                }
            });
        if status != t.status {
            save = Some(Task { status, ..t.clone() });
        }
        let mut prio = t.priority;
        egui::ComboBox::from_id_salt(("prio", &t.id))
            .selected_text(format!("P{prio}"))
            .width(60.0)
            .show_ui(ui, |ui| {
                for (i, name) in PRIORITY.iter().enumerate() {
                    ui.selectable_value(&mut prio, i as i64, format!("P{i} · {name}"));
                }
            });
        if prio != t.priority {
            save = Some(Task { priority: prio, ..t.clone() });
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Due").color(theme::t().muted).size(11.0));
        let due = ui.add(egui::TextEdit::singleline(&mut d.due).hint_text("YYYY-MM-DD").desired_width(100.0));
        if due.lost_focus() {
            let parsed = timefmt::parse_ymd_end_of_day(&d.due);
            if d.due.trim().is_empty() && t.due_at.is_some() {
                save = Some(Task { due_at: None, ..t.clone() });
            } else if parsed.is_some() && parsed != t.due_at {
                save = Some(Task { due_at: parsed, ..t.clone() });
            } else if parsed.is_none() {
                d.due = t.due_at.map(timefmt::to_ymd).unwrap_or_default();
            }
        }
        if ui.small_button("Today").clicked() {
            d.due = timefmt::to_ymd(now);
            save = Some(Task { due_at: timefmt::parse_ymd_end_of_day(&d.due), ..t.clone() });
        }
    });
    let notes = ui.add(
        egui::TextEdit::multiline(&mut d.notes)
            .hint_text("Notes, links, acceptance criteria…")
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    if notes.lost_focus() && d.notes != t.notes {
        save = Some(Task { notes: d.notes.clone(), ..t.clone() });
    }
    for ev in &d.events {
        ui.horizontal(|ui| {
            dot(ui, status_color(&ev.to_status), 6.0);
            let what = if ev.from_status.is_none() {
                format!("Created in {}", label(&ev.to_status))
            } else {
                label(&ev.to_status).to_string()
            };
            ui.label(RichText::new(what).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(fmt_stamp(ev.at, now)).color(theme::t().muted).size(11.0));
            });
        });
    }
    ui.horizontal(|ui| {
        let lead = t.completed_at.map(|c| c - t.created_at);
        let cycle = t.completed_at.zip(t.started_at).map(|(c, s)| c - s);
        let mut m = match lead {
            Some(l) => format!("Lead {}", fmt_duration(l)),
            None => format!("Open {}", fmt_duration(now - t.created_at)),
        };
        if let Some(c) = cycle {
            m.push_str(&format!(" · Cycle {}", fmt_duration(c)));
        }
        ui.label(RichText::new(m).color(theme::t().muted).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Two-step delete: first click arms, second deletes.
            let txt = if d.arm_delete { RichText::new("Click to confirm").color(theme::t().error) } else { RichText::new("Delete").color(theme::t().muted) };
            if ui.small_button(txt).clicked() {
                if d.arm_delete {
                    delete = true;
                } else {
                    d.arm_delete = true;
                }
            }
        });
    });
    if ui.input(|i| i.key_pressed(Key::Escape) && i.modifiers == Modifiers::NONE) {
        d.arm_delete = false;
    }
    (save, delete)
}

fn dot(ui: &mut egui::Ui, c: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, c);
}

fn due_info(t: &Task, now: i64) -> Option<(String, Color32)> {
    let due = t.due_at?;
    if t.status == "done" {
        return None;
    }
    let today = start_of_day(now);
    Some(if due < today {
        (format!("Overdue · {}", fmt_day(due, now)), theme::t().error)
    } else if due < today + DAY_MS {
        ("Due today".into(), theme::t().waiting)
    } else {
        (format!("Due {}", fmt_day(due, now)), theme::t().muted)
    })
}

fn age_text(t: &Task, now: i64) -> Option<String> {
    match t.status.as_str() {
        "done" => t.completed_at.map(|c| format!("✓ {}", fmt_stamp(c, now))),
        "in_progress" => t.started_at.map(|s| format!("{} in progress", fmt_duration(now - s))),
        "todo" | "backlog" => Some(format!("added {}", fmt_day(t.created_at, now).to_lowercase())),
        _ => None,
    }
}
