//! Item icons for slots, the hotbar and the cursor.
//!
//! Resolution order per item:
//! 1. `assets.items` (icons provided by the asset crate),
//! 2. block items: an isometric cube built from three `@blocks` quads (or a
//!    flat block tile for plants / torches),
//! 3. a private fallback that reads the item's texture from the pack
//!    (`textures/item_texture.json`, then `textures/items/<name>`).
//!
//! Flat icons are packed into one `@items` atlas uploaded on the first frame.

use std::sync::Arc;

use glam::Vec2;
use mc_assets::{Assets, TileId};
use mc_core::block::{Shape, Tint};
use mc_core::item::ItemKind;
use mc_core::render_types::TextureKey;
use mc_core::{Face, ItemId, ItemStack, Rgba8Image};

use crate::gui::{Color, Painter, WHITE, rgb};
use crate::text::Font;

/// Atlas cell edge (icon + 1 texel transparent gutter on each side).
const CELL: u32 = 18;
const ICON: u32 = 16;

/// Base tile, its colour, and an optional (overlay tile, overlay colour).
type CubeFace = (TileId, Color, Option<(TileId, Color)>);

#[derive(Clone, Debug)]
enum Icon {
    None,
    /// Normalised UV rect in the `@items` atlas.
    Flat([f32; 4]),
    /// Flat block tile (plants, torches) with tint.
    Tile {
        tile: TileId,
        tint: Color,
    },
    /// Isometric cube: (tile, tint, overlay) for the top, left and right faces.
    Cube {
        faces: [CubeFace; 3],
        /// Height of the block in 1/16ths (16 = full cube).
        height: f32,
    },
}

pub struct ItemIcons {
    icons: Vec<Icon>,
    pub atlas: Arc<Rgba8Image>,
    pub key: TextureKey,
    blocks_key: TextureKey,
}

fn tint_color(c: u32) -> Color {
    rgb(c)
}

impl ItemIcons {
    pub fn build(assets: &Assets) -> ItemIcons {
        let count = ItemId::count();
        let mut flat: Vec<(usize, Rgba8Image)> = Vec::new();
        let mut icons = vec![Icon::None; count];
        let item_json = assets.pack.load_json("textures/item_texture.json");
        let grass = tint_color(assets.colormaps.grass(0.8, 0.4));
        let foliage = tint_color(assets.colormaps.foliage(0.8, 0.4));

        for item in ItemId::all() {
            let i = item.0 as usize;
            if let Some(img) = assets.items.get(item) {
                flat.push((i, (**img).clone()));
                continue;
            }
            if let Some(block) = item.block() {
                let def = block.def();
                let tint = match def.tint {
                    Tint::None => WHITE,
                    Tint::Grass => grass,
                    Tint::Foliage => foliage,
                    Tint::Water => tint_color(0x44AFF5),
                    Tint::Fixed(c) => tint_color(c),
                };
                let b = &assets.blocks;
                let face_info = |f: Face| {
                    let tinted = b
                        .tinted
                        .get(block.0 as usize)
                        .map(|t| t[f.index()])
                        .unwrap_or(false);
                    let overlay = b.overlay.get(block.0 as usize).and_then(|o| o[f.index()]);
                    let base_tint = if tinted && overlay.is_none() {
                        tint
                    } else {
                        WHITE
                    };
                    (b.face(block, f), base_tint, overlay, tint)
                };
                icons[i] = match def.shape {
                    Shape::None => Icon::None,
                    Shape::Cross | Shape::Torch => {
                        // Prefer a dedicated item texture when the pack has one.
                        if let Some(img) = load_item_texture(assets, item_json.as_ref(), def.name) {
                            flat.push((i, img));
                            continue;
                        }
                        let (tile, t, _, _) = face_info(Face::North);
                        Icon::Tile { tile, tint: t }
                    }
                    Shape::Layer(h) if h <= 1 => {
                        let (tile, t, _, _) = face_info(Face::Up);
                        Icon::Tile { tile, tint: t }
                    }
                    shape => {
                        let height = match shape {
                            Shape::Layer(h) => h as f32,
                            _ => 16.0,
                        };
                        let mk = |f: Face| {
                            let (tile, c, ov, oc) = face_info(f);
                            (tile, c, ov.map(|o| (o, oc)))
                        };
                        Icon::Cube {
                            faces: [mk(Face::Up), mk(Face::South), mk(Face::East)],
                            height,
                        }
                    }
                };
                continue;
            }
            let tex = item.def().map(|d| d.texture).unwrap_or("");
            if let Some(img) = load_item_texture(assets, item_json.as_ref(), tex) {
                flat.push((i, img));
            }
        }

        // Pack flat icons.
        let cols = 32u32;
        let rows = (flat.len() as u32).div_ceil(cols).max(1);
        let mut atlas = Rgba8Image::new(cols * CELL, (rows * CELL).next_power_of_two());
        for (n, (i, img)) in flat.into_iter().enumerate() {
            let (cx, cy) = (n as u32 % cols, n as u32 / cols);
            let icon = fit_icon(&img);
            atlas.blit(&icon, cx * CELL + 1, cy * CELL + 1);
            let (aw, ah) = (atlas.width as f32, atlas.height as f32);
            let x0 = (cx * CELL + 1) as f32;
            let y0 = (cy * CELL + 1) as f32;
            icons[i] = Icon::Flat([
                x0 / aw,
                y0 / ah,
                (x0 + ICON as f32) / aw,
                (y0 + ICON as f32) / ah,
            ]);
        }
        ItemIcons {
            icons,
            atlas: Arc::new(atlas),
            key: TextureKey::new("@items"),
            blocks_key: TextureKey::new("@blocks"),
        }
    }

