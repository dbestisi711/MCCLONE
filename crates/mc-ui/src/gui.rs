//! GUI skin (sprites and nine-slice panels read from the resource pack at
//! runtime) and the [`Painter`], which turns GUI-pixel drawing commands into
//! physical-pixel [`UiQuad`]s.
//!
//! Coordinates passed to the painter are in *GUI pixels*; one GUI pixel is
//! `scale` physical pixels (an integer chosen from the window size, like the
//! "auto" GUI scale of the original game).

use glam::Vec2;
use mc_assets::Pack;
use mc_core::render_types::{TextureKey, UiDrawList, UiQuad};

pub type Color = [f32; 4];

pub const WHITE: Color = [1.0, 1.0, 1.0, 1.0];

/// `0xAARRGGBB` → linear-ish RGBA multiplier.
pub const fn argb(c: u32) -> Color {
    [
        ((c >> 16) & 0xFF) as f32 / 255.0,
        ((c >> 8) & 0xFF) as f32 / 255.0,
        (c & 0xFF) as f32 / 255.0,
        ((c >> 24) & 0xFF) as f32 / 255.0,
    ]
}

/// `0xRRGGBB` → opaque colour.
pub const fn rgb(c: u32) -> Color {
    argb(0xFF00_0000 | c)
}

pub fn with_alpha(mut c: Color, a: f32) -> Color {
    c[3] *= a;
    c
}

pub fn shade(c: Color, f: f32) -> Color {
    [c[0] * f, c[1] * f, c[2] * f, c[3]]
}

/// A whole image (or a sub-rectangle of one) from the pack.
#[derive(Clone, Debug)]
pub struct Sprite {
    pub key: TextureKey,
    /// Image size in texels.
    pub tex_w: f32,
    pub tex_h: f32,
    /// Source rectangle in texels (x, y, w, h).
    pub src: [f32; 4],
    /// False when the image could not be loaded; callers draw a fallback.
    pub ok: bool,
}

impl Sprite {
    pub fn load(pack: &Pack, path: &str) -> Sprite {
        match pack.load_image(path) {
            Some(img) => Sprite {
                key: TextureKey::new(path),
                tex_w: img.width as f32,
                tex_h: img.height as f32,
                src: [0.0, 0.0, img.width as f32, img.height as f32],
                ok: true,
            },
            None => {
                log::warn!("ui: missing texture {path}, using a placeholder");
                Sprite {
                    key: TextureKey::white(),
                    tex_w: 1.0,
                    tex_h: 1.0,
                    src: [0.0, 0.0, 1.0, 1.0],
                    ok: false,
                }
            }
        }
    }

    /// A sub-rectangle of this sprite's image (texel coordinates).
    pub fn sub(&self, x: f32, y: f32, w: f32, h: f32) -> Sprite {
        Sprite {
            src: [x, y, w, h],
            ..self.clone()
        }
    }

    pub fn width(&self) -> f32 {
        self.src[2]
    }
    pub fn height(&self) -> f32 {
        self.src[3]
    }

    /// Normalised UVs of a texel rectangle (relative to `src`).
    fn uv(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let x0 = self.src[0] + x;
        let y0 = self.src[1] + y;
        [
            x0 / self.tex_w,
            y0 / self.tex_h,
            (x0 + w) / self.tex_w,
            (y0 + h) / self.tex_h,
        ]
    }
}

/// A nine-slice image: corners keep their size, edges and centre stretch.
/// Bedrock stores the slice sizes in a `.json` next to the image
/// (`nineslice_size` + `base_size`); the image may be a multiple of the base
/// size (high-res), in which case texel insets are scaled accordingly.
#[derive(Clone, Debug)]
pub struct NineSlice {
    pub sprite: Sprite,
    /// Insets in GUI pixels: left, top, right, bottom.
    pub inset: [f32; 4],
    /// Insets in texels.
    pub tex_inset: [f32; 4],
    /// Fallback colours (fill, light edge, dark edge, outline) used when the
    /// image is missing.
    pub fallback: [Color; 4],
}

