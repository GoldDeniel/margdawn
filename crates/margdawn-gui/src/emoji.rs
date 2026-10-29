//! Color emoji for egui.
//!
//! egui's text engine only rasterizes outline glyphs, so color emoji would
//! show as "?" boxes or flat monochrome shapes. Instead we detect emoji
//! clusters in the text, reserve space for them in the layout, and paint the
//! matching Noto Emoji PNG (Apache-2.0, bundled as one tar) on top.

use eframe::egui;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

static ARCHIVE: &[u8] = include_bytes!("../assets/emoji/noto-emoji-72.tar");
/// Noto's PNG set has no country flags; Twemoji (CC-BY 4.0) fills the gap.
static FLAG_ARCHIVE: &[u8] = include_bytes!("../assets/emoji/twemoji-flags-72.tar");

/// A piece of text, either plain or one emoji cluster.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Emoji(&'a str),
}

const ZWJ: char = '\u{200D}';
const VS16: char = '\u{FE0F}';
const KEYCAP: char = '\u{20E3}';

/// Characters that are emoji even without a variation selector.
fn is_default_emoji(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x1F300..=0x1FAFF
            | 0x1F004
            | 0x1F0CF
            | 0x1F170..=0x1F171
            | 0x1F17E..=0x1F17F
            | 0x1F18E
            | 0x1F191..=0x1F19A
            | 0x1F201..=0x1F202
            | 0x1F21A
            | 0x1F22F
            | 0x1F232..=0x1F23A
            | 0x1F250..=0x1F251
            | 0x231A..=0x231B
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
    )
}

fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

fn is_skin_tone(c: char) -> bool {
    matches!(c as u32, 0x1F3FB..=0x1F3FF)
}

fn is_tag(c: char) -> bool {
    matches!(c as u32, 0xE0020..=0xE007F)
}

/// Can `c` continue a ZWJ sequence?
fn is_zwj_member(c: char, next: Option<char>) -> bool {
    is_default_emoji(c)
        || next == Some(VS16)
        || matches!(c as u32, 0x2640 | 0x2642 | 0x2695 | 0x2696 | 0x2708 | 0x2764)
}

/// Split `text` into plain-text and emoji pieces.
pub fn split(text: &str) -> Vec<Piece<'_>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut plain_start = 0usize;
    let mut i = 0usize;
    let byte_at = |idx: usize| chars.get(idx).map(|(b, _)| *b).unwrap_or(text.len());

    while i < chars.len() {
        let c = chars[i].1;
        let next = chars.get(i + 1).map(|(_, c)| *c);

        // where does an emoji cluster starting at `i` end (exclusive char index)?
        let mut end: Option<usize> = None;

        if is_regional_indicator(c) {
            if next.is_some_and(is_regional_indicator) {
                end = Some(i + 2);
            }
        } else if c.is_ascii_digit() || c == '#' || c == '*' {
            // keycap: [0-9#*] FE0F? 20E3
            let mut j = i + 1;
            if chars.get(j).map(|(_, c)| *c) == Some(VS16) {
                j += 1;
            }
            if chars.get(j).map(|(_, c)| *c) == Some(KEYCAP) {
                end = Some(j + 1);
            }
        } else if is_default_emoji(c) || (next == Some(VS16) && !c.is_ascii()) {
            end = Some(i + 1);
        }

        let Some(mut j) = end else {
            i += 1;
            continue;
        };

        // extend: variation selector, skin tone, keycap, tags, ZWJ members
        loop {
            match chars.get(j).map(|(_, c)| *c) {
                Some(VS16) | Some(KEYCAP) => j += 1,
                Some(t) if is_skin_tone(t) || is_tag(t) => j += 1,
                Some(ZWJ) => {
                    let member = chars.get(j + 1).map(|(_, c)| *c);
                    let after = chars.get(j + 2).map(|(_, c)| *c);
                    match member {
                        Some(m) if is_zwj_member(m, after) => j += 2,
                        _ => break,
                    }
                }
                _ => break,
            }
        }

        let (s, e) = (byte_at(i), byte_at(j));
        if s > plain_start {
            out.push(Piece::Text(&text[plain_start..s]));
        }
        out.push(Piece::Emoji(&text[s..e]));
        plain_start = e;
        i = j;
    }

    if plain_start < text.len() {
        out.push(Piece::Text(&text[plain_start..]));
    }
    out
}

/// Lookup key variants for Noto file names: `emoji_u1f468_200d_1f469.png`.
fn keys_for(cluster: &str) -> Vec<String> {
    let hex = |skip_vs: bool| -> String {
        cluster
            .chars()
            .filter(|c| !(skip_vs && (*c == VS16 || *c == '\u{FE0E}')))
            .map(|c| format!("{:04x}", c as u32))
            .collect::<Vec<_>>()
            .join("_")
    };
    let stripped = hex(true);
    let full = hex(false);
    let mut keys = vec![stripped.clone()];
    if full != stripped {
        keys.push(full);
    }
    keys
}

/// (archive, offset, length): which tar a given key lives in.
#[derive(Clone, Copy)]
struct Entry {
    flags: bool,
    off: usize,
    len: usize,
}

