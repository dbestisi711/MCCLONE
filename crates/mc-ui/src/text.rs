//! Text rendering with the pack's TrueType font (`font/minecraft-ten.ttf`).
//!
//! The font is drawn on a coarse pixel grid (its outlines sit on a grid of 70
//! font units). Each glyph is rasterised at high resolution with `fontdue`
//! and then sampled at the centre of every design pixel, giving crisp 1-bit
//! glyph bitmaps. They are packed into one `@font` atlas texture (one texel
//! per design pixel) and drawn with an integer number of physical pixels per
//! design pixel, so text stays sharp at every GUI scale.

use std::sync::Arc;

use mc_core::Rgba8Image;
use mc_core::render_types::TextureKey;
use rustc_hash::FxHashMap as HashMap;

use crate::gui::{Color, Painter};

/// Size of one design pixel in font units for the bundled font. Detected
/// from glyph bounds at load time; this is only the fallback.
const DEFAULT_GRID: f32 = 70.0;
/// Over-sampling used to rasterise glyphs before snapping to the grid.
const OVERSAMPLE: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default)]
struct Glyph {
    /// Atlas rect in texels.
    ax: u16,
    ay: u16,
    /// Bitmap size in design pixels.
    w: u8,
    h: u8,
    /// Design pixels from the pen position to the bitmap's left edge.
    left: i8,
    /// Design pixels from the baseline up to the bitmap's top edge.
    top: i8,
    /// Advance in design pixels.
    adv: u8,
}

pub struct Font {
    glyphs: HashMap<char, Glyph>,
    pub atlas: Arc<Rgba8Image>,
    pub key: TextureKey,
    /// Cap height in design pixels.
    pub cap: f32,
    /// Distance between lines in design pixels.
    pub line: f32,
}

/// Standard 16 formatting colours selected with `§0`..`§f`.
const CODE_COLORS: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xFFAA00, 0xAAAAAA, 0x555555,
    0x5555FF, 0x55FF55, 0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

