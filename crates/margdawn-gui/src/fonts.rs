//! Font registration. Bundles Inter (sans), Source Serif 4 (serif) and
//! JetBrains Mono, each with real bold/italic cuts, as named font families.
//!
//! claude.com uses proprietary "Anthropic Sans/Serif" which cannot be
//! redistributed; Inter and Source Serif are the closest open equivalents.

use eframe::egui::{FontData, FontDefinitions, FontFamily};
use margdawn_core::config::Config;

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.otf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.otf");
const INTER_ITALIC: &[u8] = include_bytes!("../assets/fonts/Inter-Italic.otf");
const INTER_BOLDITALIC: &[u8] = include_bytes!("../assets/fonts/Inter-BoldItalic.otf");
const SERIF_REGULAR: &[u8] = include_bytes!("../assets/fonts/SourceSerif4-Regular.otf");
const SERIF_BOLD: &[u8] = include_bytes!("../assets/fonts/SourceSerif4-Bold.otf");
const SERIF_ITALIC: &[u8] = include_bytes!("../assets/fonts/SourceSerif4-It.otf");
const SERIF_BOLDITALIC: &[u8] = include_bytes!("../assets/fonts/SourceSerif4-BoldIt.otf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// Monochrome emoji shipped with egui: last-resort fallback so a stray symbol
/// never renders as a "?" box (color emoji are drawn as images, see emoji.rs).
const SYMBOL_FALLBACKS: &[&str] = &["NotoEmoji-Regular", "emoji-icon-font"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Sans,
    Serif,
}

impl Face {
    pub fn parse(s: Option<&str>) -> Option<Face> {
        match s {
            Some("serif") => Some(Face::Serif),
            Some("sans") | Some("inter") => Some(Face::Sans),
            _ => None,
        }
    }
}

pub fn family(face: Face, bold: bool, italic: bool) -> FontFamily {
    let base = match face {
        Face::Sans => "sans",
        Face::Serif => "serif",
    };
    let suffix = match (bold, italic) {
        (false, false) => "",
        (true, false) => "-b",
        (false, true) => "-i",
        (true, true) => "-bi",
    };
    FontFamily::Name(format!("{base}{suffix}").into())
}

/// (Re)install fonts; call at startup and when the mono font changes.
pub fn install(ctx: &eframe::egui::Context, cfg: &Config) {
    let mut fonts = FontDefinitions::default();

    let faces: [(&str, &'static [u8]); 9] = [
        ("inter-regular", INTER_REGULAR),
        ("inter-bold", INTER_BOLD),
        ("inter-italic", INTER_ITALIC),
        ("inter-bolditalic", INTER_BOLDITALIC),
        ("serif-regular", SERIF_REGULAR),
        ("serif-bold", SERIF_BOLD),
        ("serif-italic", SERIF_ITALIC),
        ("serif-bolditalic", SERIF_BOLDITALIC),
        ("jetbrainsmono", JETBRAINS_MONO),
    ];
    for (name, data) in faces {
        fonts.font_data.insert(name.to_string(), FontData::from_static(data));
    }

    let with_fallbacks = |primary: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = primary.iter().map(|s| s.to_string()).collect();
        v.push("inter-regular".to_string());
        v.extend(SYMBOL_FALLBACKS.iter().map(|s| s.to_string()));
        v
    };

    // egui's own UI text (buttons, labels): Inter
    fonts
        .families
        .insert(FontFamily::Proportional, with_fallbacks(&["inter-regular"]));

    // monospace: JetBrains Mono (or egui's Hack)
    let mut mono: Vec<String> = Vec::new();
    if cfg.font_mono() != "hack" {
        mono.push("jetbrainsmono".into());
    }
    mono.push("Hack".into());
    mono.extend(SYMBOL_FALLBACKS.iter().map(|s| s.to_string()));
    fonts.families.insert(FontFamily::Monospace, mono);

    for (face, regular, bold, italic, bolditalic) in [
        (Face::Sans, "inter-regular", "inter-bold", "inter-italic", "inter-bolditalic"),
        (Face::Serif, "serif-regular", "serif-bold", "serif-italic", "serif-bolditalic"),
    ] {
        fonts.families.insert(family(face, false, false), with_fallbacks(&[regular]));
        fonts.families.insert(family(face, true, false), with_fallbacks(&[bold, regular]));
        fonts.families.insert(family(face, false, true), with_fallbacks(&[italic, regular]));
        fonts
            .families
            .insert(family(face, true, true), with_fallbacks(&[bolditalic, regular]));
    }

    ctx.set_fonts(fonts);
}

pub fn mono_font_id(size: f32) -> eframe::egui::FontId {
    eframe::egui::FontId::new(size, FontFamily::Monospace)
}
