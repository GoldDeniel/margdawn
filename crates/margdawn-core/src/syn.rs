//! Syntax-highlighted code blocks (syntect) for both renderers.
//!
//! Ships a hand-built "monokai" theme plus syntect's defaults, loads user
//! `*.tmTheme` files from `<config>/margdawn/themes/code/`, and caches
//! highlight results so the immediate-mode GUI stays cheap.

use std::collections::HashMap;
use std::str::FromStr;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SColor, FontStyle, StyleModifier, Theme, ThemeItem, ThemeSet, ThemeSettings,
};
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::color::Color;
use crate::config;

/// A highlighted code block: one line = many colored runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlighted {
    /// Theme background color.
    pub background: Color,
    /// Default foreground for uninterpreted text (comments etc. fallback).
    pub foreground: Color,
    /// Lines of (color, text) runs.
    pub lines: Vec<Vec<(Color, String)>>,
}

struct Globals {
    pub syntax_set: SyntaxSet,
    pub theme_set: ThemeSet,
}

fn global() -> &'static Globals {
    static GLOBALS: OnceLock<Globals> = OnceLock::new();
    GLOBALS.get_or_init(build_globals)
}

struct Cache {
    map: HashMap<String, Highlighted>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache { map: HashMap::new() }))
}

pub fn builtin_code_theme_names() -> Vec<String> {
    ["monokai", "InspiredGitHub", "Solarized (dark)", "Solarized (light)"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Names from user-imported `*.tmTheme` files.
pub fn user_code_theme_names() -> Vec<String> {
    let dir = config::code_themes_dir();
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "tmTheme") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

pub fn code_theme_names() -> Vec<String> {
    let mut names = builtin_code_theme_names();
    names.extend(user_code_theme_names());
    names
}

/// Build the theme set: syntect defaults + our monokai + user themes.
fn build_globals() -> Globals {
    let syntax_set = SyntaxSet::load_defaults_newlines();

    let mut theme_set = ThemeSet::load_defaults();
    theme_set.themes.insert("monokai".to_string(), build_monokai());

    let user_dir = config::code_themes_dir();
    if user_dir.is_dir() {
        theme_set.add_from_folder(&user_dir).ok();
    }

    Globals { syntax_set, theme_set }
}

/// A hand-rolled Monokai for scope-based highlighting.
fn build_monokai() -> Theme {
    let sc = |r: u8, g: u8, b: u8| SColor { r, g, b, a: 0xFF };
    let fg = (0xF8, 0xF8, 0xF2);

    let item = |scope: &str, (r, g, b): (u8, u8, u8), italic: bool, bold: bool| ThemeItem {
        scope: ScopeSelectors::from_str(scope).unwrap(),
        style: StyleModifier {
            foreground: Some(sc(r, g, b)),
            background: None,
            font_style: Some(if italic && bold { FontStyle::BOLD | FontStyle::ITALIC } else if italic { FontStyle::ITALIC } else if bold { FontStyle::BOLD } else { FontStyle::empty() }),
        },
    };

    Theme {
        name: Some("monokai".to_string()),
        author: Some("margdawn".to_string()),
        settings: ThemeSettings {
            background: Some(sc(0x27, 0x28, 0x22)),
            foreground: Some(sc(fg.0, fg.1, fg.2)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item("comment", (0x75, 0x71, 0x5E), true, false),
            item("keyword", (0xF9, 0x26, 0x72), false, false),
            item("string", (0xE6, 0xDB, 0x74), false, false),
            item("constant", (0xAE, 0x81, 0xFF), false, false),
            item("entity.name.function", (0xA6, 0xE2, 0x2E), false, false),
            item("support.function", (0x66, 0xD9, 0xEF), false, false),
            item("entity.name.tag", (0xF9, 0x26, 0x72), false, false),
            item("entity.name", (0xA6, 0xE2, 0x2E), false, false),
            item("entity.other", (0x66, 0xD9, 0xEF), false, false),
            item("variable.language", (0xFD, 0x97, 0x1F), false, true),
            item("variable.other", (0xFD, 0x97, 0x1F), false, false),
            item("support.type", (0x66, 0xD9, 0xEF), false, false),
            item("storage", (0x66, 0xD9, 0xEF), true, false),
            item("markup.heading", (0xA6, 0xE2, 0x2E), false, true),
            item("markup.bold", (0xF9, 0x26, 0x72), false, true),
            item("markup.italic", (0xFD, 0x97, 0x1F), false, false),
            item("markup.underline", (0x66, 0xD9, 0xEF), false, false),
            item("punctuation", (0x9E, 0x9E, 0x93), false, false),
        ],
    }
}

/// Copy a `.tmTheme` into the code-themes dir; returns the theme name.
pub fn import_code_theme(path: &Path, name: Option<&str>) -> Result<String, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no file stem".to_string())?;
    let name = name.filter(|n| !n.is_empty()).unwrap_or(stem);
    let dir = config::code_themes_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{name}.tmTheme"));
    std::fs::copy(path, &dest).map_err(|e| e.to_string())?;
    Ok(name.to_string())
}

/// Background / foreground of a theme.
fn theme_colors(theme: &Theme) -> (Color, Color) {
    let bg = theme
        .settings
        .background
        .map(|c| Color::new(c.r, c.g, c.b))
        .unwrap_or(Color::new(0x27, 0x28, 0x22));
    let fg = theme
        .settings
        .foreground
        .map(|c| Color::new(c.r, c.g, c.b))
        .unwrap_or_else(|| if bg.is_dark() { Color::new(0xF8, 0xF8, 0xF2) } else { Color::new(0x21, 0x21, 0x21) });
    (bg, fg)
}

/// Highlight `code` (language `lang`) with named theme; cached.
pub fn highlight(lang: Option<&str>, code: &str, theme_name: &str) -> Highlighted {
    let key = format!("{}|{}|{}", lang.unwrap_or(""), theme_name, code);
    if let Some(hit) = cache().lock().unwrap().map.get(&key) {
        return hit.clone();
    }

    let theme_set = &global().theme_set;
    let syntax_set = &global().syntax_set;
    let theme = match theme_set.themes.get(theme_name) {
        Some(t) => t,
        None => theme_set.themes.get("monokai").unwrap_or_else(|| {
            theme_set.themes.values().next().expect("no themes at all")
        }),
    };
    let (background, foreground) = theme_colors(theme);

    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_extension(l))
        .or_else(|| {
            lang.and_then(|l| {
                syntax_set
                    .syntaxes()
                    .iter()
                    .find(|s| !s.hidden && s.name.to_lowercase() == l.to_lowercase())
            })
        })
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut hl = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let styles = hl.highlight_line(line, syntax_set).unwrap_or_default();
        let mut runs = Vec::new();
        for (style, text) in styles {
            if text.is_empty() || text == "\n" || text == "\r\n" {
                continue;
            }
            let text = text.trim_end_matches('\n').trim_end_matches('\r').to_string();
            if text.is_empty() {
                continue;
            }
            let color = Color::new(style.foreground.r, style.foreground.g, style.foreground.b);
            runs.push((color, text));
        }
        lines.push(runs);
    }

    let hl = Highlighted { background, foreground, lines };
    cache()
        .lock()
        .unwrap()
        .map
        .insert(key, hl.clone());
    hl
}

