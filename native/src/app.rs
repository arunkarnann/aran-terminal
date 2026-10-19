//! Window layout — two-row tab groups (one group per project folder), a sidebar with
//! Dashboard + Tasks panes (right side by default), the terminal, and a status bar.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Frame, Key, Margin, Modifiers, RichText, Sense, Stroke, Vec2};

use crate::db;
use crate::ipc::{AttentionState, SessionId, Summary};
use crate::session::{self, lock, Core, CreateError, GridSize};
use crate::beads::IssuesPanel;
use crate::git_panel::GitPanel;
use crate::history::HistoryPanel;
use crate::tasks::TaskPanel;
use crate::term_view::{self, CellMetrics, TermViewState};
use crate::theme;

const DEFAULT_FONT_SIZE: f32 = 13.0;
const KEY_THEME: &str = "native.theme";
const KEY_SIDE: &str = "native.sidebar.side";
const KEY_TAB: &str = "native.sidebar.tab";
const KEY_OPEN: &str = "native.sidebar.open";
const KEY_COLORS: &str = "native.project_colors";
const KEY_SUGGEST: &str = "native.suggestions";
const KEY_UPDATE_AUTO: &str = "native.update.auto";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Dashboard,
    Tasks,
    Issues,
    Git,
    History,
}

/// Issues + Git panes are still being built; flip on when they're done.
const NEW_PANES: bool = false;

impl Pane {
    const ALL: [Pane; 5] = [Pane::Dashboard, Pane::Tasks, Pane::Issues, Pane::Git, Pane::History];

    fn enabled(self) -> bool {
        NEW_PANES || !matches!(self, Pane::Issues | Pane::Git)
    }

