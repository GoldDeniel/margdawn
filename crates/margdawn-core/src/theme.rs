//! Theme definition, loading, and resolution.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

pub use crate::color::Color;

/// Which palette slot a span uses. Renderers resolve these against a theme,
/// falling back to a sensible default when a slot is missing/invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColorKey {
    #[default]
    Foreground,
    Muted,
    Heading,
    Link,
    Code,
    Emphasis,
    Strong,
    Strike,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appearance;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shade {
    Dark,
    Light,
}

impl Shade {
    pub fn from_background(bg: Color) -> Self {
        if bg.is_dark() {
            Shade::Dark
        } else {
            Shade::Light
        }
    }
}

impl fmt::Display for Shade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Shade::Dark => write!(f, "dark"),
            Shade::Light => write!(f, "light"),
        }
    }
}

/// Every color slot a theme may define. Optional slots fall back at render
/// time, so partial themes and imported VS Code themes both work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Colors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_background: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emphasis: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_bar: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_marker: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_border: Option<Color>,
}

impl Colors {
    /// Resolve a key with a fallback chain, never panicking.
    pub fn resolve(&self, key: ColorKey, base: ColorKey) -> Color {
        let fg = self.foreground.unwrap_or(Color::new(0, 0, 0));
        let bg = self.background.unwrap_or(Color::new(255, 255, 255));
        let muted = self.muted.unwrap_or_else(|| fg.blend(bg, 0.45));

        let slot = |c: Option<Color>, fb: Color| c.unwrap_or(fb);
        match key {
            ColorKey::Foreground => fg,
            ColorKey::Muted => muted,
            ColorKey::Heading => self.heading.unwrap_or(fg),
            ColorKey::Link => self.link.unwrap_or(fg),
            ColorKey::Code => self.code.unwrap_or(fg),
            ColorKey::Emphasis => slot(self.emphasis, self.resolve(base, ColorKey::Foreground)),
            ColorKey::Strong => slot(self.strong, self.resolve(base, ColorKey::Foreground)),
            ColorKey::Strike => self.strikethrough.unwrap_or(muted),
        }
    }

    pub fn background(&self) -> Color {
        self.background.unwrap_or(Color::new(255, 255, 255))
    }

    pub fn foreground(&self) -> Color {
        self.foreground.unwrap_or(Color::new(0, 0, 0))
    }

    /// A subtle backdrop for code blocks: nudged toward the foreground so it
    /// reads as a panel on any theme.
    pub fn code_background(&self) -> Color {
        if let Some(c) = self.code_background {
            return c;
        }
        let (bg, _fg) = (self.background(), self.foreground());
        if bg.is_dark() {
            bg.lighten(0.06)
        } else {
            bg.darken(0.04)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    /// Display / lookup name. When absent, the directory name is used.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub appearance: Option<Shade>,
    /// Preferred body typeface: `"sans"` or `"serif"` (config can override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_body: Option<String>,
    /// Preferred heading typeface: `"sans"` or `"serif"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_heading: Option<String>,
    #[serde(default)]
    pub colors: Colors,
}

impl Theme {
    pub fn new(name: &str, shade: Shade, colors: Colors) -> Self {
        Self {
            name: Some(name.to_string()),
            appearance: Some(shade),
            font_body: None,
            font_heading: None,
            colors,
        }
    }

    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("unnamed")
    }

    pub fn shade(&self) -> Shade {
        self.appearance.unwrap_or_else(|| Shade::from_background(self.colors.background()))
    }
}

/// Errors while loading theme files.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("theme file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("theme `{0}` not found")]
    NotFound(String),
}

pub const DEFAULT_THEME: &str = "claude";
pub const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("claude", include_str!("../assets/themes/claude.json")),
    ("claude-light", include_str!("../assets/themes/claude-light.json")),
    ("monokai-night", include_str!("../assets/themes/monokai-night.json")),
];

/// Load a built-in theme by name.
pub fn builtin(name: &str) -> Option<Theme> {
    BUILTIN_THEMES
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, json)| serde_json::from_str(json).ok())
}

pub fn builtin_names() -> Vec<&'static str> {
    BUILTIN_THEMES.iter().map(|(n, _)| *n).collect()
}

/// Where user themes live: `<config>/margdawn/themes/<name>/theme.json`
/// (a flat `<name>.json` file is also accepted).
pub fn theme_search_paths(config_dir: &std::path::Path, name: &str) -> Vec<PathBuf> {
    vec![
        config_dir.join("themes").join(name).join("theme.json"),
        config_dir.join("themes").join(format!("{name}.json")),
        config_dir.join("themes").join(name).join("theme.jsonc"),
    ]
}

/// Load a theme by name: user themes first, built-ins as fallback.
pub fn load(config_dir: &std::path::Path, name: &str) -> Result<Theme, ThemeError> {
    for path in theme_search_paths(config_dir, name) {
        if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                ThemeError::NotFound(format!("{name}: cannot read {}: {e}", path.display()))
            })?;
            let theme: Theme = serde_json::from_str(&text)?;
            let theme = theme.with_name(name);
            return Ok(theme);
        }
    }
    builtin(name).ok_or_else(|| ThemeError::NotFound(name.to_string()))
}

impl Theme {
    /// Fill in the canonical name (directory/lookup name wins over key).
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    /// A theme without background/foreground would render invisibly; inherit
    /// those two slots from `fallback`. Returns true when something was filled.
    pub fn inherit_essentials_from(&mut self, fallback: &Theme) -> bool {
        let mut filled = false;
        if self.colors.background.is_none() {
            self.colors.background = Some(fallback.colors.background());
            filled = true;
        }
        if self.colors.foreground.is_none() {
            self.colors.foreground = Some(fallback.colors.foreground());
            filled = true;
        }
        if filled {
            self.appearance = Some(Shade::from_background(self.colors.background()));
        }
        filled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse() {
        for (n, json) in BUILTIN_THEMES {
            let t: Theme = serde_json::from_str(json).expect(n);
            assert!(t.colors.background.is_some(), "{n} needs background");
            assert!(t.colors.foreground.is_some(), "{n} needs foreground");
        }
    }

    #[test]
    fn resolution_never_panics() {
        let empty = Colors::default();
        for k in [
            ColorKey::Foreground,
            ColorKey::Muted,
            ColorKey::Heading,
            ColorKey::Code,
            ColorKey::Strong,
        ] {
            let _ = empty.resolve(k, ColorKey::Foreground);
        }
    }

    #[test]
    fn inherits_missing_essentials() {
        let mut broken: Theme = serde_json::from_str(
            r##"{ "name": "broken", "appearance": "light", "colors": { "link": "#66D9EF" } }"##,
        )
        .unwrap();
        let fallback = builtin("claude").unwrap();
        assert!(broken.inherit_essentials_from(&fallback));
        assert_eq!(broken.colors.background, fallback.colors.background);
        assert_eq!(broken.colors.foreground, fallback.colors.foreground);
        assert_eq!(broken.shade(), Shade::Dark);
        // idempotent
        assert!(!broken.inherit_essentials_from(&fallback));
    }

    #[test]
    fn partial_theme_falls_back() {
        let mut c = Colors::default();
        c.background = Some(Color::new(0, 0, 0));
        c.foreground = Some(Color::new(255, 255, 255));
        let t = Theme::new("x", Shade::Dark, c);
        assert_eq!(t.shade(), Shade::Dark);
        // missing heading falls back to foreground
        assert_eq!(t.colors.resolve(ColorKey::Heading, ColorKey::Foreground), Color::new(255, 255, 255));
    }
}