impl Font {
    /// Load the TTF from the pack. Falls back to simple block glyphs when the
    /// font is missing so the UI still shows something.
    pub fn load(pack: &mc_assets::Pack) -> Font {
        let path = pack.path("font/minecraft-ten.ttf");
        let font = std::fs::read(&path)
            .ok()
            .and_then(|b| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).ok());
        match font {
            Some(f) => Self::from_fontdue(&f),
            None => {
                log::warn!("ui: could not load {}, using block glyphs", path.display());
                Self::fallback()
            }
        }
    }

    fn charset() -> impl Iterator<Item = char> {
        (' '..='~').chain('\u{A0}'..='\u{FF}')
    }

    fn from_fontdue(font: &fontdue::Font) -> Font {
        // Detect the design grid from a capital letter's height: caps are
        // 10 design pixels tall in the bundled font.
        let m = font.metrics('H', 1000.0);
        let upem = font.units_per_em();
        let grid = if m.bounds.height > 0.0 {
            m.bounds.height * upem / 1000.0 / 10.0
        } else {
            DEFAULT_GRID
        };
        let px = upem / grid * OVERSAMPLE; // font size where 1 design px = OVERSAMPLE px
        let mut bitmaps: Vec<(char, Glyph, Vec<bool>)> = Vec::new();
        for c in Self::charset() {
            if font.lookup_glyph_index(c) == 0 && c != ' ' {
                continue;
            }
            let (m, bmp) = font.rasterize(c, px);
            let units = |v: f32| v / OVERSAMPLE; // hi-res px → design px
            let adv = units(m.advance_width).round().max(1.0);
            if m.width == 0 || m.height == 0 {
                let g = Glyph {
                    adv: (adv.max(4.0)) as u8,
                    ..Default::default()
                };
                bitmaps.push((c, g, Vec::new()));
                continue;
            }
            // Design-pixel grid anchored at the outline's left edge and the
            // baseline.
            let bx0 = m.bounds.xmin;
            let by0 = (m.bounds.ymin / OVERSAMPLE).floor();
            let by1 = ((m.bounds.ymin + m.bounds.height) / OVERSAMPLE).ceil();
            let gw = (m.bounds.width / OVERSAMPLE).round().max(1.0) as i32;
            let gh = (by1 - by0).max(1.0) as i32;
            let mut bits = vec![false; (gw * gh) as usize];
            for gy in 0..gh {
                // Row 0 is the top; y is measured upward from the baseline.
                let yu = (by1 - gy as f32 - 0.5) * OVERSAMPLE;
                let row = (m.ymin as f32 + m.height as f32 - yu).floor() as i32;
                for gx in 0..gw {
                    let xu = bx0 + (gx as f32 + 0.5) * OVERSAMPLE;
                    let col = (xu - m.xmin as f32).floor() as i32;
                    if row >= 0 && col >= 0 && (row as usize) < m.height && (col as usize) < m.width
                    {
                        bits[(gy * gw + gx) as usize] =
                            bmp[row as usize * m.width + col as usize] > 127;
                    }
                }
            }
            let g = Glyph {
                w: gw as u8,
                h: gh as u8,
                left: 0,
                top: by1 as i8,
                adv: (gw as f32 + 1.0).max(adv - 1.0).min(adv + 1.0) as u8,
                ..Default::default()
            };
            bitmaps.push((c, g, bits));
        }
        Self::pack(bitmaps, 10.0, 13.0)
    }

    /// Placeholder font: every printable glyph is a small block (original,
    /// generic art used only when the pack's font cannot be read).
    fn fallback() -> Font {
        let mut v = Vec::new();
        for c in Self::charset() {
            if c == ' ' {
                v.push((
                    c,
                    Glyph {
                        adv: 4,
                        ..Default::default()
                    },
                    Vec::new(),
                ));
                continue;
            }
            let (w, h) = (5, 7);
            let bits = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    x == 0 || y == 0 || x == w - 1 || y == h - 1
                })
                .collect();
            let g = Glyph {
                w: w as u8,
                h: h as u8,
                top: 7,
                adv: 6,
                ..Default::default()
            };
            v.push((c, g, bits));
        }
        Self::pack(v, 7.0, 9.0)
    }

    fn pack(bitmaps: Vec<(char, Glyph, Vec<bool>)>, cap: f32, line: f32) -> Font {
        let atlas_w = 256u32;
        let (mut x, mut y, mut row_h) = (1u32, 1u32, 0u32);
        let mut placed = Vec::with_capacity(bitmaps.len());
        for (c, mut g, bits) in bitmaps {
            if g.w > 0 {
                if x + g.w as u32 + 1 > atlas_w {
                    x = 1;
                    y += row_h + 1;
                    row_h = 0;
                }
                g.ax = x as u16;
                g.ay = y as u16;
                x += g.w as u32 + 1;
                row_h = row_h.max(g.h as u32);
            }
            placed.push((c, g, bits));
        }
        let atlas_h = (y + row_h + 1).next_power_of_two().max(16);
        let mut img = Rgba8Image::new(atlas_w, atlas_h);
        let mut glyphs = HashMap::default();
        for (c, g, bits) in placed {
            for gy in 0..g.h as u32 {
                for gx in 0..g.w as u32 {
                    if bits[(gy * g.w as u32 + gx) as usize] {
                        img.put(g.ax as u32 + gx, g.ay as u32 + gy, [255, 255, 255, 255]);
                    }
                }
            }
            glyphs.insert(c, g);
        }
        Font {
            glyphs,
            atlas: Arc::new(img),
            key: TextureKey::new("@font"),
            cap,
            line,
        }
    }

    fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c).or_else(|| {
            let up = c.to_ascii_uppercase();
            self.glyphs.get(&up).or_else(|| self.glyphs.get(&'?'))
        })
    }

    /// Physical pixels per design pixel at a GUI scale, so the cap height is
    /// close to 7 GUI pixels while staying on the physical pixel grid.
    pub fn unit(&self, scale: f32) -> f32 {
        (scale * 7.0 / self.cap).round().max(1.0)
    }

    /// Cap height in GUI pixels.
    pub fn cap_height(&self, scale: f32) -> f32 {
        self.cap * self.unit(scale) / scale
    }

    /// Line spacing in GUI pixels.
    pub fn line_height(&self, scale: f32) -> f32 {
        (self.line * self.unit(scale) / scale).ceil()
    }

    /// Width of `text` in GUI pixels (formatting codes are skipped).
    pub fn width(&self, text: &str, scale: f32) -> f32 {
        self.width_units(text) as f32 * self.unit(scale) / scale
    }

    fn width_units(&self, text: &str) -> i32 {
        let mut w = 0i32;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '§' {
                chars.next();
                continue;
            }
            if let Some(g) = self.glyph(c) {
                w += g.adv as i32;
            }
        }
        (w - 1).max(0)
    }

    /// Draw `text` with its cap top at (`x`, `y`) in GUI pixels. `size`
    /// multiplies the glyph size (2.0 for titles). Returns the width in GUI
    /// pixels. Supports `§0`..`§f` colour codes and `§r` reset.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        p: &mut Painter,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        shadow: bool,
        size: f32,
    ) -> f32 {
        let unit = self.unit(p.scale) * size.max(0.25);
        if shadow {
            let sc = [color[0] * 0.25, color[1] * 0.25, color[2] * 0.25, color[3]];
            self.draw_run(p, text, x, y, unit, unit, sc, true);
        }
        self.draw_run(p, text, x, y, 0.0, unit, color, false)
    }

    /// Draw text with a 1-unit outline in `outline` (the XP level style).
    pub fn draw_outlined(
        &self,
        p: &mut Painter,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        outline: Color,
    ) -> f32 {
        let unit = self.unit(p.scale);
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            self.draw_run_xy(p, text, x, y, dx * unit, dy * unit, unit, outline, true);
        }
        self.draw_run(p, text, x, y, 0.0, unit, color, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self,
        p: &mut Painter,
        text: &str,
        x: f32,
        y: f32,
        offset: f32,
        unit: f32,
        color: Color,
        is_shadow: bool,
    ) -> f32 {
        self.draw_run_xy(p, text, x, y, offset, offset, unit, color, is_shadow)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_run_xy(
        &self,
        p: &mut Painter,
        text: &str,
        x: f32,
        y: f32,
        ox: f32,
        oy: f32,
        unit: f32,
        color: Color,
        is_shadow: bool,
    ) -> f32 {
        let (aw, ah) = (self.atlas.width as f32, self.atlas.height as f32);
        let mut pen = (x * p.scale).round() + ox;
        let start = pen;
        let base = (y * p.scale).round() + oy + self.cap * unit;
        let mut cur = color;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '§' {
                if let Some(code) = chars.next() {
                    if let Some(d) = code.to_digit(16) {
                        let mut nc = crate::gui::rgb(CODE_COLORS[d as usize]);
                        if is_shadow {
                            nc = [nc[0] * 0.25, nc[1] * 0.25, nc[2] * 0.25, 1.0];
                        }
                        nc[3] = color[3];
                        cur = nc;
                    } else if code == 'r' {
                        cur = color;
                    }
                }
                continue;
            }
            let Some(g) = self.glyph(c) else { continue };
            if g.w > 0 {
                let rect = [
                    pen + g.left as f32 * unit,
                    base - g.top as f32 * unit,
                    g.w as f32 * unit,
                    g.h as f32 * unit,
                ];
                let uv = [
                    g.ax as f32 / aw,
                    g.ay as f32 / ah,
                    (g.ax as f32 + g.w as f32) / aw,
                    (g.ay as f32 + g.h as f32) / ah,
                ];
                p.quad_phys(&self.key, rect, uv, cur);
            }
            pen += g.adv as f32 * unit;
        }
        ((pen - start) / p.scale - unit / p.scale).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font {
        let pack = mc_assets::Pack::find().expect("pack");
        Font::load(&pack)
    }

    #[test]
    fn glyphs_are_crisp_and_measurable() {
        let f = font();
        let a = f.glyph('A').unwrap();
        assert!(a.w >= 5 && a.h >= 7, "glyph A is {}x{}", a.w, a.h);
        assert!(f.width("AAA", 3.0) > f.width("A", 3.0));
        assert_eq!(f.width("", 3.0), 0.0);
        // Formatting codes take no space.
        assert_eq!(f.width("§cAB", 3.0), f.width("AB", 3.0));
        // Atlas is binary (crisp pixels).
        assert!(f.atlas.data.chunks(4).all(|p| p[3] == 0 || p[3] == 255));
    }

    #[test]
    fn unit_is_integral() {
        let f = font();
        for s in 1..8 {
            let u = f.unit(s as f32);
            assert_eq!(u.fract(), 0.0);
            let cap = f.cap_height(s as f32);
            assert!((4.5..=10.5).contains(&cap), "scale {s}: cap {cap}");
        }
    }
}
