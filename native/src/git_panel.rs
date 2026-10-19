//! Git sidebar — status, staging, commit, branches and log for the active folder.
//! In progress: hidden behind `NEW_PANES` in app.rs.

use eframe::egui;

pub struct GitPanel {
    #[allow(dead_code)]
    ctx: egui::Context,
}

impl GitPanel {
    pub fn new(ctx: egui::Context) -> Self {
        GitPanel { ctx }
    }

    /// Draw the pane for the active session's folder.
    pub fn show(&mut self, ui: &mut egui::Ui, _cwd: Option<String>) {
        ui.label("Git — coming soon");
    }
}