pub struct EmojiStore {
    index: OnceCell<HashMap<String, Entry>>,
    textures: RefCell<HashMap<String, Option<egui::TextureHandle>>>,
}

impl Default for EmojiStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EmojiStore {
    pub fn new() -> Self {
        Self { index: OnceCell::new(), textures: RefCell::new(HashMap::new()) }
    }

    fn index(&self) -> &HashMap<String, Entry> {
        self.index.get_or_init(|| {
            let mut map = HashMap::new();
            scan_tar(ARCHIVE, false, |key, off, len| {
                map.insert(key, Entry { flags: false, off, len });
            });
            scan_tar(FLAG_ARCHIVE, true, |key, off, len| {
                map.entry(key).or_insert(Entry { flags: true, off, len });
            });
            map
        })
    }

    /// Resolve an emoji cluster to the key of an available image.
    pub fn resolve(&self, cluster: &str) -> Option<String> {
        let idx = self.index();
        for key in keys_for(cluster) {
            if idx.contains_key(&key) {
                return Some(key);
            }
            // flag sequences are named with dashes by Twemoji
            let dashed = key.replace('_', "-");
            if idx.contains_key(&dashed) {
                return Some(dashed);
            }
        }
        None
    }

    /// Texture for a resolved key (decoded lazily, cached).
    pub fn texture(&self, ctx: &egui::Context, key: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.borrow().get(key) {
            return t.clone();
        }
        let tex = self.load(ctx, key);
        self.textures.borrow_mut().insert(key.to_string(), tex.clone());
        tex
    }

    fn load(&self, ctx: &egui::Context, key: &str) -> Option<egui::TextureHandle> {
        let entry = *self.index().get(key)?;
        let archive = if entry.flags { FLAG_ARCHIVE } else { ARCHIVE };
        let bytes = archive.get(entry.off..entry.off + entry.len)?;
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let data = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => data.to_vec(),
            png::ColorType::Rgb => data
                .chunks_exact(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => data
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Grayscale => data.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        Some(ctx.load_texture(format!("emoji-{key}"), image, egui::TextureOptions::LINEAR))
    }
}

/// Minimal ustar scanner: 512-byte headers, octal size at 124..136.
fn scan_tar(bytes: &[u8], twemoji: bool, mut f: impl FnMut(String, usize, usize)) {
    let mut off = 0usize;
    while off + 512 <= bytes.len() {
        let header = &bytes[off..off + 512];
        let name_end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
        if name_end == 0 {
            break;
        }
        let name = String::from_utf8_lossy(&header[..name_end]).to_string();
        let size_str = String::from_utf8_lossy(&header[124..136]);
        let size = usize::from_str_radix(
            size_str.trim_matches(|c: char| c == '\0' || c == ' '),
            8,
        )
        .unwrap_or(0);
        let data_off = off + 512;
        let stem = name.strip_suffix(".png").unwrap_or(&name);
        let key = if twemoji {
            stem.to_string()
        } else {
            match stem.strip_prefix("emoji_u") {
                Some(k) => k.to_string(),
                None => String::new(),
            }
        };
        if key.is_empty() {
            // skip
        } else if data_off + size <= bytes.len() {
            f(key, data_off, size);
        }
        off = data_off + size.div_ceil(512) * 512;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(bool, String)> {
        split(text)
            .into_iter()
            .map(|p| match p {
                Piece::Text(t) => (false, t.to_string()),
                Piece::Emoji(e) => (true, e.to_string()),
            })
            .collect()
    }

    #[test]
    fn plain_text_has_no_emoji() {
        assert_eq!(kinds("Hello 123 #1 * ©"), vec![(false, "Hello 123 #1 * ©".to_string())]);
    }

    #[test]
    fn simple_and_surrounded() {
        assert_eq!(
            kinds("hi 😀 there"),
            vec![(false, "hi ".into()), (true, "😀".into()), (false, " there".into())]
        );
    }

    #[test]
    fn sequences_stay_together() {
        assert_eq!(kinds("👨‍👩‍👧"), vec![(true, "👨‍👩‍👧".into())]); // ZWJ family
        assert_eq!(kinds("👍🏽"), vec![(true, "👍🏽".into())]); // skin tone
        assert_eq!(kinds("🇸🇪"), vec![(true, "🇸🇪".into())]); // flag
        assert_eq!(kinds("1️⃣"), vec![(true, "1️⃣".into())]); // keycap
        assert_eq!(kinds("❤️"), vec![(true, "❤️".into())]); // text-default + VS16
    }

    #[test]
    fn text_default_symbols_without_vs16_stay_text() {
        assert_eq!(kinds("☑ ☐ ✔"), vec![(false, "☑ ☐ ✔".to_string())]);
    }

    #[test]
    fn bundled_archive_resolves_common_emoji() {
        let store = EmojiStore::new();
        for e in ["😀", "👍🏽", "👨‍👩‍👧", "🇸🇪", "🇯🇵", "1️⃣", "❤️", "🖼", "✅", "🏳️‍🌈"] {
            assert!(store.resolve(e).is_some(), "no image for {e}");
        }
    }
}