    /// Stored in settings, so keep these stable.
    fn id(self) -> &'static str {
        match self {
            Pane::Dashboard => "dashboard",
            Pane::Tasks => "tasks",
            Pane::Issues => "issues",
            Pane::Git => "git",
            Pane::History => "history",
        }
    }

    fn from_id(s: &str) -> Pane {
        Pane::ALL.into_iter().find(|p| p.id() == s && p.enabled()).unwrap_or(Pane::Dashboard)
    }

    fn title(self) -> &'static str {
        match self {
            Pane::Dashboard => "Dashboard",
            Pane::Tasks => "Tasks",
            Pane::Issues => "Issues",
            Pane::Git => "Git",
            Pane::History => "History",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// One tab group = all sessions in the same project folder.
struct Group {
    key: String,
    project: Option<String>,
    cwd: Option<String>,
    sessions: Vec<SessionId>,
}

struct TabInfo {
    id: SessionId,
    label: String,
    state: AttentionState,
    project: Option<String>,
    cwd: Option<String>,
}

pub struct AranApp {
    core: Core,
    active: Option<SessionId>,
    views: HashMap<SessionId, TermViewState>,
    metrics: Option<CellMetrics>,
    font_size: f32,
    last_grid: GridSize,
    show_sidebar: bool,
    side: Side,
    pane: Pane,
    tasks: TaskPanel,
    issues: IssuesPanel,
    git: GitPanel,
    history: HistoryPanel,
    /// Last active tab per group, so clicking a group returns you to where you were.
    last_in_group: HashMap<String, SessionId>,
    confirm_close_group: Option<(String, Vec<SessionId>)>,
    cap_dialog: Option<usize>,
    renaming: Option<(SessionId, String)>,
    summary: Option<(Instant, Summary)>,
    app_rss_kb: Arc<AtomicI64>,
    /// Hand keyboard focus back to the terminal next frame.
    focus_terminal: bool,
    /// Command autocomplete at the prompt (Settings).
    suggestions_on: bool,
    updater: crate::updater::Updater,
    update_auto: bool,
    confirm_update: Option<crate::updater::Release>,
    error: Option<String>,
}

impl AranApp {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Core) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        let (theme_idx, side, pane, open) = {
            let conn = lock(&core.db);
            let th = db::get_setting(&conn, KEY_THEME).and_then(|v| theme::index_of(&v)).unwrap_or(0);
            let side = match db::get_setting(&conn, KEY_SIDE).as_deref() {
                Some("left") => Side::Left,
                _ => Side::Right,
            };
            let pane = Pane::from_id(db::get_setting(&conn, KEY_TAB).as_deref().unwrap_or(""));
            let open = db::get_setting(&conn, KEY_OPEN).as_deref() != Some("0");
            (th, side, pane, open)
        };
        theme::apply(&cc.egui_ctx, theme_idx);
        if let Some(c) = db::get_setting(&lock(&core.db), KEY_COLORS) {
            theme::load_project_colors(&c);
        }

        let app_rss_kb = Arc::new(AtomicI64::new(0));
        {
            let rss = app_rss_kb.clone();
            let _ = std::thread::Builder::new().name("app-mem".into()).spawn(move || loop {
                if let Some(kb) = session::app_rss_kb() {
                    rss.store(kb, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_secs(5));
            });
        }
        let tasks = TaskPanel::new(core.db.clone());
        let issues = IssuesPanel::new(cc.egui_ctx.clone());
        let git = GitPanel::new(cc.egui_ctx.clone());
        let history = HistoryPanel::new(core.db.clone());
        let mut app = AranApp {
            core,
            active: None,
            views: HashMap::new(),
            metrics: None,
            font_size: DEFAULT_FONT_SIZE,
            last_grid: GridSize { cols: 100, rows: 30 },
            show_sidebar: open,
            side,
            pane,
            tasks,
            issues,
            git,
            history,
            last_in_group: HashMap::new(),
            confirm_close_group: None,
            cap_dialog: None,
            renaming: None,
            summary: None,
            app_rss_kb,
            focus_terminal: false,
            suggestions_on: true,
            updater: crate::updater::Updater::new(cc.egui_ctx.clone()),
            update_auto: true,
            confirm_update: None,
            error: None,
        };
        app.suggestions_on = db::get_setting(&lock(&app.core.db), KEY_SUGGEST).as_deref() != Some("0");
        app.update_auto = db::get_setting(&lock(&app.core.db), KEY_UPDATE_AUTO).as_deref() != Some("0");
        if app.update_auto {
            app.updater.start_background_checks();
        }
        app.new_tab(None);
        app
    }

    fn setting(&self, key: &str, value: &str) {
        db::set_setting(&lock(&self.core.db), key, value);
    }

    // ---------------- sessions ----------------

    /// Open a terminal in `cwd` (None = home folder, a fresh group).
    fn new_tab(&mut self, cwd: Option<String>) {
        match self.core.create_session(cwd, self.last_grid) {
            Ok(id) => self.active = Some(id),
            Err(CreateError::CapReached { limit }) => self.cap_dialog = Some(limit),
            Err(CreateError::Spawn(e)) => self.error = Some(format!("Could not start shell: {e}")),
        }
    }

    fn active_cwd(&self) -> Option<String> {
        let g = lock(&self.core.mgr);
        self.active.as_ref().and_then(|id| g.sessions.get(id)).and_then(|s| s.cwd.clone())
    }

    /// Type text at the active terminal's prompt (not submitted).
    fn type_into_active(&mut self, text: &str) {
        if let Some(id) = &self.active {
            if let Some(s) = lock(&self.core.mgr).sessions.get(id) {
                s.write(text.replace(['\r', '\n'], " ").as_bytes());
            }
        }
        self.focus_terminal = true;
    }

    fn active_project(&self) -> Option<String> {
        let g = lock(&self.core.mgr);
        self.active.as_ref().and_then(|id| g.sessions.get(id)).and_then(|s| s.project.clone())
    }

    fn close_tab(&mut self, id: &str) {
        // Pick the neighbour in the visible (grouped) order before removing.
        let order = self.flat_order();
        let pos = order.iter().position(|x| x == id);
        {
            let mut g = lock(&self.core.mgr);
            g.close(id);
        }
        db::mark_session_closed(&lock(&self.core.db), id, session::now_ms());
        self.views.remove(id);
        if self.active.as_deref() == Some(id) {
            let rest: Vec<&SessionId> = order.iter().filter(|x| *x != id).collect();
            self.active = pos.and_then(|p| rest.get(p.min(rest.len().saturating_sub(1))).map(|s| (*s).clone()));
        }
    }

    fn tab_infos(&self) -> Vec<TabInfo> {
        let g = lock(&self.core.mgr);
        g.order
            .iter()
            .filter_map(|id| {
                g.sessions.get(id).map(|s| TabInfo {
                    id: id.clone(),
                    label: s.label(),
                    state: s.state,
                    project: s.project.clone(),
                    cwd: s.cwd.clone(),
                })
            })
            .collect()
    }

    fn groups(&self, tabs: &[TabInfo]) -> Vec<Group> {
        let mut out: Vec<Group> = Vec::new();
        for t in tabs {
            let key = t.project.clone().unwrap_or_else(|| "__loose__".into());
            match out.iter_mut().find(|g| g.key == key) {
                Some(g) => {
                    if g.cwd.is_none() {
                        g.cwd = t.cwd.clone();
                    }
                    g.sessions.push(t.id.clone());
                }
                None => out.push(Group { key, project: t.project.clone(), cwd: t.cwd.clone(), sessions: vec![t.id.clone()] }),
            }
        }
        out
    }

    /// Tabs in on-screen order (group by group).
    fn flat_order(&self) -> Vec<SessionId> {
        let tabs = self.tab_infos();
        self.groups(&tabs).into_iter().flat_map(|g| g.sessions).collect()
    }

    // ---------------- shortcuts ----------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        let consume = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));

        if consume(cmd, Key::T) {
            let cwd = self.active_cwd();
            self.new_tab(cwd);
        }
        if consume(cmd, Key::N) {
            self.new_tab(None);
        }
        if consume(cmd, Key::W) {
            if let Some(id) = self.active.clone() {
                self.close_tab(&id);
            }
        }
        if consume(cmd, Key::B) {
            self.toggle_pane(Pane::Dashboard);
        }
        if consume(cmd, Key::J) {
            self.toggle_pane(Pane::Tasks);
        }
        if NEW_PANES && consume(cmd, Key::I) {
            self.toggle_pane(Pane::Issues);
        }
        if NEW_PANES && consume(shift_cmd, Key::G) {
            self.toggle_pane(Pane::Git);
        }
        if consume(cmd, Key::Y) {
            self.toggle_pane(Pane::History);
        }
        if consume(cmd, Key::Plus) || consume(cmd, Key::Equals) {
            self.font_size = (self.font_size + 1.0).min(32.0);
            self.metrics = None;
        }
        if consume(cmd, Key::Minus) {
            self.font_size = (self.font_size - 1.0).max(8.0);
            self.metrics = None;
        }
        if consume(cmd, Key::Num0) {
            self.font_size = DEFAULT_FONT_SIZE;
            self.metrics = None;
        }
        if consume(cmd, Key::K) {
            let term = self.active.as_ref().and_then(|id| lock(&self.core.mgr).sessions.get(id).map(|s| s.term.clone()));
            if let Some(t) = term {
                t.lock().grid_mut().clear_history();
            }
        }
        let order = self.flat_order();
        let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in digits.iter().enumerate() {
            if consume(cmd, *k) {
                let idx = if i == 8 { order.len().saturating_sub(1) } else { i };
                if let Some(id) = order.get(idx) {
                    self.active = Some(id.clone());
                }
            }
        }
        if let Some(p) = self.active.as_ref().and_then(|a| order.iter().position(|x| x == a)) {
            if consume(shift_cmd, Key::CloseBracket) {
                self.active = order.get((p + 1) % order.len()).cloned();
            }
            if consume(shift_cmd, Key::OpenBracket) {
                self.active = order.get((p + order.len() - 1) % order.len()).cloned();
            }
        }
    }

    /// Show `pane`; if it's already showing, hide the sidebar.
    fn toggle_pane(&mut self, pane: Pane) {
        if self.show_sidebar && self.pane == pane {
            self.show_sidebar = false;
        } else {
            self.show_sidebar = true;
            self.pane = pane;
            if pane == Pane::Tasks {
                self.tasks.focus_input();
            }
        }
        self.setting(KEY_OPEN, if self.show_sidebar { "1" } else { "0" });
        self.setting(KEY_TAB, self.pane.id());
    }

    // ---------------- header: group row + tab row ----------------

    fn header(&mut self, ui: &mut egui::Ui) {
        let tabs = self.tab_infos();
        let groups = self.groups(&tabs);
        let active_gi = groups
            .iter()
            .position(|g| self.active.as_ref().is_some_and(|a| g.sessions.contains(a)))
            .unwrap_or(0);
        if let (Some(a), Some(g)) = (self.active.clone(), groups.get(active_gi)) {
            self.last_in_group.insert(g.key.clone(), a);
        }
        let th = theme::t();

        let mut activate: Option<SessionId> = None;
        let mut close: Option<SessionId> = None;
        let mut new_in: Option<Option<String>> = None;
        let mut colors_changed = false;

        // Row 1 — group chips + global actions.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (gi, g) in groups.iter().enumerate() {
                let is_active = gi == active_gi;
                let color = g.project.as_deref().map(theme::project_color).unwrap_or(th.muted);
                let (fill, ink) = if is_active { (color, theme::ink_on(color)) } else { (Color32::TRANSPARENT, th.text) };
                let resp = Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, if is_active { color } else { th.border }))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(10, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            if !is_active {
                                dot(ui, color, 8.0);
                            }
                            let name = g.project.clone().unwrap_or_else(|| "Loose".into());
                            ui.add(egui::Label::new(RichText::new(name).color(ink).strong().size(12.0)).selectable(false));
                            ui.add(egui::Label::new(RichText::new(g.sessions.len().to_string()).color(ink.gamma_multiply(0.7)).size(11.0)).selectable(false));
                            let x = ui
                                .add(egui::Label::new(RichText::new("×").color(ink.gamma_multiply(0.7))).sense(Sense::click()))
                                .on_hover_text(format!("Close group ({} tabs)", g.sessions.len()));
                            if x.clicked() {
                                if g.sessions.len() == 1 {
                                    close = Some(g.sessions[0].clone());
                                } else {
                                    self.confirm_close_group =
                                        Some((g.project.clone().unwrap_or_else(|| "Loose".into()), g.sessions.clone()));
                                }
                            }
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_text(g.cwd.clone().unwrap_or_default());
                if let Some(p) = &g.project {
                    resp.context_menu(|ui| {
                        if color_menu(ui, p) {
                            self.save_colors();
                        }
                        if let Some(cwd) = &g.cwd {
                            ui.separator();
                            if ui.button("Reveal in Finder").clicked() {
                                reveal_in_finder(cwd);
                                ui.close();
                            }
                        }
                    });
                }
                if resp.clicked() && !is_active {
                    let target = self
                        .last_in_group
                        .get(&g.key)
                        .filter(|id| g.sessions.contains(id))
                        .cloned()
                        .or_else(|| g.sessions.first().cloned());
                    activate = target;
                }
            }
            if ui.add(egui::Button::new(RichText::new("＋ New").size(12.0)).frame(false)).on_hover_text("New terminal in your home folder  ⌘N").clicked() {
                new_in = Some(None);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.header_actions(ui);
            });
        });

        ui.add_space(4.0);

        // Row 2 — tabs of the active group.
        let Some(g) = groups.get(active_gi) else {
            if let Some(cwd) = new_in {
                self.new_tab(cwd);
            }
            return;
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for id in &g.sessions {
                let Some(t) = tabs.iter().find(|t| &t.id == id) else { continue };
                let is_active = self.active.as_ref() == Some(id);

                if let Some((rid, buf)) = self.renaming.as_mut().filter(|(rid, _)| rid == id) {
                    let r = ui.add(egui::TextEdit::singleline(buf).desired_width(140.0));
                    r.request_focus();
                    if r.lost_focus() {
                        let name = buf.trim().to_string();
                        let rid = rid.clone();
                        if let Some(s) = lock(&self.core.mgr).sessions.get_mut(&rid) {
                            s.name = (!name.is_empty()).then_some(name);
                        }
                        self.renaming = None;
                    }
                    continue;
                }

                let resp = Frame::new()
                    .fill(if is_active { th.bg } else { Color32::TRANSPARENT })
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if let Some(p) = &t.project {
                                let sw = ui
                                    .menu_button(RichText::new("■").color(theme::project_color(p)).size(12.0), |ui| {
                                        if color_menu(ui, p) {
                                            colors_changed = true;
                                        }
                                    })
                                    .response;
                                sw.on_hover_text(format!("{p} — click to change colour"));
                            }
                            dot(ui, state_color(t.state), 8.0);
                            let c = if is_active { th.text } else { th.muted };
                            ui.add(egui::Label::new(RichText::new(truncate(&t.label, 30)).color(c)).selectable(false));
                            if let Some(cwd) = &t.cwd {
                                let f = folder_icon(ui).on_hover_text("Reveal in Finder");
                                if f.clicked() {
                                    reveal_in_finder(cwd);
                                }
                            }
                            let x = ui.add(egui::Label::new(RichText::new("×").color(th.muted)).sense(Sense::click()));
                            if x.clicked() {
                                close = Some(id.clone());
                            }
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_text(t.cwd.clone().unwrap_or_default());
                if resp.clicked() {
                    activate = Some(id.clone());
                }
                if resp.double_clicked() {
                    let current = lock(&self.core.mgr).sessions.get(id).and_then(|s| s.name.clone()).unwrap_or_default();
                    self.renaming = Some((id.clone(), current));
                }
                if resp.middle_clicked() {
                    close = Some(id.clone());
                }
            }
            let hint = match &g.project {
                Some(p) => format!("New tab in {p}  ⌘T"),
                None => "New tab in this folder  ⌘T".into(),
            };
            if ui.add(egui::Button::new(RichText::new("+").size(15.0)).frame(false)).on_hover_text(hint).clicked() {
                // Prefer the active tab's own folder, else the group's.
                let cwd = self.active_cwd().or_else(|| g.cwd.clone());
                new_in = Some(cwd);
            }
        });

        if let Some(id) = activate {
            self.active = Some(id);
        }
        if let Some(id) = close {
            self.close_tab(&id);
        }
        if let Some(cwd) = new_in {
            self.new_tab(cwd);
        }
        if colors_changed {
            self.save_colors();
        }
    }

    fn save_colors(&self) {
        self.setting(KEY_COLORS, &theme::project_colors_string());
    }

    /// Right side of the group row: sidebar toggles, side swap, theme menu.
    fn header_actions(&mut self, ui: &mut egui::Ui) {
        let th = theme::t();
        // (Right-to-left layout: first added = rightmost.)
        self.update_chip(ui);
        let side_icon = if self.side == Side::Right { "⇤" } else { "⇥" };
        if ui.add(egui::Button::new(RichText::new(side_icon).size(13.0)).frame(false)).on_hover_text("Move sidebar to the other side").clicked() {
            self.side = if self.side == Side::Right { Side::Left } else { Side::Right };
            self.setting(KEY_SIDE, if self.side == Side::Left { "left" } else { "right" });
        }

        let hist_on = self.show_sidebar && self.pane == Pane::History;
        if ui.add(egui::Button::selectable(hist_on, RichText::new("History").size(12.0))).on_hover_text("Command history  ⌘Y").clicked() {
            self.toggle_pane(Pane::History);
        }
        let git_on = self.show_sidebar && self.pane == Pane::Git;
        if NEW_PANES && ui.add(egui::Button::selectable(git_on, RichText::new("Git").size(12.0))).on_hover_text("Git panel  ⌘⇧G").clicked() {
            self.toggle_pane(Pane::Git);
        }
        let issues_on = self.show_sidebar && self.pane == Pane::Issues;
        let issues_label = match self.issues.open_count() {
            Some(n) if n > 0 => format!("Issues {n}"),
            _ => "Issues".into(),
        };
        if NEW_PANES && ui.add(egui::Button::selectable(issues_on, RichText::new(issues_label).size(12.0))).on_hover_text("Beads issues for this project  ⌘I").clicked() {
            self.toggle_pane(Pane::Issues);
        }
        let tasks_on = self.show_sidebar && self.pane == Pane::Tasks;
        let n = self.tasks.active_count();
        let tasks_label = if n > 0 { format!("Tasks {n}") } else { "Tasks".into() };
        if ui.add(egui::Button::selectable(tasks_on, RichText::new(tasks_label).size(12.0))).on_hover_text("To-do list  ⌘J").clicked() {
            self.toggle_pane(Pane::Tasks);
        }
        let dash_on = self.show_sidebar && self.pane == Pane::Dashboard;
        if ui.add(egui::Button::selectable(dash_on, RichText::new("Dashboard").size(12.0))).on_hover_text("Focus dashboard  ⌘B").clicked() {
            self.toggle_pane(Pane::Dashboard);
        }

        let mut chosen = None;
        ui.menu_button(RichText::new(format!("◐ {}", th.name)).size(12.0), |ui| {
            for (i, t) in theme::THEMES.iter().enumerate() {
                if ui.selectable_label(i == theme::current_index(), t.name).clicked() {
                    chosen = Some(i);
                    ui.close();
                }
            }
        });
        if let Some(i) = chosen {
            theme::apply(ui.ctx(), i);
            self.setting(KEY_THEME, theme::THEMES[i].id);
        }
    }

    /// Header chip for OTA updates: offer → installing → error.
    fn update_chip(&mut self, ui: &mut egui::Ui) {
        use crate::updater::Status;
        let th = theme::t();
        match self.updater.status() {
            Status::Available(r) => {
                let b = egui::Button::new(RichText::new(format!("⬆ Update {}", r.version)).size(12.0).color(theme::ink_on(th.primary)))
                    .fill(th.primary);
                if ui.add(b).on_hover_text("A new version of Aran Terminal is available").clicked() {
                    self.confirm_update = Some(r);
                }
            }
            Status::Installing(msg) => {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(msg).size(11.0).color(th.muted));
            }
            Status::Failed(e) => {
                if ui.add(egui::Button::new(RichText::new("✕").size(11.0).color(th.muted)).frame(false)).clicked() {
                    self.updater.dismiss();
                }
                ui.add(egui::Label::new(RichText::new(format!("Update: {e}")).size(11.0).color(th.error)).truncate());
            }
            _ => {}
        }
    }

    // ---------------- sidebar ----------------

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let th = theme::t();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for p in Pane::ALL.into_iter().filter(|p| p.enabled()) {
                if ui.add(egui::Button::selectable(self.pane == p, RichText::new(p.title()).strong().size(12.0))).clicked() && self.pane != p {
                    self.pane = p;
                    self.setting(KEY_TAB, p.id());
                    if p == Pane::Tasks {
                        self.tasks.focus_input();
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("✕").color(th.muted)).frame(false)).on_hover_text("Hide sidebar").clicked() {
                    self.show_sidebar = false;
                    self.setting(KEY_OPEN, "0");
                }
            });
        });
        ui.separator();
        match self.pane {
            Pane::Dashboard => self.dashboard(ui),
            Pane::Tasks => {
                let project = self.active_project();
                self.tasks.show(ui, project);
            }
            Pane::Issues => {
                let cwd = self.active_cwd();
                self.issues.show(ui, cwd);
            }
            Pane::Git => {
                let cwd = self.active_cwd();
                self.git.show(ui, cwd);
            }
            Pane::History => {
                if let Some(cmd) = self.history.show(ui) {
                    self.type_into_active(&cmd);
                }
            }
        }
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let th = theme::t();
        let now = session::now_ms();
        struct Row {
            id: SessionId,
            label: String,
            project: Option<String>,
            cwd: Option<String>,
            state: AttentionState,
            uptime_ms: i64,
            cmds: i64,
            rss_kb: Option<i64>,
        }
        let rows: Vec<Row> = {
            let g = lock(&self.core.mgr);
            g.order
                .iter()
                .filter_map(|id| g.sessions.get(id).map(|s| (id, s)))
                .map(|(id, s)| Row {
                    id: id.clone(),
                    label: s.label(),
                    project: s.project.clone(),
                    cwd: s.cwd.clone(),
                    state: s.state,
                    uptime_ms: now - s.created_at,
                    cmds: s.command_count,
                    rss_kb: s.rss_kb,
                })
                .collect()
        };
        let mut activate = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "NEEDS YOU");
            let waiting: Vec<&Row> = rows.iter().filter(|r| r.state == AttentionState::Waiting).collect();
            if waiting.is_empty() {
                ui.label(RichText::new("Nothing waiting").color(th.muted).size(12.0));
            }
            for r in waiting {
                let resp = Frame::new()
                    .fill(th.waiting.gamma_multiply(0.15))
                    .stroke(Stroke::new(1.0, th.waiting.gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            dot(ui, state_color(r.state), 8.0);
                            ui.add(egui::Label::new(RichText::new(&r.label).color(th.text).strong()).truncate());
                        });
                        let p = r.project.clone().unwrap_or_else(|| "Loose".into());
                        ui.label(RichText::new(format!("{p} · waiting for input")).color(th.waiting).size(11.0));
                    })
                    .response
                    .interact(Sense::click());
                if resp.clicked() {
                    activate = Some(r.id.clone());
                }
            }

            ui.add_space(12.0);
            section(ui, "SESSIONS");
            let mut projects: Vec<Option<String>> = Vec::new();
            for r in &rows {
                if !projects.contains(&r.project) {
                    projects.push(r.project.clone());
                }
            }
            for p in projects {
                let name = p.clone().unwrap_or_else(|| "Loose".into());
                let color = p.as_deref().map(theme::project_color).unwrap_or(th.muted);
                ui.horizontal(|ui| {
                    dot(ui, color, 7.0);
                    ui.label(RichText::new(name).color(color).size(11.0).strong());
                });
                for r in rows.iter().filter(|r| r.project == p) {
                    let is_active = self.active.as_ref() == Some(&r.id);
                    let resp = Frame::new()
                        .fill(if is_active { th.border.gamma_multiply(0.5) } else { Color32::TRANSPARENT })
                        .corner_radius(CornerRadius::same(5))
                        .inner_margin(Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                dot(ui, state_color(r.state), 8.0);
                                ui.add(egui::Label::new(RichText::new(truncate(&r.label, 30)).color(th.text)).selectable(false));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if let Some(cwd) = &r.cwd {
                                        let f = folder_icon(ui).on_hover_text("Reveal in Finder");
                                        if f.clicked() {
                                            reveal_in_finder(cwd);
                                        }
                                    }
                                    ui.label(RichText::new(state_name(r.state)).color(state_color(r.state)).size(10.0));
                                });
                            });
                            let mem = r.rss_kb.map(fmt_kb).unwrap_or_default();
                            ui.label(
                                RichText::new(format!("{} · {} cmds  {}", fmt_dur(r.uptime_ms), r.cmds, mem))
                                    .color(th.muted)
                                    .size(11.0),
                            );
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        activate = Some(r.id.clone());
                    }
                }
                ui.add_space(4.0);
            }

            ui.add_space(12.0);
            section(ui, "TODAY");
            if let Some((_, s)) = &self.summary {
                stat(ui, "Sessions opened", s.sessions_opened.to_string());
                stat(ui, "Commands run", s.commands_run.to_string());
                stat(ui, "Agents blocked", fmt_dur(s.agent_blocked_ms));
                for p in s.per_project.iter().take(5) {
                    stat(ui, &format!("  {}", p.project), fmt_dur(p.active_ms));
                }
            }

            ui.add_space(12.0);
            egui::CollapsingHeader::new(RichText::new("SETTINGS").color(th.muted).size(11.0))
                .default_open(false)
                .show(ui, |ui| {
                    let mut cap = lock(&self.core.mgr).cap;
                    if ui.add(egui::Slider::new(&mut cap, 1..=20).text("max sessions")).changed() {
                        lock(&self.core.mgr).cap = cap;
                    }
                    if ui
                        .checkbox(&mut self.suggestions_on, "Command suggestions")
                        .on_hover_text("Grey autocomplete from your history at the prompt: → accept, ↓ list, Esc hide")
                        .changed()
                    {
                        db::set_setting(&lock(&self.core.db), KEY_SUGGEST, if self.suggestions_on { "1" } else { "0" });
                    }
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.update_auto, "Check for updates").changed() {
                            db::set_setting(&lock(&self.core.db), KEY_UPDATE_AUTO, if self.update_auto { "1" } else { "0" });
                            if self.update_auto {
                                self.updater.start_background_checks();
                            }
                        }
                        if ui.small_button("Check now").clicked() {
                            self.updater.check_now();
                        }
                    });
                    let note = match self.updater.status() {
                        crate::updater::Status::Checking => "checking…".to_string(),
                        crate::updater::Status::UpToDate => "you're up to date".to_string(),
                        crate::updater::Status::Available(r) => format!("{} available", r.version),
                        _ => String::new(),
                    };
                    ui.label(RichText::new(format!("Aran Terminal {}  {note}", crate::updater::CURRENT)).size(11.0).color(th.muted));
                    let mut secs = lock(&self.core.det).t_wait_secs();
                    if ui.add(egui::Slider::new(&mut secs, 2..=60).text("waiting after (s)")).changed() {
                        lock(&self.core.det).set_t_wait_secs(secs);
                        db::set_setting(&lock(&self.core.db), "t_wait_secs", &secs.to_string());
                    }
                });
        });

        if let Some(id) = activate {
            self.active = Some(id);
        }
    }

    fn refresh_summary(&mut self) {
        let stale = self.summary.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(30));
        if stale {
            let overrides = lock(&self.core.mgr).override_count as i64;
            let since = crate::timefmt::start_of_day(session::now_ms());
            let s = db::daily_summary(&lock(&self.core.db), since, session::now_ms(), overrides);
            self.summary = Some((Instant::now(), s));
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let th = theme::t();
        let (n, cap, waiting) = {
            let g = lock(&self.core.mgr);
            let w = g.sessions.values().filter(|s| s.state == AttentionState::Waiting).count();
            (g.sessions.len(), g.cap, w)
        };
        ui.horizontal(|ui| {
            let small = |t: String, c: Color32| RichText::new(t).size(11.0).color(c);
            ui.label(small(format!("{n}/{cap} sessions"), th.muted));
            if waiting > 0 {
                ui.label(small(format!("● {waiting} waiting"), th.waiting));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let kb = self.app_rss_kb.load(Ordering::Relaxed);
                if kb > 0 {
                    ui.label(small(format!("app {}", fmt_kb(kb)), th.muted));
                }
                ui.label(small("⌘T tab  ⌘N new folder  ⌘B dashboard  ⌘J tasks  ⌘Y history".into(), th.muted));
            });
        });
    }
}

