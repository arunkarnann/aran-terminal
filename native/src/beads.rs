//! Issues sidebar — a GUI over the local Beads (`bd`) tracker of the active project.
//! In progress: hidden behind `NEW_PANES` in app.rs.

use eframe::egui;

pub struct IssuesPanel {
    #[allow(dead_code)]
    ctx: egui::Context,
}

impl IssuesPanel {
    pub fn new(ctx: egui::Context) -> Self {
        IssuesPanel { ctx }
    }

    /// Open (non-closed) issue count for the header badge, once loaded.
    pub fn open_count(&self) -> Option<usize> {
        None
    }

    /// Draw the pane for the active session's folder.
    pub fn show(&mut self, ui: &mut egui::Ui, _cwd: Option<String>) {
        ui.label("Issues — coming soon");
    }
}
