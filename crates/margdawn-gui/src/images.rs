//! Local image loading/caching for the viewer.
//!
//! Markdown image sources are resolved relative to the document (or used as
//! absolute paths). Remote URLs are not fetched — the viewer stays offline.

use eframe::egui;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type Entry = (Option<SystemTime>, Option<(egui::TextureHandle, egui::Vec2)>);

#[derive(Default)]
pub struct ImageStore {
    cache: RefCell<HashMap<PathBuf, Entry>>,
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Texture + pixel size for `src`, resolving relative paths against
    /// `base_dir`. Returns `None` for unsupported/remote/unreadable sources.
    pub fn texture(
        &self,
        ctx: &egui::Context,
        base_dir: Option<&Path>,
        src: &str,
    ) -> Option<(egui::TextureHandle, egui::Vec2)> {
        let path = resolve(base_dir, src)?;
        let modified = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());

        if let Some((cached_modified, cached)) = self.cache.borrow().get(&path) {
            if *cached_modified == modified {
                return cached.clone();
            }
        }

        let loaded = load(ctx, &path);
        self.cache
            .borrow_mut()
            .insert(path, (modified, loaded.clone()));
        loaded
    }
}

/// Turn a markdown source into a local path (remote/data URLs are unsupported).
fn resolve(base_dir: Option<&Path>, src: &str) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty()
        || src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("data:")
        || src.starts_with("//")
    {
        return None;
    }
    let raw = src.strip_prefix("file://").unwrap_or(src);
    // strip an optional query/fragment (e.g. "pic.png?raw=1")
    let raw = raw.split(['?', '#']).next().unwrap_or(raw);
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Some(path)
    } else {
        base_dir.map(|b| b.join(path))
    }
}

fn load(ctx: &egui::Context, path: &Path) -> Option<(egui::TextureHandle, egui::Vec2)> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let mut rgba = image.to_rgba8();
    let (original_w, original_h) = rgba.dimensions();

    // very large photos shouldn't blow the GPU texture limit; keep the
    // original dimensions for layout so the image still displays at its
    // intended size
    const MAX_DIM: u32 = 2048;
    if original_w.max(original_h) > MAX_DIM {
        let scale = MAX_DIM as f32 / original_w.max(original_h) as f32;
        let w = ((original_w as f32 * scale).round() as u32).max(1);
        let h = ((original_h as f32 * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle);
    }

    let (w, h) = rgba.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
    let texture = ctx.load_texture(
        format!("image:{}", path.display()),
        color,
        egui::TextureOptions::LINEAR,
    );
    Some((texture, egui::vec2(original_w as f32, original_h as f32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_absolute_paths() {
        let base = Path::new("/docs");
        assert_eq!(
            resolve(Some(base), "img/pic.png"),
            Some(PathBuf::from("/docs/img/pic.png"))
        );
        assert_eq!(resolve(Some(base), "/tmp/x.png"), Some(PathBuf::from("/tmp/x.png")));
        assert_eq!(
            resolve(Some(base), "file:///tmp/y.png?a=1"),
            Some(PathBuf::from("/tmp/y.png"))
        );
    }

    #[test]
    fn rejects_remote_and_empty_sources() {
        let base = Path::new("/docs");
        assert_eq!(resolve(Some(base), "https://example.com/a.png"), None);
        assert_eq!(resolve(Some(base), "data:image/png;base64,AAAA"), None);
        assert_eq!(resolve(Some(base), ""), None);
        // relative path without a document directory is unresolvable
        assert_eq!(resolve(None, "pic.png"), None);
    }
}
