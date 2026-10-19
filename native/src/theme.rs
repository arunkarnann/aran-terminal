//! Themes (same palettes as the Tauri build's theme JSON files), switchable at runtime.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use alacritty_terminal::vte::ansi::NamedColor;
use eframe::egui::{self, Color32, Stroke};

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub primary: Color32,
    pub running: Color32,
    pub idle: Color32,
    pub waiting: Color32,
    pub error: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    /// black red green yellow blue magenta cyan white, then the bright set.
    pub ansi: [Color32; 16],
}

pub static THEMES: [Theme; 3] = [
    Theme {
        id: "dracula",
        name: "Dracula",
        dark: true,
        bg: hex(0x282a36),
        panel: hex(0x21222c),
        border: hex(0x44475a),
        text: hex(0xf8f8f2),
        muted: hex(0x6272a4),
        primary: hex(0xbd93f9),
        running: hex(0x50fa7b),
        idle: hex(0x6272a4),
        waiting: hex(0xffb86c),
        error: hex(0xff5555),
        cursor: hex(0xf8f8f2),
        selection: hex(0x44475a),
        ansi: [
            hex(0x21222c), hex(0xff5555), hex(0x50fa7b), hex(0xf1fa8c),
            hex(0xbd93f9), hex(0xff79c6), hex(0x8be9fd), hex(0xf8f8f2),
            hex(0x6272a4), hex(0xff6e6e), hex(0x69ff94), hex(0xffffa5),
            hex(0xd6acff), hex(0xff92df), hex(0xa4ffff), hex(0xffffff),
        ],
    },
    Theme {
        id: "tokyo-night-storm",
        name: "Tokyo Night Storm",
        dark: true,
        bg: hex(0x24283b),
        panel: hex(0x1a1b26),
        border: hex(0x3b4261),
        text: hex(0xc0caf5),
        muted: hex(0x565f89),
        primary: hex(0x7aa2f7),
        running: hex(0x9ece6a),
        idle: hex(0x565f89),
        waiting: hex(0xe0af68),
        error: hex(0xf7768e),
        cursor: hex(0xc0caf5),
        selection: hex(0x3b4261),
        ansi: [
            hex(0x1a1b26), hex(0xf7768e), hex(0x9ece6a), hex(0xe0af68),
            hex(0x7aa2f7), hex(0xbb9af7), hex(0x7dcfff), hex(0xa9b1d6),
            hex(0x565f89), hex(0xf99aaa), hex(0xb0da7a), hex(0xe8bc7a),
            hex(0x89b4fa), hex(0xc8a8fa), hex(0x8ad8ff), hex(0xc0caf5),
        ],
    },
    Theme {
        id: "github-light",
        name: "GitHub Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xf6f8fa),
        border: hex(0xd0d7de),
        text: hex(0x1f2328),
        muted: hex(0x656d76),
        primary: hex(0x0969da),
        running: hex(0x1a7f37),
        idle: hex(0x8b949e),
        waiting: hex(0x9a6700),
        error: hex(0xcf222e),
        cursor: hex(0x1f2328),
        selection: hex(0xddf4ff),
        ansi: [
            hex(0x24292f), hex(0xcf222e), hex(0x1a7f37), hex(0x9a6700),
            hex(0x0969da), hex(0x8250df), hex(0x1b7c83), hex(0x6e7781),
            hex(0x656d76), hex(0xe44b55), hex(0x26a148), hex(0xb08800),
            hex(0x218bff), hex(0xa475e5), hex(0x31929a), hex(0x8c959f),
        ],
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The active theme.
pub fn t() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed) % THEMES.len()]
}

pub fn current_index() -> usize {
    CURRENT.load(Ordering::Relaxed) % THEMES.len()
}

pub fn index_of(id: &str) -> Option<usize> {
    THEMES.iter().position(|th| th.id == id)
}

/// Switch theme and restyle egui's widgets to match.
pub fn apply(ctx: &egui::Context, idx: usize) {
    CURRENT.store(idx % THEMES.len(), Ordering::Relaxed);
    let th = t();
    let mut v = if th.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = th.panel;
    v.window_fill = th.panel;
    v.extreme_bg_color = th.bg;
    v.faint_bg_color = th.bg;
    v.override_text_color = Some(th.text);
    v.selection.bg_fill = th.primary.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, th.primary);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, th.border);
    v.window_stroke = Stroke::new(1.0, th.border);
    ctx.set_visuals(v);
    ctx.request_repaint();
}