impl NineSlice {
    /// Load `path` (no extension) and its `.json` slice info. `default_inset`
    /// is used when the json is absent.
    pub fn load(pack: &Pack, path: &str, default_inset: f32, fallback: [Color; 4]) -> NineSlice {
        let sprite = Sprite::load(pack, path);
        let mut inset = [default_inset; 4];
        let mut base = [sprite.tex_w, sprite.tex_h];
        if let Some(json) = pack.load_json(&format!("{path}.json")) {
            match &json["nineslice_size"] {
                serde_json::Value::Number(n) => {
                    inset = [n.as_f64().unwrap_or(0.0) as f32; 4];
                }
                serde_json::Value::Array(a) if a.len() == 4 => {
                    for (i, v) in a.iter().enumerate() {
                        inset[i] = v.as_f64().unwrap_or(0.0) as f32;
                    }
                }
                _ => {}
            }
            if let Some(b) = json["base_size"].as_array()
                && b.len() == 2
            {
                base = [
                    b[0].as_f64().unwrap_or(base[0] as f64) as f32,
                    b[1].as_f64().unwrap_or(base[1] as f64) as f32,
                ];
            }
        }
        let (rx, ry) = if sprite.ok && base[0] > 0.0 && base[1] > 0.0 {
            (sprite.tex_w / base[0], sprite.tex_h / base[1])
        } else {
            (1.0, 1.0)
        };
        NineSlice {
            tex_inset: [inset[0] * rx, inset[1] * ry, inset[2] * rx, inset[3] * ry],
            inset,
            sprite,
            fallback,
        }
    }

    /// Use explicit insets (texel == GUI pixel) for images without json.
    pub fn fixed(pack: &Pack, path: &str, inset: f32, fallback: [Color; 4]) -> NineSlice {
        let sprite = Sprite::load(pack, path);
        NineSlice {
            inset: [inset; 4],
            tex_inset: [inset; 4],
            sprite,
            fallback,
        }
    }
}

/// Every pack texture the UI uses, loaded once.
pub struct Skin {
    pub panel: NineSlice,
    pub slot: NineSlice,
    pub button: NineSlice,
    pub button_hover: NineSlice,
    pub button_pressed: NineSlice,
    pub tab_front: NineSlice,
    pub tab_back: NineSlice,
    pub scroll_handle: NineSlice,
    pub xp_empty: NineSlice,
    pub xp_full: NineSlice,
    pub hotbar: Vec<Sprite>,
    pub hotbar_start: Sprite,
    pub hotbar_end: Sprite,
    pub hotbar_selected: Sprite,
    pub crosshair: Sprite,
    pub heart_full: Sprite,
    pub heart_half: Sprite,
    pub heart_bg: Sprite,
    pub heart_blink: Sprite,
    pub heart_flash: Sprite,
    pub heart_flash_half: Sprite,
    pub hunger_full: Sprite,
    pub hunger_half: Sprite,
    pub hunger_bg: Sprite,
    pub armor_full: Sprite,
    pub armor_half: Sprite,
    pub armor_empty: Sprite,
    pub bubble: Sprite,
    pub bubble_pop: Sprite,
    pub arrow: Sprite,
    pub arrow_small: Sprite,
    pub arrow_active: Sprite,
    pub arrow_inactive: Sprite,
    pub flame_empty: Sprite,
    pub flame_full: Sprite,
    pub empty_armor: [Sprite; 4],
    pub empty_offhand: Sprite,
    pub trash: Sprite,
    pub skin: Sprite,
}

