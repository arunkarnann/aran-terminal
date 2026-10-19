//! History sidebar — every command run in any Aran session (newest first), searchable.
//! Click a row to type it into the active terminal (without running it).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Sense};
use rusqlite::Connection;

use crate::db;
use crate::ipc::HistoryEntry;
use crate::session::{lock, now_ms};
use crate::theme;
use crate::timefmt::{fmt_duration, fmt_stamp};

const LIMIT: i64 = 300;
/// Re-query at most this often while the pane is open (new commands appear on their own).
const REFRESH_MS: i64 = 3_000;

pub struct HistoryPanel {
    db: Arc<Mutex<Connection>>,
    search: String,
    queried: Option<String>,
    rows: Vec<HistoryEntry>,
    loaded_at: i64,
}

impl HistoryPanel {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        HistoryPanel { db, search: String::new(), queried: None, rows: Vec::new(), loaded_at: 0 }
    }

    fn reload(&mut self) {
        let q = self.search.trim().to_string();
        self.rows = db::command_history(&lock(&self.db), (!q.is_empty()).then_some(q.as_str()), LIMIT);
        self.queried = Some(q);
        self.loaded_at = now_ms();
    }

    /// Draw the pane. Returns a command the user picked, to type into the terminal.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let th = theme::t();
        let now = now_ms();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search commands…")
                .desired_width(f32::INFINITY),
        );
        let stale = self.queried.as_deref() != Some(self.search.trim()) || now - self.loaded_at > REFRESH_MS;
        if stale && !resp.has_focus() || resp.changed() || self.queried.is_none() {
            self.reload();
        }
        ui.add_space(6.0);

        let mut picked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.rows.is_empty() {
                let msg = if self.search.trim().is_empty() {
                    "No commands yet — commands you run in Aran show up here."
                } else {
                    "No matching commands."
                };
                ui.label(RichText::new(msg).color(th.muted).size(12.0));
            }
            for r in &self.rows {
                let failed = r.exit_code.is_some_and(|c| c != 0);
                let resp = Frame::new()
                    .corner_radius(CornerRadius::same(5))
                    .inner_margin(Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(&r.cmdline)
                                    .monospace()
                                    .size(12.0)
                                    .color(if failed { th.error } else { th.text }),
                            )
                            .truncate()
                            .selectable(false),
                        );
                        let mut meta = Vec::new();
                        if let Some(p) = &r.project {
                            meta.push(p.clone());
                        }
                        if let Some(t) = r.finished_at {
                            meta.push(fmt_stamp(t, now));
                        }
                        if let Some(d) = r.duration_ms.filter(|d| *d >= 1000) {
                            meta.push(fmt_duration(d));
                        }
                        if let Some(c) = r.exit_code.filter(|c| *c != 0) {
                            meta.push(format!("exit {c}"));
                        }
                        ui.label(RichText::new(meta.join(" · ")).color(th.muted).size(10.5));
                    })
                    .response
                    .interact(Sense::click());
                let resp = resp.on_hover_text("Click to type into the terminal · right-click to copy");
                if resp.clicked() {
                    picked = Some(r.cmdline.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(r.cmdline.clone());
                        ui.close();
                    }
                    if ui.button("Type into terminal").clicked() {
                        picked = Some(r.cmdline.clone());
                        ui.close();
                    }
                });
            }
        });
        picked
    }
}
