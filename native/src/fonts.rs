//! Load the user's terminal font from disk. egui's bundled font has no Nerd Font
//! icons (prompt glyphs) or ⌘ symbol, so we prefer an installed Nerd Font, then
//! Menlo, and add Apple Symbols as a fallback. Only these few files are loaded, to
//! keep memory low (CJK system fonts are 10-50 MB each, so they are skipped).

use std::path::{Path, PathBuf};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(Path::new(&home).join("Library/Fonts"));
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs
}

/// First installed Nerd Font regular face, if any.
fn find_nerd_font() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = font_dirs()
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.contains("NerdFont") && n.ends_with("-Regular.ttf") && !n.contains("Propo")
        })
        .collect();
    // Prefer the "Mono" variant: its icons are one cell wide.
    found.sort_by_key(|p| !p.to_string_lossy().contains("NerdFontMono"));
    found.into_iter().next()
}

fn load(defs: &mut FontDefinitions, name: &str, path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => {
            defs.font_data.insert(name.into(), FontData::from_owned(bytes).into());
            true
        }
        Err(_) => false,
    }
}

const MENLO: &str = "/System/Library/Fonts/Menlo.ttc";

const FALLBACKS: &[(&str, &str)] = &[
    ("menlo", MENLO),
    ("apple-symbols", "/System/Library/Fonts/Apple Symbols.ttf"),
    ("stix-math", "/System/Library/Fonts/Supplemental/STIXTwoMath.otf"),
    ("zapf", "/System/Library/Fonts/ZapfDingbats.ttf"),
];

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();

    let nerd = find_nerd_font();
    let primary = nerd.clone().unwrap_or_else(|| PathBuf::from(MENLO));
    if load(&mut defs, "term", &primary) {
        if let Some(f) = defs.families.get_mut(&FontFamily::Monospace) {
            f.insert(0, "term".into());
        }
    }
    // Small symbol fonts (≈2 MB total) covering what CLI agents draw: ⏺ ⎿ ✻ ⌘ ❯ …
    for (name, path) in FALLBACKS {
        // Menlo is already the primary when no Nerd Font is installed.
        if *path == MENLO && nerd.is_none() {
            continue;
        }
        if load(&mut defs, name, Path::new(path)) {
            for fam in [FontFamily::Monospace, FontFamily::Proportional] {
                if let Some(f) = defs.families.get_mut(&fam) {
                    f.push((*name).into());
                }
            }
        }
    }
    ctx.set_fonts(defs);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, FontId};

    #[test]
    fn covers_agent_glyphs() {
        let ctx = egui::Context::default();
        super::install(&ctx);
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let font = FontId::monospace(13.0);
        let missing: Vec<char> = "⏺⎿✻✳●✔❯→⌘│─█■◐⇤"
            .chars()
            .filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font, *c)))
            .collect();
        ctx.tex_manager().write().take_delta().clear();
        assert!(missing.is_empty(), "missing glyphs: {missing:?}");
    }
}
