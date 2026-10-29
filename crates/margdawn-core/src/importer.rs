//! Theme importer.
//!
//! Supports two source formats, auto-detected:
//! 1. VS Code color themes (`theme-name-color-theme.json` files with the
//!    standard `colors` + `tokenColors` schema — the de-facto standard used
//!    by many editors/tools).
//! 2. margdawn's own native theme JSON format (round-trip friendly).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::color::Color;
use crate::theme::{Colors, Shade, Theme};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read file: {0}")]
    Read(#[from] std::io::Error),
    #[error("not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Deserialize)]
struct VsCodeTheme {
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    token_colors: Vec<VsToken>,
    #[serde(rename = "tokenColors", default)]
    token_colors2: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct VsToken {
    #[serde(default)]
    scope: Option<serde_json::Value>,
    settings: VsSettings,
}

#[derive(Debug, Deserialize)]
struct VsSettings {
    #[serde(default, rename = "foreground")]
    foreground: Option<String>,
    #[serde(default, rename = "fontStyle")]
    font_style: Option<String>,
}

impl VsCodeTheme {
    fn tokens(&mut self) -> &mut Vec<VsToken> {
        // Normalize tokenColors given as a single object or array.
        if self.token_colors.is_empty() {
            if let Some(arr) = self.token_colors2.as_array() {
                for v in arr {
                    if let Ok(t) = serde_json::from_value::<VsToken>(v.clone()) {
                        self.token_colors.push(t);
                    }
                }
            } else if self.token_colors2.is_object() {
                if let Ok(t) = serde_json::from_value::<VsToken>(self.token_colors2.clone()) {
                    self.token_colors.push(t);
                }
            }
        }
        &mut self.token_colors
    }

    fn find_color(&mut self, needles: &[&str]) -> Option<Color> {
        for t in self.tokens().iter_mut() {
            let scopes = match t.scope.take() {
                Some(serde_json::Value::String(s)) => vec![s],
                Some(serde_json::Value::Array(a)) => {
                    a.into_iter().filter_map(|v| v.as_str().map(String::from)).collect()
                }
                _ => vec![],
            };
            // restore consumed scope values back for reuse
            t.scope = Some(serde_json::Value::Array(
                scopes.iter().cloned().map(serde_json::Value::String).collect(),
            ));
            let matches = scopes.iter().any(|s| {
                needles.iter().any(|n| s == n || s.starts_with(n) || s.ends_with(n) || s.contains(n))
            });
            if matches {
                if let Some(fg) = t.settings.foreground.as_deref().and_then(Color::parse) {
                    return Some(fg);
                }
            }
        }
        None
    }

    fn font_color(&mut self, style: &str) -> Option<Color> {
        for t in self.tokens().iter_mut() {
            if t.settings
                .font_style
                .as_deref()
                .is_some_and(|f| f.to_lowercase().contains(style))
            {
                if let Some(fg) = t.settings.foreground.as_deref().and_then(Color::parse) {
                    return Some(fg);
                }
            }
        }
        None
    }

    fn color_of(&self, keys: &[&str]) -> Option<Color> {
        keys.iter()
            .find_map(|k| self.colors.get(*k).and_then(|v| Color::parse(v)))
    }

    fn to_theme(&mut self) -> Result<Theme, ImportError> {
        let background = self
            .color_of(&["editor.background"])
            .ok_or_else(|| ImportError::Invalid("missing `colors.editor.background` — is this a VS Code theme?".into()))?;
        let foreground = self
            .color_of(&["editor.foreground"])
            .unwrap_or_else(|| if background.is_dark() { Color::new(255, 255, 255) } else { Color::new(0, 0, 0) });

        let heading = self
            .find_color(&["entity.name.section", "markup.heading"])
            .or_else(|| self.font_color("bold"));
        let link = self.find_color(&["markup.underline.link", "constant.other.reference-link", "string.other.link", "markup.underline"]);
        let code = self.find_color(&["string.quote", "string", "constant", "support.function", "markup.inline.raw"]).or_else(|| self.color_of(&["text.code.foreground"]));
        let emphasis = self.font_color("italic");
        let strong = self.font_color("bold");
        let strike = self
            .font_color("strikethrough")
            .or_else(|| self.find_color(&["comment.line", "comment"]));
        let quote = self.find_color(&["comment"]);
        let list_marker = self.find_color(&["punctuation.definition.list_item", "markup.list", "entity.name.tag"]).or_else(|| self.find_color(&["string"]));
        let muted = self
            .find_color(&["comment"])
            .unwrap_or_else(|| foreground.blend(background, 0.45));

        let mut colors = Colors {
            background: Some(background),
            foreground: Some(foreground),
            muted: Some(muted),
            heading,
            link,
            code,
            code_background: None,
            emphasis,
            strong,
            strikethrough: strike,
            quote,
            quote_bar: heading.is_some().then_some(heading).flatten(),
            list_marker,
            hr: Some(muted),
            table_border: Some(muted),
        };
        colors.code_background = Some(if background.is_dark() {
            background.lighten(0.06)
        } else {
            background.darken(0.04)
        });

        Ok(Theme::new("", Shade::from_background(background), colors))
    }
}

/// Detect and parse a theme file (VS Code or native).
pub fn parse_theme_file(path: &Path) -> Result<Theme, ImportError> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&text)?;

    let is_vscode = value.get("tokenColors").is_some()
        || value
            .get("colors")
            .and_then(|c| c.as_object())
            .is_some_and(|map| map.iter().any(|(k, _)| k.contains('.')));
    if is_vscode {
        let name = value.get("name").and_then(|n| n.as_str()).map(String::from);
        let mut vscode: VsCodeTheme = serde_json::from_value(value)?;
        let mut theme = vscode.to_theme()?;
        if let Some(name) = name {
            theme.name = Some(name.to_string());
        }
        Ok(theme)
    } else {
        let theme: Theme = serde_json::from_value(value)?;
        if theme.colors.background.is_none() && theme.colors.foreground.is_none() {
            return Err(ImportError::Invalid(
                "theme has neither background nor foreground; unknown format".into(),
            ));
        }
        Ok(theme)
    }
}

