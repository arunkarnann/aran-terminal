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

pub static THEMES: [Theme; 8] = [
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
    Theme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        dark: true,
        bg: hex(0x1e1e2e),
        panel: hex(0x181825),
        border: hex(0x313244),
        text: hex(0xcdd6f4),
        muted: hex(0x585b70),
        primary: hex(0xcba6f7),
        running: hex(0xa6e3a1),
        idle: hex(0x6c7086),
        waiting: hex(0xf9e2af),
        error: hex(0xf38ba8),
        cursor: hex(0xf5e0dc),
        selection: hex(0x45475a),
        ansi: [
            hex(0x45475a), hex(0xf38ba8), hex(0xa6e3a1), hex(0xf9e2af),
            hex(0x89b4fa), hex(0xcba6f7), hex(0x94e2d5), hex(0xbac2de),
            hex(0x585b70), hex(0xf5a0b8), hex(0xb4e8b0), hex(0xfce8c0),
            hex(0xa0c4fe), hex(0xd4b0fb), hex(0xa0e8d8), hex(0xcdd6f4),
        ],
    },
    Theme {
        id: "github-dark",
        name: "GitHub Dark",
        dark: true,
        bg: hex(0x0d1117),
        panel: hex(0x161b22),
        border: hex(0x30363d),
        text: hex(0xe6edf3),
        muted: hex(0x8b949e),
        primary: hex(0x58a6ff),
        running: hex(0x3fb950),
        idle: hex(0x8b949e),
        waiting: hex(0xd29922),
        error: hex(0xf85149),
        cursor: hex(0xe6edf3),
        selection: hex(0x30363d),
        ansi: [
            hex(0x21262d), hex(0xf85149), hex(0x3fb950), hex(0xd29922),
            hex(0x58a6ff), hex(0xbc8cff), hex(0x39d2c0), hex(0x8b949e),
            hex(0x484f58), hex(0xfa6e64), hex(0x56d364), hex(0xe3b341),
            hex(0x79c0ff), hex(0xd2a8ff), hex(0x56d4c4), hex(0xb1bac4),
        ],
    },
    Theme {
        id: "one-dark",
        name: "One Dark",
        dark: true,
        bg: hex(0x282c34),
        panel: hex(0x21252b),
        border: hex(0x3b4048),
        text: hex(0xabb2bf),
        muted: hex(0x5c6370),
        primary: hex(0x61afef),
        running: hex(0x98c379),
        idle: hex(0x5c6370),
        waiting: hex(0xe5c07b),
        error: hex(0xe06c75),
        cursor: hex(0xabb2bf),
        selection: hex(0x3b4048),
        ansi: [
            hex(0x282c34), hex(0xe06c75), hex(0x98c379), hex(0xe5c07b),
            hex(0x61afef), hex(0xc678dd), hex(0x56b6c2), hex(0xabb2bf),
            hex(0x5c6370), hex(0xe9848c), hex(0xa8d88a), hex(0xefd090),
            hex(0x82baf7), hex(0xd48fe8), hex(0x6cc7d3), hex(0xc8ccd4),
        ],
    },
    Theme {
        id: "material-palenight",
        name: "Material Palenight",
        dark: true,
        bg: hex(0x292d3e),
        panel: hex(0x202331),
        border: hex(0x393f4f),
        text: hex(0xbabed8),
        muted: hex(0x676e95),
        primary: hex(0xc792ea),
        running: hex(0xc3e88d),
        idle: hex(0x676e95),
        waiting: hex(0xffcb6b),
        error: hex(0xf07178),
        cursor: hex(0xbabed8),
        selection: hex(0x393f4f),
        ansi: [
            hex(0x202331), hex(0xf07178), hex(0xc3e88d), hex(0xffcb6b),
            hex(0x82aaff), hex(0xc792ea), hex(0x89ddff), hex(0x959dcb),
            hex(0x676e95), hex(0xf48a90), hex(0xd0eea0), hex(0xffd880),
            hex(0x94baff), hex(0xd4a0f4), hex(0x94e4ff), hex(0xbabed8),
        ],
    },
    Theme {
        id: "nord",
        name: "Nord",
        dark: true,
        bg: hex(0x2e3440),
        panel: hex(0x3b4252),
        border: hex(0x434c5e),
        text: hex(0xeceff4),
        muted: hex(0x616e88),
        primary: hex(0x88c0d0),
        running: hex(0xa3be8c),
        idle: hex(0x616e88),
        waiting: hex(0xebcb8b),
        error: hex(0xbf616a),
        cursor: hex(0xeceff4),
        selection: hex(0x434c5e),
        ansi: [
            hex(0x3b4252), hex(0xbf616a), hex(0xa3be8c), hex(0xebcb8b),
            hex(0x81a1c1), hex(0xb48ead), hex(0x88c0d0), hex(0xe5e9f0),
            hex(0x4c566a), hex(0xc9828a), hex(0xb2cf9a), hex(0xf0d89a),
            hex(0x8fbcbb), hex(0xc4a3b8), hex(0x8fbcbb), hex(0xeceff4),
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

    fn json_hex(v: &serde_json::Value, key: &str) -> Color32 {
        let s = v[key].as_str().unwrap_or_else(|| panic!("missing {key}"));
        let n = u32::from_str_radix(s.trim_start_matches('#'), 16)
            .unwrap_or_else(|_| panic!("bad hex for {key}: {s}"));
        hex(n)
    }

    #[test]
    fn themes_match_tauri_json() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../conductor/src/themes");
        let mut seen = std::collections::HashSet::new();
        for th in &THEMES {
            assert!(seen.insert(th.id), "duplicate theme id {}", th.id);
            let path = dir.join(format!("{}.json", th.id));
            let raw = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("no JSON for theme {}: {e}", th.id));
            let j: serde_json::Value = serde_json::from_str(&raw).expect("valid theme JSON");
            let (c, term) = (&j["colors"], &j["terminal"]);
            assert_eq!(j["name"].as_str(), Some(th.name), "{}", th.id);
            assert_eq!(j["type"].as_str() == Some("dark"), th.dark, "{}", th.id);
            let ui = [
                ("bg", th.bg), ("panel", th.panel), ("border", th.border), ("text", th.text),
                ("muted", th.muted), ("primary", th.primary), ("running", th.running),
                ("idle", th.idle), ("waiting", th.waiting), ("error", th.error),
            ];
            for (k, v) in ui {
                assert_eq!(json_hex(c, k), v, "{} colors.{k}", th.id);
            }
            assert_eq!(json_hex(term, "cursor"), th.cursor, "{} cursor", th.id);
            assert_eq!(json_hex(term, "selectionBackground"), th.selection, "{} selection", th.id);
            let names = [
                "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
                "brightBlack", "brightRed", "brightGreen", "brightYellow",
                "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
            ];
            for (k, v) in names.iter().zip(th.ansi) {
                assert_eq!(json_hex(term, k), v, "{} terminal.{k}", th.id);
            }
        }
    }
}