impl Skin {
    pub fn load(pack: &Pack) -> Skin {
        let panel_fb = [rgb(0xC6C6C6), rgb(0xFFFFFF), rgb(0x555555), rgb(0x000000)];
        let slot_fb = [rgb(0x8B8B8B), rgb(0x373737), rgb(0xFFFFFF), [0.0; 4]];
        let button_fb = [rgb(0x6F6F6F), rgb(0xA8A8A8), rgb(0x404040), rgb(0x000000)];
        let hover_fb = [rgb(0x7E88BF), rgb(0xBEC6F0), rgb(0x45507F), rgb(0xFFFFFF)];
        let xp_fb = [rgb(0x202020), rgb(0x202020), rgb(0x202020), rgb(0x000000)];
        let xp_full_fb = [rgb(0x80FF20), rgb(0x80FF20), rgb(0x80FF20), [0.0; 4]];
        let ui = |n: &str| format!("textures/ui/{n}");
        let s = |n: &str| Sprite::load(pack, &ui(n));
        let icons = Sprite::load(pack, "textures/gui/icons");
        Skin {
            panel: NineSlice::load(pack, &ui("dialog_background_opaque"), 4.0, panel_fb),
            slot: NineSlice::load(pack, &ui("cell_image"), 1.0, slot_fb),
            button: NineSlice::fixed(
                pack,
                "textures/gui/newgui/buttons/border/base",
                2.0,
                button_fb,
            ),
            button_hover: NineSlice::fixed(
                pack,
                "textures/gui/newgui/buttons/border/hover",
                2.0,
                hover_fb,
            ),
            button_pressed: NineSlice::fixed(
                pack,
                "textures/gui/newgui/buttons/border/basePress",
                2.0,
                button_fb,
            ),
            tab_front: NineSlice::load(pack, &ui("TabTopFront"), 4.0, panel_fb),
            tab_back: NineSlice::load(pack, &ui("TabTopBack"), 4.0, button_fb),
            scroll_handle: NineSlice::load(pack, &ui("ScrollHandle"), 1.0, button_fb),
            xp_empty: NineSlice::load(pack, &ui("experiencebarempty"), 1.0, xp_fb),
            xp_full: NineSlice::load(pack, &ui("experiencebarfull"), 1.0, xp_full_fb),
            hotbar: (0..9).map(|i| s(&format!("hotbar_{i}"))).collect(),
            hotbar_start: s("hotbar_start_cap"),
            hotbar_end: s("hotbar_end_cap"),
            hotbar_selected: s("selected_hotbar_slot"),
            crosshair: icons.sub(0.0, 0.0, 15.0, 15.0),
            heart_full: s("heart"),
            heart_half: s("heart_half"),
            heart_bg: s("heart_background"),
            heart_blink: s("heart_blink"),
            heart_flash: s("heart_flash"),
            heart_flash_half: s("heart_flash_half"),
            hunger_full: s("hunger_full"),
            hunger_half: s("hunger_half"),
            hunger_bg: s("hunger_background"),
            armor_full: s("armor_full"),
            armor_half: s("armor_half"),
            armor_empty: s("armor_empty"),
            bubble: s("bubble"),
            bubble_pop: s("bubble_pop"),
            arrow: s("arrow_large"),
            arrow_small: s("arrow"),
            arrow_active: s("arrow_active"),
            arrow_inactive: s("arrow_inactive"),
            flame_empty: s("flame_empty_image"),
            flame_full: s("flame_full_image"),
            empty_armor: [
                s("empty_armor_slot_helmet"),
                s("empty_armor_slot_chestplate"),
                s("empty_armor_slot_leggings"),
                s("empty_armor_slot_boots"),
            ],
            empty_offhand: s("empty_armor_slot_shield"),
            trash: Sprite::load(pack, "textures/gui/newgui/trash"),
            skin: Sprite::load(pack, "textures/entity/steve"),
        }
    }
}

/// Draws into a [`UiDrawList`] using GUI-pixel coordinates.
pub struct Painter<'a> {
    pub out: &'a mut UiDrawList,
    /// Physical pixels per GUI pixel.
    pub scale: f32,
    /// Optional clip rectangle in GUI pixels (x0, y0, x1, y1).
    pub clip: Option<[f32; 4]>,
}

impl<'a> Painter<'a> {
    pub fn new(out: &'a mut UiDrawList, scale: f32) -> Self {
        Painter {
            out,
            scale,
            clip: None,
        }
    }