impl eframe::App for AranApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.metrics.is_none() {
            self.metrics = Some(CellMetrics::new(&ctx, self.font_size));
        }

        // Shells that exited on their own.
        let dead = lock(&self.core.mgr).reap_exited();
        for id in &dead {
            self.views.remove(id);
        }
        if self.active.as_ref().is_some_and(|a| dead.contains(a)) || self.active.is_none() {
            self.active = self.flat_order().last().cloned();
        }
        if lock(&self.core.mgr).order.is_empty() && !dead.is_empty() {
            // Last shell exited (e.g. `exit`): close the window like Terminal.app.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        self.shortcuts(&ctx);
        let th = theme::t();

        egui::Panel::top("tabs")
            .frame(Frame::new().fill(th.panel).inner_margin(Margin::symmetric(8, 6)))
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui));

        egui::Panel::bottom("status")
            .frame(Frame::new().fill(th.panel).inner_margin(Margin::symmetric(10, 3)))
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));

        if self.show_sidebar {
            if self.pane == Pane::Dashboard {
                self.refresh_summary();
            }
            let panel = match self.side {
                Side::Right => egui::Panel::right("sidebar"),
                Side::Left => egui::Panel::left("sidebar"),
            };
            panel
                .frame(Frame::new().fill(th.panel).inner_margin(Margin::same(12)))
                .default_size(300.0)
                .size_range(240.0..=520.0)
                .resizable(true)
                .show_separator_line(true)
                .show(ui, |ui| self.sidebar(ui));
            // Uptimes / task ages are minute-grained; refresh occasionally, not per frame.
            ctx.request_repaint_after(Duration::from_secs(30));
        }

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let Some(metrics) = self.metrics.as_ref() else { return };
            let Some(id) = self.active.clone() else {
                ui.centered_and_justified(|ui| {
                    if ui.button("New terminal  ⌘N").clicked() {
                        self.new_tab(None);
                    }
                });
                return;
            };
            let mut g = lock(&self.core.mgr);
            let Some(s) = g.sessions.get_mut(&id) else { return };
            let view = self.views.entry(id).or_default();
            let db = self.core.db.clone();
            let lookup = move |prefix: &str, cwd: Option<&str>| {
                db::command_suggestions(&lock(&db), prefix, cwd, crate::suggest::MAX_SUGGESTIONS)
            };
            let suggest: Option<term_view::SuggestFn<'_>> = self.suggestions_on.then_some(&lookup);
            term_view::show(ui, s, view, metrics, suggest, std::mem::take(&mut self.focus_terminal));
            self.last_grid = s.size;
        });

        if let Some((name, ids)) = self.confirm_close_group.clone() {
            egui::Window::new("Close group?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(format!("This closes all {} terminals in {name}. This can't be undone.", ids.len()));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(format!("Close all {}", ids.len())).color(th.error)).clicked() {
                            for id in &ids {
                                self.close_tab(id);
                            }
                            self.confirm_close_group = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_close_group = None;
                        }
                    });
                });
        }

        if let Some(limit) = self.cap_dialog {
            egui::Window::new("Session limit reached")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(format!("You have {limit} sessions open — the limit you set to stay focused."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open anyway").clicked() {
                            lock(&self.core.mgr).raise_cap();
                            self.cap_dialog = None;
                            let cwd = self.active_cwd();
                            self.new_tab(cwd);
                        }
                        if ui.button("Cancel").clicked() {
                            self.cap_dialog = None;
                        }
                    });
                });
        }

        if let crate::updater::Status::Installed(path) = self.updater.status() {
            // Bundle swapped: quit, and the new version opens itself.
            self.updater.dismiss(); // once only
            crate::updater::relaunch(&path);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if let Some(r) = self.confirm_update.clone() {
            let open = lock(&self.core.mgr).sessions.len();
            egui::Window::new(format!("Update to {}?", r.version))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(format!("You're on {}. Aran restarts after the update.", crate::updater::CURRENT));
                    if open > 0 {
                        ui.label(RichText::new(format!("Your {open} open terminal(s) will be closed.")).color(th.waiting));
                    }
                    if !r.notes.trim().is_empty() {
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                            ui.label(RichText::new(r.notes.trim()).size(11.5).color(th.muted));
                        });
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Update & restart").clicked() {
                            self.updater.install(r.clone());
                            self.confirm_update = None;
                        }
                        if ui.button("Release page").clicked() {
                            crate::updater::open_url(&r.page_url);
                        }
                        if ui.button("Later").clicked() {
                            self.confirm_update = None;
                        }
                    });
                });
        }

        if let Some(err) = self.error.clone() {
            egui::Window::new("Error").collapsible(false).show(&ctx, |ui| {
                ui.label(err);
                if ui.button("OK").clicked() {
                    self.error = None;
                }
            });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.core.shutdown();
    }
}

