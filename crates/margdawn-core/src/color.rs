//! Minimal RGB color utilities shared by both renderers.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa` (alpha is blended away by the
    /// caller via [`Color::blend`]; here it is simply ignored).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().strip_prefix('#')?;
        let h = |a: &str, i: usize| u8::from_str_radix(&a[i * 2..i * 2 + 2], 16).ok();
        // short-form digit d (0..16) becomes 0x11 * d = d * 17
        let prev = |c: char| -> Option<u8> { c.to_digit(16).map(|d| (d * 17) as u8) };
        match s.len() {
            3 => Some(Self {
                r: s.chars().next().and_then(prev)?,
                g: s.chars().nth(1).and_then(prev)?,
                b: s.chars().nth(2).and_then(prev)?,
            }),
            6 => Some(Self {
                r: h(s, 0)?,
                g: h(s, 1)?,
                b: h(s, 2)?,
            }),
            8 => Some(Self {
                r: h(s, 0)?,
                g: h(s, 1)?,
                b: h(s, 2)?,
            }),
            _ => None,
        }
    }

    /// Linear-ish blend: `t = 0` returns `self`, `t = 1` returns `other`.
    pub fn blend(self, other: Self, t: f32) -> Self {
        let lerp = |a: u8, b: u8| -> u8 {
            let v = a as f32 + ((b as f32 - a as f32) * t);
            v.round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
        }
    }

    pub fn lighten(self, t: f32) -> Self {
        self.blend(Self::new(255, 255, 255), t)
    }

    pub fn darken(self, t: f32) -> Self {
        self.blend(Self::new(0, 0, 0), t)
    }

    pub fn luma(self) -> f32 {
        0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32
    }

    pub fn is_dark(self) -> bool {
        self.luma() < 128.0
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Best-quantized xterm-256 color index (6x6x6 cube + grayscale ramp).
    pub fn ansi256(self) -> u8 {
        let levels = [0u8, 95, 135, 175, 215, 255];
        let nearest = |c: u8| -> usize {
            let mut best = 0;
            let mut bestd = 300;
            for (i, &l) in levels.iter().enumerate() {
                let d = (i128::from(c) - i128::from(l)).abs();
                if d < bestd {
                    bestd = d;
                    best = i;
                }
            }
            best
        };
        let q = self.r as i32 + self.g as i32 + self.b as i32;
        if q > 690 || q < 15 {
            let g = best_gray(self);
            if g > 231 {
                return g;
            }
        }
        let idx = nearest(self.r) * 36 + nearest(self.g) * 6 + nearest(self.b);
        (16 + idx).min(231) as u8
    }

    pub fn to_truecolor(self) -> String {
        format!("{};{};{}", self.r, self.g, self.b)
    }

    pub fn to_256color(self) -> String {
        self.ansi256().to_string()
    }
}

fn best_gray(c: Color) -> u8 {
    let g = ((c.r as u32 + c.g as u32 + c.b as u32) / 3) as u8;
    let mut best = 232u8;
    let mut bestd = 300;
    for i in 0..24u8 {
        let v = 8 + i * 10;
        let d = (i32::from(g) - i32::from(v)).abs();
        if d < bestd {
            bestd = d;
            best = 232 + i;
        }
    }
    best
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl serde::Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Color::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid color: `{text}` (expected #rgb or #rrggbb)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_variants() {
        assert_eq!(Color::parse("#fff"), Some(Color::new(255, 255, 255)));
        assert_eq!(Color::parse("#F92672"), Some(Color::new(0xF9, 0x26, 0x72)));
        assert_eq!(Color::parse("#112233FF"), Some(Color::new(0x11, 0x22, 0x33)));
        assert_eq!(Color::parse("nope"), None);
    }

    #[test]
    fn blends() {
        assert_eq!(Color::new(0, 0, 0).blend(Color::new(10, 10, 10), 0.5), Color::new(5, 5, 5));
    }

    #[test]
    fn ansi256_range() {
        let i = Color::new(255, 0, 0).ansi256();
        assert!((16..=255).contains(&i));
    }
}