/// xterm 256-colour palette: 16 theme colours, 6x6x6 cube, 24 greys.
pub fn indexed(i: u8) -> Color32 {
    match i {
        0..=15 => t().ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(step(i / 36), step((i / 6) % 6), step(i % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

pub fn named(n: NamedColor) -> Color32 {
    let th = t();
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground => th.text,
        NamedColor::DimForeground => th.text.gamma_multiply(0.6),
        NamedColor::Background => th.bg,
        NamedColor::Cursor => th.cursor,
        n if (n as usize) < 16 => th.ansi[n as usize],
        // DimBlack..DimWhite follow the 16 bright entries in alacritty's enum.
        n => {
            let idx = (n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8;
            th.ansi[idx].gamma_multiply(0.6)
        }
    }
}

/// Same project palette + hash as `projectColors.ts`, so a folder keeps its colour
/// across both builds.
pub const PROJECT_PALETTE: [Color32; 12] = [
    hex(0x7aa2f7), hex(0xbb9af7), hex(0x7dcfff), hex(0x9ece6a),
    hex(0xe0af68), hex(0xf7768e), hex(0xff9e64), hex(0x2ac3de),
    hex(0x73daca), hex(0xff75a0), hex(0xc0caf5), hex(0xb4f9f8),
];

/// User-picked colours per project (persisted by the app in the settings table).
static OVERRIDES: Mutex<Option<HashMap<String, Color32>>> = Mutex::new(None);

fn overrides() -> std::sync::MutexGuard<'static, Option<HashMap<String, Color32>>> {
    OVERRIDES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Load overrides from "name=#rrggbb\n…" (names may contain anything but newline).
pub fn load_project_colors(s: &str) {
    let map = s
        .lines()
        .filter_map(|l| {
            let (name, hexs) = l.rsplit_once('=')?;
            let v = u32::from_str_radix(hexs.trim_start_matches('#'), 16).ok()?;
            Some((name.to_string(), hex(v)))
        })
        .collect();
    *overrides() = Some(map);
}

pub fn project_colors_string() -> String {
    let g = overrides();
    let mut v: Vec<String> = g
        .iter()
        .flatten()
        .map(|(k, c)| format!("{k}=#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
        .collect();
    v.sort();
    v.join("\n")
}

/// Set (Some) or reset to the default (None) a project's colour.
pub fn set_project_color(name: &str, color: Option<Color32>) {
    let mut g = overrides();
    let map = g.get_or_insert_with(HashMap::new);
    match color {
        Some(c) => map.insert(name.to_string(), c),
        None => map.remove(name),
    };
}

pub fn has_custom_color(name: &str) -> bool {
    overrides().as_ref().is_some_and(|m| m.contains_key(name))
}

pub fn project_color(name: &str) -> Color32 {
    if let Some(c) = overrides().as_ref().and_then(|m| m.get(name).copied()) {
        return c;
    }
    default_project_color(name)
}

pub fn default_project_color(name: &str) -> Color32 {
    let mut h: u32 = 0;
    for unit in name.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as u32);
    }
    PROJECT_PALETTE[(h as usize) % PROJECT_PALETTE.len()]
}

/// Readable text colour on top of a solid `bg` fill (WCAG luminance, like `inkOn`).
pub fn ink_on(bg: Color32) -> Color32 {
    let lin = |c: u8| {
        let v = c as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * lin(bg.r()) + 0.7152 * lin(bg.g()) + 0.0722 * lin(bg.b());
    if l > 0.179 { hex(0x14151c) } else { Color32::WHITE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_colour_matches_ts_hash() {
        // "conductor": TS (h*31 + charCode) >>> 0, mod 12.
        let mut h: u32 = 0;
        for c in "conductor".chars() {
            h = h.wrapping_mul(31).wrapping_add(c as u32);
        }
        assert_eq!(default_project_color("conductor"), PROJECT_PALETTE[h as usize % 12]);
        load_project_colors("my=proj=#ff0000\nbad");
        assert_eq!(project_color("my=proj"), hex(0xff0000));
        assert_eq!(project_colors_string(), "my=proj=#ff0000");
        set_project_color("my=proj", None);
        assert!(!has_custom_color("my=proj"));
        assert_eq!(ink_on(Color32::WHITE), hex(0x14151c));
    }
}