    /// Raw axis-aligned quad: GUI rect + normalised UV rect.
    pub fn quad(&mut self, key: &TextureKey, rect: [f32; 4], uv: [f32; 4], color: Color) {
        self.quad_layer(key, rect, uv, 0, color);
    }

    pub fn quad_layer(
        &mut self,
        key: &TextureKey,
        rect: [f32; 4],
        mut uv: [f32; 4],
        layer: u32,
        color: Color,
    ) {
        let [mut x, mut y, mut w, mut h] = rect;
        if w <= 0.0 || h <= 0.0 || color[3] <= 0.0 {
            return;
        }
        if let Some([cx0, cy0, cx1, cy1]) = self.clip {
            let (x1, y1) = (x + w, y + h);
            let (nx0, ny0, nx1, ny1) = (x.max(cx0), y.max(cy0), x1.min(cx1), y1.min(cy1));
            if nx0 >= nx1 || ny0 >= ny1 {
                return;
            }
            let du = (uv[2] - uv[0]) / w;
            let dv = (uv[3] - uv[1]) / h;
            uv = [
                uv[0] + (nx0 - x) * du,
                uv[1] + (ny0 - y) * dv,
                uv[2] - (x1 - nx1) * du,
                uv[3] - (y1 - ny1) * dv,
            ];
            (x, y, w, h) = (nx0, ny0, nx1 - nx0, ny1 - ny0);
        }
        let s = self.scale;
        let mut q = UiQuad::rect(key.clone(), x * s, y * s, w * s, h * s, uv, color);
        q.layer = layer;
        self.out.push(q);
    }

    /// Quad given directly in physical pixels (used by text, which snaps to
    /// the physical pixel grid itself).
    pub fn quad_phys(&mut self, key: &TextureKey, rect: [f32; 4], uv: [f32; 4], color: Color) {
        if let Some([cx0, cy0, cx1, cy1]) = self.clip {
            let s = self.scale;
            if rect[0] + rect[2] <= cx0 * s
                || rect[1] + rect[3] <= cy0 * s
                || rect[0] >= cx1 * s
                || rect[1] >= cy1 * s
            {
                return;
            }
        }
        self.out.push(UiQuad::rect(
            key.clone(),
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            uv,
            color,
        ));
    }

    /// Arbitrary quad (corners TL, TR, BR, BL) in GUI pixels.
    pub fn quad_corners(
        &mut self,
        key: &TextureKey,
        pos: [Vec2; 4],
        uv: [Vec2; 4],
        layer: u32,
        color: Color,
    ) {
        let s = self.scale;
        self.out.push(UiQuad {
            texture: key.clone(),
            pos: pos.map(|p| p * s),
            uv,
            layer,
            color,
        });
    }

    pub fn solid(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.quad(
            &TextureKey::white(),
            [x, y, w, h],
            [0.0, 0.0, 1.0, 1.0],
            color,
        );
    }

    /// Vertical gradient approximated with horizontal bands.
    pub fn gradient(&mut self, x: f32, y: f32, w: f32, h: f32, top: Color, bottom: Color) {
        let bands = 24;
        for i in 0..bands {
            let t = (i as f32 + 0.5) / bands as f32;
            let c = [
                top[0] + (bottom[0] - top[0]) * t,
                top[1] + (bottom[1] - top[1]) * t,
                top[2] + (bottom[2] - top[2]) * t,
                top[3] + (bottom[3] - top[3]) * t,
            ];
            let y0 = y + h * i as f32 / bands as f32;
            let y1 = y + h * (i + 1) as f32 / bands as f32;
            self.solid(x, y0, w, y1 - y0, c);
        }
    }

    /// Draw a sprite stretched to the given rect.
    pub fn sprite(&mut self, s: &Sprite, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if !s.ok {
            self.solid(x, y, w, h, with_alpha(color, 0.5));
            return;
        }
        let uv = s.uv(0.0, 0.0, s.width(), s.height());
        self.quad(&s.key, [x, y, w, h], uv, color);
    }

