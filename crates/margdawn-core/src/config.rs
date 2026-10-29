//! User configuration: `~/.config/margdawn/config.json`

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::theme::{self, Theme};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Name of the theme stored under `<config>/themes/<name>/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Syntax-highlighting theme for code blocks (e.g. "monokai").
    /// Falls back to `DEFAULT_CODE_THEME`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_theme: Option<String>,
    /// Base body font size for the GUI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    /// Body typeface override: `"sans"` (Inter) or `"serif"` (Source Serif).
    /// Unset = whatever the active theme prefers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_body: Option<String>,
    /// Heading typeface override (`"sans"` / `"serif"`); unset = theme's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_heading: Option<String>,
    /// Monospace font: `"jetbrainsmono"` (embedded) or `"hack"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_mono: Option<String>,
    /// Line height multiplier (1.0 = font default, default 1.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    /// Extra vertical spacing between blocks in points (default 8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paragraph_spacing: Option<f32>,
    /// Page margin (sides, top, bottom) in points (default 28).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f32>,
    /// Keep the author's blank lines as vertical space (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserve_blank_lines: Option<bool>,
    /// Terminal: paint the document background across the whole width.
    /// Default: false (blend with the terminal's own background).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Terminal: paint code-block panels and inline-code chips.
    /// Default: false (keep only foreground colors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_background: Option<bool>,
}

pub const DEFAULT_FONT_SIZE: f32 = 15.0;
pub const DEFAULT_LINE_HEIGHT: f32 = 1.5;
pub const DEFAULT_PARAGRAPH_SPACING: f32 = 8.0;
pub const DEFAULT_MARGIN: f32 = 28.0;
pub const DEFAULT_CODE_THEME: &str = "monokai";

impl Config {
    pub fn theme_name(&self) -> &str {
        self.theme.as_deref().unwrap_or(theme::DEFAULT_THEME)
    }

    pub fn code_theme_name(&self) -> &str {
        self.code_theme.as_deref().unwrap_or(DEFAULT_CODE_THEME)
    }

    pub fn font_size(&self) -> f32 {
        self.font_size.unwrap_or(DEFAULT_FONT_SIZE).clamp(9.0, 32.0)
    }

    /// Body typeface override, if any (`"inter"` accepted as legacy alias of sans).
    pub fn font_body_override(&self) -> Option<&str> {
        self.font_body.as_deref().map(|f| if f == "inter" { "sans" } else { f })
    }

    pub fn font_heading_override(&self) -> Option<&str> {
        self.font_heading.as_deref()
    }

    pub fn font_mono(&self) -> &str {
        self.font_mono.as_deref().unwrap_or("jetbrainsmono")
    }

    pub fn line_height(&self) -> f32 {
        self.line_height.unwrap_or(DEFAULT_LINE_HEIGHT).clamp(0.9, 2.6)
    }

    pub fn paragraph_spacing(&self) -> f32 {
        self.paragraph_spacing.unwrap_or(DEFAULT_PARAGRAPH_SPACING).clamp(0.0, 48.0)
    }

    pub fn margin(&self) -> f32 {
        self.margin.unwrap_or(DEFAULT_MARGIN).clamp(0.0, 120.0)
    }

    pub fn preserve_blank_lines(&self) -> bool {
        self.preserve_blank_lines.unwrap_or(true)
    }

    /// Terminal: paint the document background (default off).
    pub fn background_enabled(&self) -> bool {
        self.background.unwrap_or(false)
    }

    /// Terminal: paint code-block and inline-code backgrounds (default off).
    pub fn code_background_enabled(&self) -> bool {
        self.code_background.unwrap_or(false)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not write config: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not serialize config: {0}")]
    Json(#[from] serde_json::Error),
}

/// `~/.config/margdawn` (respects XDG_CONFIG_HOME on Linux,
/// platform-appropriate locations elsewhere).
pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("margdawn")
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

/// User-imported syntect themes for code blocks.
pub fn code_themes_dir() -> PathBuf {
    themes_dir().join("code")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Read config.json; tolerate any failure (missing dir, broken JSON, wrong
/// shape) by returning defaults.
pub fn load() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Persist config.json (creating the directory if needed).
pub fn save(config: &Config) -> Result<(), ConfigError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Everything a renderer needs: resolved theme + warnings about fallbacks.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub theme: Arc<Theme>,
    pub warnings: Vec<String>,
    pub config: Config,
}

/// Resolve the theme to use. Precedence:
/// `override_name` (CLI --theme) → `config.theme` → default (monokai-night).
/// Any invalid or missing theme falls back gracefully with a warning.
pub fn resolve_theme(override_name: Option<&str>) -> Resolved {
    let config = load();
    let mut warnings = Vec::new();
    let dir = config_dir();

    let (name, source) = match override_name {
        Some(n) => (n.to_string(), "argument"),
        None => match config.theme.as_deref() {
            Some(n) => (n.to_string(), "config.json"),
            None => (theme::DEFAULT_THEME.to_string(), "default"),
        },
    };

    let theme = match theme::load(&dir, &name) {
        Ok(t) => t,
        Err(e) => {
            warnings.push(format!(
                "theme `{name}` from {source} could not be used ({e}); falling back to `{}`",
                theme::DEFAULT_THEME
            ));
            theme::builtin(theme::DEFAULT_THEME).unwrap_or_else(|| Theme::new(
                theme::DEFAULT_THEME,
                theme::Shade::Dark,
                theme::Colors::default(),
            ))
        }
    };

    Resolved {
        theme: Arc::new(theme),
        warnings,
        config,
    }
}

/// Set `theme` in config.json and save. Returns warnings from validation.
pub fn set_theme(name: &str) -> Result<Vec<String>, ConfigError> {
    let mut config = load();
    config.theme = Some(name.to_string());
    save(&config)?;
    Ok(resolve_theme(Some(name)).warnings)
}

/// List user themes (directories + flat json files) and built-ins.
pub fn available_themes() -> (Vec<String>, Vec<&'static str>) {
    let mut user = Vec::new();
    let tdir = themes_dir();
    if let Ok(entries) = std::fs::read_dir(&tdir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("theme.json").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    user.push(name.to_string());
                }
            } else if path.is_file()
                && path.extension().is_some_and(|e| e == "json")
                && !path.join("theme.json").exists()
            {
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    user.push(name.to_string());
                }
            }
        }
    }
    user.sort();
    user.dedup();
    (user, theme::builtin_names())
}

pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(themes_dir())
}

/// Path helper for import errors/messages.
pub fn theme_dir_for(name: &str) -> PathBuf {
    themes_dir().join(name)
}

pub fn is_valid_theme_file(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .map(|t| serde_json::from_str::<Theme>(&t).is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_load_never_fails() {
        // Should not panic even if HOME is unusual.
        let _ = load();
    }

    #[test]
    fn background_toggles_parse_and_default_off() {
        let c: Config =
            serde_json::from_str(r#"{"background": true, "code_background": true}"#).unwrap();
        assert!(c.background_enabled());
        assert!(c.code_background_enabled());
        let default = Config::default();
        assert!(!default.background_enabled());
        assert!(!default.code_background_enabled());
    }

    #[test]
    fn resolve_unknown_theme_falls_back() {
        // Uses the real config dir; if the user's config names a valid theme
        // it resolves, otherwise we still always get some theme.
        let r = resolve_theme(None);
        assert!(!Arc::clone(&r.theme).name().is_empty());
    }
}