    pub fn has_icon(&self, item: ItemId) -> bool {
        !matches!(self.icons.get(item.0 as usize), None | Some(Icon::None))
    }

    /// Draw an item icon in a `size`×`size` GUI-pixel box (16 = slot size),
    /// multiplied by `tint`.
    pub fn draw(&self, p: &mut Painter, item: ItemId, x: f32, y: f32, size: f32, tint: Color) {
        let k = size / 16.0;
        let a = |c: Color| {
            [
                c[0] * tint[0],
                c[1] * tint[1],
                c[2] * tint[2],
                c[3] * tint[3],
            ]
        };
        match self.icons.get(item.0 as usize).unwrap_or(&Icon::None) {
            Icon::None => {
                // Placeholder: a small magenta/black square.
                p.solid(
                    x + 2.0 * k,
                    y + 2.0 * k,
                    12.0 * k,
                    12.0 * k,
                    a(rgb(0xF800F8)),
                );
                p.solid(x + 2.0 * k, y + 2.0 * k, 6.0 * k, 6.0 * k, a(rgb(0x000000)));
                p.solid(x + 8.0 * k, y + 8.0 * k, 6.0 * k, 6.0 * k, a(rgb(0x000000)));
            }
            Icon::Flat(uv) => p.quad(&self.key, [x, y, size, size], *uv, a(WHITE)),
            Icon::Tile { tile, tint } => p.quad_layer(
                &self.blocks_key,
                [x, y, size, size],
                [0.0, 0.0, 1.0, 1.0],
                *tile as u32,
                a(*tint),
            ),
            Icon::Cube { faces, height } => {
                self.draw_cube(p, faces, *height, x, y, k, tint);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cube(
        &self,
        p: &mut Painter,
        faces: &[CubeFace; 3],
        height: f32,
        x: f32,
        y: f32,
        k: f32,
        tint: Color,
    ) {
        // Isometric projection of a unit cube into a 16×16 box: the top is a
        // rhombus, the two visible sides are parallelograms.
        let hw = 7.0; // half width
        let hh = 3.6; // half height of the top rhombus
        let side = 8.4 * height / 16.0; // vertical side length
        let y0 = 0.3 + (8.4 - side);
        let v = |px: f32, py: f32| Vec2::new(x + px * k, y + py * k);
        let top = v(8.0, y0);
        let left = v(8.0 - hw, y0 + hh);
        let right = v(8.0 + hw, y0 + hh);
        let center = v(8.0, y0 + 2.0 * hh);
        let bl = v(8.0 - hw, y0 + hh + side);
        let br = v(8.0 + hw, y0 + hh + side);
        let bottom = v(8.0, y0 + 2.0 * hh + side);
        let vfrac = 1.0 - height / 16.0; // partial blocks show the top of the side tile
        let full = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let side_uv = [
            Vec2::new(0.0, vfrac),
            Vec2::new(1.0, vfrac),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let shapes: [([Vec2; 4], [Vec2; 4], f32); 3] = [
            ([top, right, center, left], full, 1.0),
            ([left, center, bottom, bl], side_uv, 0.8),
            ([center, right, br, bottom], side_uv, 0.62),
        ];
        for (i, (pos, uv, light)) in shapes.into_iter().enumerate() {
            let (tile, face_tint, overlay) = faces[i];
            let lit = |t: Color| {
                [
                    t[0] * light * tint[0],
                    t[1] * light * tint[1],
                    t[2] * light * tint[2],
                    tint[3],
                ]
            };
            p.quad_corners(&self.blocks_key, pos, uv, tile as u32, lit(face_tint));
            if let Some((o, oc)) = overlay {
                p.quad_corners(&self.blocks_key, pos, uv, o as u32, lit(oc));
            }
        }
    }

    /// Icon + stack count + durability bar, like an inventory slot.
    pub fn draw_stack(&self, p: &mut Painter, font: &Font, stack: &ItemStack, x: f32, y: f32) {
        self.draw(p, stack.item, x, y, 16.0, WHITE);
        self.draw_decorations(p, font, stack, x, y, None);
    }

    /// Count text and durability bar. `count_override` replaces the count
    /// text (e.g. drag previews).
    pub fn draw_decorations(
        &self,
        p: &mut Painter,
        font: &Font,
        stack: &ItemStack,
        x: f32,
        y: f32,
        count_override: Option<&str>,
    ) {
        if let ItemKind::Tool { durability, .. } | ItemKind::Armor { durability, .. } =
            stack.item.kind()
            && stack.damage > 0
            && durability > 0
        {
            let frac = 1.0 - (stack.damage as f32 / durability as f32).clamp(0.0, 1.0);
            let w = (13.0 * frac).round();
            p.solid(x + 2.0, y + 13.0, 13.0, 2.0, [0.0, 0.0, 0.0, 1.0]);
            p.solid(x + 2.0, y + 13.0, w, 1.0, durability_color(frac));
        }
        let text;
        let label = match count_override {
            Some(s) => Some(s),
            None if stack.count > 1 => {
                text = stack.count.to_string();
                Some(text.as_str())
            }
            None => None,
        };
        if let Some(label) = label {
            let w = font.width(label, p.scale);
            let cap = font.cap_height(p.scale);
            font.draw(p, label, x + 17.0 - w, y + 16.0 - cap, WHITE, true, 1.0);
        }
    }
}

/// Green (full) → yellow → red (broken).
pub fn durability_color(frac: f32) -> Color {
    let h = frac.clamp(0.0, 1.0) / 3.0; // hue 0..120°
    let (r, g) = if h < 1.0 / 6.0 {
        (1.0, h * 6.0)
    } else {
        (1.0 - (h - 1.0 / 6.0) * 6.0, 1.0)
    };
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), 0.0, 1.0]
}

/// Scale/crop an icon to 16×16 (nearest neighbour; uses the first frame of
/// vertical animation strips).
fn fit_icon(img: &Rgba8Image) -> Rgba8Image {
    let s = img.width.min(img.height).max(1);
    let frame = if img.height > img.width {
        img.crop(0, 0, s, s)
    } else {
        img.clone()
    };
    if frame.width == ICON && frame.height == ICON {
        frame
    } else {
        mc_assets::resize_nearest(&frame, ICON, ICON)
    }
}

/// Look an item texture up via `textures/item_texture.json`, then directly
/// in `textures/items/`.
fn load_item_texture(
    assets: &Assets,
    item_json: Option<&serde_json::Value>,
    name: &str,
) -> Option<Rgba8Image> {
    if name.is_empty() {
        return None;
    }
    if let Some(json) = item_json {
        let entry = &json["texture_data"][name]["textures"];
        let path = match entry {
            serde_json::Value::String(s) => Some(s.as_str()),
            serde_json::Value::Array(a) => a.first().and_then(|v| v.as_str()),
            _ => None,
        };
        if let Some(img) = path.and_then(|p| assets.pack.load_image(p)) {
            return Some(img);
        }
    }
    assets.pack.load_image(&format!("textures/items/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durability_colors() {
        assert_eq!(durability_color(1.0), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(durability_color(0.0), [1.0, 0.0, 0.0, 1.0]);
        let mid = durability_color(0.5);
        assert!(mid[0] > 0.9 && mid[1] > 0.9);
    }

    #[test]
    fn every_item_has_an_icon() {
        let pack = mc_assets::Pack::find().expect("pack");
        let assets = Assets::load(pack);
        let icons = ItemIcons::build(&assets);
        for item in ItemId::all() {
            let liquid = item
                .block()
                .is_some_and(|b| matches!(b.def().shape, Shape::Liquid | Shape::None));
            assert!(
                icons.has_icon(item) || liquid,
                "no icon for {}",
                item.name()
            );
        }
    }
}