/// Import a theme file into `<config>/themes/<name>/theme.json`.
/// Returns the directory it was written to.
pub fn import(path: &Path, name: &str) -> Result<PathBuf, ImportError> {
    let theme = parse_theme_file(path)?;
    let dir = write_theme(&theme, name)?;
    Ok(dir)
}

/// Write a theme object into `<config>/themes/<name>/theme.json`.
pub fn write_theme(theme: &Theme, name: &str) -> Result<PathBuf, ImportError> {
    if name.is_empty() {
        return Err(ImportError::Invalid("empty theme name".into()));
    }
    let dir = crate::config::theme_dir_for(name);
    std::fs::create_dir_all(&dir)?;
    let mut t = theme.clone();
    t.name = Some(name.to_string());
    let json = serde_json::to_string_pretty(&t)?;
    std::fs::write(dir.join("theme.json"), json)?;
    Ok(dir)
}

/// Alias used by the GUI theme editor.
pub fn save_theme(theme: &Theme, name: &str) -> Result<PathBuf, ImportError> {
    write_theme(theme, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpfile(content: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "margdawn-test-{}-{}.json",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    #[test]
    fn imports_vscode_theme() {
        let json = r##"{
            "name": "Test Dark",
            "type": "dark",
            "colors": { "editor.background": "#1e1e1e", "editor.foreground": "#d4d4d4" },
            "tokenColors": [
                { "scope": "comment", "settings": { "foreground": "#6a9955", "fontStyle": "italic" } },
                { "scope": "string", "settings": { "foreground": "#ce9178" } },
                { "scope": "entity.name.section.markdown", "settings": { "foreground": "#569cd6" } },
                { "scope": "markup.underline.link", "settings": { "foreground": "#4ec9b0" } },
                { "scope": ["emphasis", "markup.italic"], "settings": { "fontStyle": "italic", "foreground": "#679558" } },
                { "scope": ["strong", "markup.bold"], "settings": { "fontStyle": "bold", "foreground": "#c586c0" } }
            ]
        }"##;
        let p = tmpfile(json);
        let theme = parse_theme_file(&p).expect("imports");
        std::fs::remove_file(&p).ok();
        assert_eq!(theme.colors.background, Some(Color::new(0x1e, 0x1e, 0x1e)));
        assert_eq!(theme.colors.code, Some(Color::new(0xce, 0x91, 0x78)));
        assert_eq!(theme.colors.heading, Some(Color::new(0x56, 0x9c, 0xd6)));
        assert_eq!(theme.colors.link, Some(Color::new(0x4e, 0xc9, 0xb0)));
        assert_eq!(theme.colors.quote, Some(Color::new(0x6a, 0x99, 0x55)));
        assert!(theme.colors.emphasis.is_some());
        assert_eq!(theme.colors.emphasis, theme.colors.quote); // italic comment scopes
        assert_eq!(theme.colors.strong, Some(Color::new(0xc5, 0x86, 0xc0)));
    }

    #[test]
    fn imports_native_theme() {
        let json = r##"{ "name": "t", "appearance": "light", "colors": { "background": "#fff", "foreground": "#000" } }"##;
        let p = tmpfile(json);
        let t = parse_theme_file(&p);
        std::fs::remove_file(&p).ok();
        assert!(t.is_ok());
    }

    #[test]
    fn rejects_non_theme() {
        let p = tmpfile(r##"{ "hello": 1 }"##);
        let t = parse_theme_file(&p);
        std::fs::remove_file(&p).ok();
        assert!(t.is_err());
    }
}