    /// Draw a sprite at its natural size (1 texel = 1 GUI pixel).
    pub fn sprite_at(&mut self, s: &Sprite, x: f32, y: f32, color: Color) {
        self.sprite(s, x, y, s.width(), s.height(), color);
    }

    /// Draw part of a sprite: `src` = texel rect inside the sprite.
    pub fn sprite_part(&mut self, s: &Sprite, src: [f32; 4], dst: [f32; 4], color: Color) {
        if !s.ok {
            return;
        }
        let uv = s.uv(src[0], src[1], src[2], src[3]);
        self.quad(&s.key, dst, uv, color);
    }

    /// Draw a nine-slice panel.
    pub fn nine(&mut self, n: &NineSlice, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if !n.sprite.ok {
            self.bevel(x, y, w, h, n.fallback, color);
            return;
        }
        let [l, t, r, b] = n.inset;
        let [tl, tt, tr, tb] = n.tex_inset;
        let (tw, th) = (n.sprite.width(), n.sprite.height());
        // Columns/rows: (dst start, dst size, src start, src size).
        let cols = [
            (x, l, 0.0, tl),
            (x + l, w - l - r, tl, tw - tl - tr),
            (x + w - r, r, tw - tr, tr),
        ];
        let rows = [
            (y, t, 0.0, tt),
            (y + t, h - t - b, tt, th - tt - tb),
            (y + h - b, b, th - tb, tb),
        ];
        for &(dy, dh, sy, sh) in &rows {
            for &(dx, dw, sx, sw) in &cols {
                if dw > 0.0 && dh > 0.0 && sw > 0.0 && sh > 0.0 {
                    self.sprite_part(&n.sprite, [sx, sy, sw, sh], [dx, dy, dw, dh], color);
                }
            }
        }
    }

    /// Procedural bevelled box (placeholder art when a texture is missing).
    pub fn bevel(&mut self, x: f32, y: f32, w: f32, h: f32, c: [Color; 4], tint: Color) {
        let m = |a: Color| {
            [
                a[0] * tint[0],
                a[1] * tint[1],
                a[2] * tint[2],
                a[3] * tint[3],
            ]
        };
        let o = if c[3][3] > 0.0 { 1.0 } else { 0.0 };
        if o > 0.0 {
            self.solid(x, y, w, h, m(c[3]));
        }
        self.solid(x + o, y + o, w - 2.0 * o, h - 2.0 * o, m(c[2]));
        self.solid(x + o, y + o, w - 2.0 * o - 1.0, h - 2.0 * o - 1.0, m(c[1]));
        self.solid(
            x + o + 1.0,
            y + o + 1.0,
            w - 2.0 * o - 2.0,
            h - 2.0 * o - 2.0,
            m(c[0]),
        );
    }
}

/// Pick the GUI scale like the original "auto" setting: the largest integer
/// such that the GUI is at least 320×240 GUI pixels. `max` (0 = unlimited)
/// caps it, e.g. for a user setting.
pub fn auto_scale(width: f32, height: f32, max: u32) -> f32 {
    let mut s = 1u32;
    while (width / (s + 1) as f32) >= 320.0 && (height / (s + 1) as f32) >= 240.0 {
        s += 1;
        if max != 0 && s >= max {
            break;
        }
    }
    s as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scale_matches_window_sizes() {
        assert_eq!(auto_scale(1280.0, 720.0, 0), 3.0);
        assert_eq!(auto_scale(1920.0, 1080.0, 0), 4.0);
        assert_eq!(auto_scale(854.0, 480.0, 0), 2.0);
        assert_eq!(auto_scale(320.0, 240.0, 0), 1.0);
        assert_eq!(auto_scale(100.0, 100.0, 0), 1.0);
        assert_eq!(auto_scale(3840.0, 2160.0, 2), 2.0);
    }

    #[test]
    fn argb_unpacks() {
        assert_eq!(argb(0x80FF0000), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