/// Theme color accessor for the settings UI.
pub fn code_theme_bg(name: &str) -> Option<Color> {
    global()
        .theme_set
        .themes
        .get(name)
        .map(|t| theme_colors(t).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monokai_highlighting() {
        let h = highlight(Some("rust"), "fn main() { let x = 1; }", "monokai");
        assert!(h.background.is_dark());
        assert_eq!(h.lines.len(), 1);
        // `fn`/`let` are storage keywords → cyan; `main` → function green; numeric → purple
        let runs = &h.lines[0];
        assert!(runs.iter().any(|(c, t)| t.contains("fn") && *c == Color::new(0x66, 0xD9, 0xEF)));
        assert!(runs.iter().any(|(c, t)| t.contains("main") && *c == Color::new(0xA6, 0xE2, 0x2E)));
        assert!(runs.iter().any(|(c, t)| t == "1" && *c == Color::new(0xAE, 0x81, 0xFF)));
    }

    #[test]
    fn unknown_lang_is_plain() {
        let h = highlight(Some("nonexistent-languagename"), "hello", "monokai");
        assert!(!h.lines.is_empty());
    }

    #[test]
    fn theme_names_include_builtins() {
        assert!(code_theme_names().iter().any(|n| n == "monokai"));
    }

    #[test]
    fn github_theme_exists() {
        let h = highlight(Some("js"), "var x = 1;", "InspiredGitHub");
        assert!(!h.background.is_dark());
    }
}