// ---------------- helpers ----------------

fn state_color(s: AttentionState) -> Color32 {
    let th = theme::t();
    match s {
        AttentionState::Running => th.running,
        AttentionState::Idle => th.idle,
        AttentionState::Waiting => th.waiting,
    }
}

fn state_name(s: AttentionState) -> &'static str {
    match s {
        AttentionState::Running => "RUNNING",
        AttentionState::Idle => "IDLE",
        AttentionState::Waiting => "WAITING",
    }
}

/// Small clickable folder glyph, drawn (no font has one).
fn folder_icon(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 12.0), Sense::click());
    let c = if resp.hovered() { theme::t().text } else { theme::t().muted };
    let p = ui.painter();
    let body = egui::Rect::from_min_max(rect.min + Vec2::new(0.5, 3.0), rect.max - Vec2::new(0.5, 1.0));
    let tab = egui::Rect::from_min_size(rect.min + Vec2::new(0.5, 1.0), Vec2::new(5.5, 3.0));
    p.rect_filled(tab, CornerRadius::same(1), c);
    p.rect_stroke(body, CornerRadius::same(2), Stroke::new(1.3, c), egui::StrokeKind::Inside);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Open the folder in Finder with it selected (like Tauri's revealItemInDir).
fn reveal_in_finder(path: &str) {
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
}

/// Project colour picker: palette swatches, a custom colour, reset. Returns true if changed.
fn color_menu(ui: &mut egui::Ui, project: &str) -> bool {
    let mut changed = false;
    ui.label(RichText::new(format!("{project} colour")).strong());
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(170.0);
        for c in theme::PROJECT_PALETTE {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
            let selected = theme::project_color(project) == c;
            ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(4), c);
            if selected {
                ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0, theme::t().text), egui::StrokeKind::Inside);
            }
            if resp.clicked() {
                theme::set_project_color(project, Some(c));
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        let mut c = theme::project_color(project);
        ui.label("Custom");
        if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
            theme::set_project_color(project, Some(c));
            changed = true;
        }
        if theme::has_custom_color(project) && ui.small_button("Reset").clicked() {
            theme::set_project_color(project, None);
            changed = true;
        }
    });
    changed
}

fn dot(ui: &mut egui::Ui, c: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, c);
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).color(theme::t().muted).size(11.0).strong());
    ui.add_space(4.0);
}

fn stat(ui: &mut egui::Ui, k: &str, v: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(k).color(theme::t().muted).size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(v).size(12.0));
        });
    });
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}

fn fmt_dur(ms: i64) -> String {
    let m = ms / 60_000;
    if m < 1 {
        "<1m".into()
    } else if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {}m", m / 60, m % 60)
    }
}

fn fmt_kb(kb: i64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", kb / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_dur(30_000), "<1m");
        assert_eq!(fmt_dur(125 * 60_000), "2h 5m");
        assert_eq!(fmt_kb(2048), "2 MB");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }
}
