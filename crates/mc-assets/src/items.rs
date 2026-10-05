//! Item icons: flat icons from `item_texture.json` (non-block items) or the
//! block's carried/world texture (plants, torch, sugar cane...), isometric
//! icons for cube-like block items, and one atlas containing all of them.

use std::sync::Arc;

use mc_core::block::{Layer, Shape};
use mc_core::{BlockId, Face, ItemId, Rgba8Image};
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;

use crate::blocks::{
    AlphaMode, PackIndex, all_tex_paths, face_key, load_terrain_texture, parse_tex_entry,
};
use crate::image_ops::{alpha_over, resize_nearest, resize_tile, tint};
use crate::{BlockTextures, ColorMaps, Pack};

/// Edge length of the precomputed isometric block icons (and of atlas cells).
pub const BLOCK_ICON_SIZE: u32 = 32;
/// Edge length of flat item icons.
pub const FLAT_ICON_SIZE: u32 = 16;

/// The three visible faces of an isometric block icon, already tinted
/// (default plains colours) and with overlays composited.
#[derive(Clone, Debug)]
pub struct IconFaces {
    /// +Y face.
    pub top: Rgba8Image,
    /// +Z (south) face, drawn on the left.
    pub left: Rgba8Image,
    /// +X (east) face, drawn on the right.
    pub right: Rgba8Image,
    /// Box drawn, in block units (0..1): (min, max).
    pub bounds: ([f32; 3], [f32; 3]),
}

/// Item icons, one image per item.
#[derive(Clone, Debug, Default)]
pub struct ItemIcons {
    /// Flat 16×16 icon per item: every non-block item and flat block items
    /// (plants, torch, sugar cane, lily pad, vine...). `None` for cube-like
    /// block items (see `block_icons`) and air.
    pub icons: Vec<Option<Arc<Rgba8Image>>>,
    /// Isometric [`BLOCK_ICON_SIZE`]² icon for cube-like block items
    /// (top face + south face on the left + east face on the right, tinted
    /// with plains colours).
    pub block_icons: Vec<Option<Arc<Rgba8Image>>>,
    /// The faces used for `block_icons`, to re-render at other sizes with
    /// [`ItemIcons::render_block_icon`].
    pub block_faces: Vec<Option<Arc<IconFaces>>>,
    /// Every item's display icon in one image: a grid of `atlas_cell`²
    /// cells (flat icons are scaled 2× with nearest filtering).
    pub atlas: Arc<Rgba8Image>,
    /// Per item: normalised (u0, v0, u1, v1) of its cell in `atlas`.
    pub atlas_uv: Vec<Option<[f32; 4]>>,
    pub atlas_cell: u32,
    /// Items whose icon is a generated placeholder (texture not found).
    pub placeholders: Vec<String>,
}

impl ItemIcons {
    /// Flat icon (non-block items and flat block items).
    pub fn get(&self, item: ItemId) -> Option<&Arc<Rgba8Image>> {
        self.icons.get(item.0 as usize).and_then(|i| i.as_ref())
    }
    /// Precomputed isometric icon for cube-like block items.
    pub fn block_icon(&self, item: ItemId) -> Option<&Arc<Rgba8Image>> {
        self.block_icons
            .get(item.0 as usize)
            .and_then(|i| i.as_ref())
    }
    /// Whatever should be shown in a slot: the flat icon, else the isometric one.
    pub fn display_icon(&self, item: ItemId) -> Option<&Arc<Rgba8Image>> {
        self.get(item).or_else(|| self.block_icon(item))
    }
    /// Normalised atlas rectangle (u0, v0, u1, v1) of the item's display icon.
    pub fn atlas_uv(&self, item: ItemId) -> Option<[f32; 4]> {
        self.atlas_uv.get(item.0 as usize).copied().flatten()
    }
    /// Render the isometric icon of a cube-like block item at any size.
    pub fn render_block_icon(&self, item: ItemId, size: u32) -> Option<Rgba8Image> {
        let faces = self.block_faces.get(item.0 as usize)?.as_ref()?;
        Some(render_iso(faces, size))
    }

    /// Build all icons (normally done by `Assets::load`).
    pub fn load(pack: &Pack, blocks: &BlockTextures, colormaps: &ColorMaps) -> ItemIcons {
        let index = PackIndex::load(pack);
        Self::load_with(pack, &index, blocks, colormaps)
    }

    pub(crate) fn load_with(
        pack: &Pack,
        index: &PackIndex,
        blocks: &BlockTextures,
        colormaps: &ColorMaps,
    ) -> ItemIcons {
        let n = ItemId::count();
        let mut out = ItemIcons {
            icons: vec![None; n],
            block_icons: vec![None; n],
            block_faces: vec![None; n],
            atlas_cell: BLOCK_ICON_SIZE,
            ..Default::default()
        };
        let basenames = item_basenames(index);

        for b in BlockId::all().skip(1) {
            let i = b.0 as usize;
            if is_flat_block_item(b) {
                out.icons[i] = Some(Arc::new(flat_block_icon(pack, index, blocks, colormaps, b)));
            } else {
                let faces = iso_faces(pack, index, blocks, colormaps, b);
                out.block_icons[i] = Some(Arc::new(render_iso(&faces, BLOCK_ICON_SIZE)));
                out.block_faces[i] = Some(Arc::new(faces));
            }
        }
        for (k, def) in mc_core::item::ITEM_DEFS.iter().enumerate() {
            let i = BlockId::count() + k;
            let icon = resolve_item_texture(pack, index, &basenames, def.texture, def.name)
                .and_then(|p| pack.load_image_ext(&p, true))
                .map(|(img, _)| {
                    let s = img.width.min(img.height).max(1);
                    resize_tile(&img.crop(0, 0, s, s), FLAT_ICON_SIZE)
                });
            let icon = icon.unwrap_or_else(|| {
                out.placeholders.push(format!(
                    "item '{}': texture '{}' not found in item_texture.json",
                    def.name, def.texture
                ));
                placeholder_icon()
            });
            out.icons[i] = Some(Arc::new(icon));
        }
        out.build_atlas();
        out
    }

    fn build_atlas(&mut self) {
        let cell = self.atlas_cell;
        let n = self.icons.len();
        let cols = 32u32;
        let entries: Vec<usize> = (0..n)
            .filter(|&i| self.display_icon(ItemId(i as u16)).is_some())
            .collect();
        let rows = (entries.len() as u32).div_ceil(cols).max(1);
        let mut atlas = Rgba8Image::new(cols * cell, rows * cell);
        self.atlas_uv = vec![None; n];
        for (slot, &i) in entries.iter().enumerate() {
            let icon = self.display_icon(ItemId(i as u16)).unwrap();
            let img = if icon.width == cell && icon.height == cell {
                (**icon).clone()
            } else {
                resize_nearest(icon, cell, cell)
            };
            let (x, y) = ((slot as u32 % cols) * cell, (slot as u32 / cols) * cell);
            atlas.blit(&img, x, y);
            let (w, h) = (atlas.width as f32, atlas.height as f32);
            self.atlas_uv[i] = Some([
                x as f32 / w,
                y as f32 / h,
                (x + cell) as f32 / w,
                (y + cell) as f32 / h,
            ]);
        }
        self.atlas = Arc::new(atlas);
    }
}

/// Block items drawn as a flat sprite rather than a cube.
pub fn is_flat_block_item(b: BlockId) -> bool {
    let d = b.def();
    matches!(d.shape, Shape::Cross | Shape::Torch)
        || (matches!(d.shape, Shape::Layer(_)) && d.layer == Layer::Cutout)
}

/// Texture basename → path over every variant in `item_texture.json`.
fn item_basenames(index: &PackIndex) -> HashMap<String, String> {
    let mut map = HashMap::default();
    if let Value::Object(m) = &index.items {
        for v in m.values() {
            let mut paths = Vec::new();
            all_tex_paths(&v["textures"], &mut paths);
            for p in paths {
                if let Some(base) = p.rsplit('/').next() {
                    map.entry(base.to_string()).or_insert(p.clone());
                }
            }
        }
    }
    map
}

/// Resolve an `ItemDef::texture` key to a pack image path. Accepts an
/// `item_texture.json` key, the basename of any texture listed there (tool
/// variants such as `wood_pickaxe` live in the `pickaxe` array), common
/// Java→Bedrock renames (`wooden_` → `wood_`, `golden_` → `gold_`), the item
/// name itself, or a file under `textures/items/`.
pub(crate) fn resolve_item_texture(
    pack: &Pack,
    index: &PackIndex,
    basenames: &HashMap<String, String>,
    key: &str,
    name: &str,
) -> Option<String> {
    let mut cands: Vec<String> = Vec::new();
    for k in [key, name] {
        for c in [
            k.to_string(),
            k.replace("wooden_", "wood_").replace("golden_", "gold_"),
        ] {
            if !c.is_empty() && !cands.contains(&c) {
                cands.push(c);
            }
        }
    }
    if key.starts_with("textures/") {
        return Some(key.to_string());
    }
    for c in &cands {
        if let Some(e) = parse_tex_entry(&index.items[c.as_str()]["textures"]) {
            return Some(e.path);
        }
    }
    for c in &cands {
        if let Some(p) = basenames.get(c) {
            return Some(p.clone());
        }
    }
    for c in &cands {
        let p = format!("textures/items/{c}");
        if pack.exists(&format!("{p}.png")) || pack.exists(&format!("{p}.tga")) {
            return Some(p);
        }
    }
    None
}

/// Generic placeholder: grey square with a dark border and a diagonal cross.
fn placeholder_icon() -> Rgba8Image {
    let s = FLAT_ICON_SIZE;
    let mut img = Rgba8Image::new(s, s);
    for y in 2..s - 2 {
        for x in 2..s - 2 {
            let edge = x == 2 || y == 2 || x == s - 3 || y == s - 3;
            let diag = x == y || x + y == s - 1;
            img.put(
                x,
                y,
                if edge || diag {
                    [60, 60, 60, 255]
                } else {
                    [170, 170, 170, 255]
                },
            );
        }
    }
    img
}

fn block_entry<'a>(index: &'a PackIndex, b: BlockId) -> &'a Value {
    &index.blocks[b.def().pack_name]
}

/// World tile of a face with the default tint and overlay applied.
fn tinted_world_face(
    blocks: &BlockTextures,
    colormaps: &ColorMaps,
    b: BlockId,
    f: Face,
) -> Rgba8Image {
    let mut img = blocks.tile(blocks.face(b, f)).clone();
    let t = colormaps.default_tint(b);
    if blocks.is_tinted(b, f) {
        tint(&mut img, t);
    }
    if let Some(ov) = blocks.overlay(b, f) {
        let mut o = blocks.tile(ov).clone();
        tint(&mut o, t);
        alpha_over(&mut img, &o);
    }
    img
}

/// Face image for inventory display: the pack's "carried" texture when the
/// block has one (pre-coloured grass/leaves), else the tinted world texture.
fn carried_face(
    pack: &Pack,
    index: &PackIndex,
    blocks: &BlockTextures,
    colormaps: &ColorMaps,
    b: BlockId,
    f: Face,
) -> Rgba8Image {
    let mode = match b.def().layer {
        Layer::Opaque => AlphaMode::Opaque,
        _ => AlphaMode::Keep,
    };
    let carried = &block_entry(index, b)["carried_textures"];
    if let Some(key) = face_key(carried, f) {
        if let Ok(t) = load_terrain_texture(pack, index, key, 0, mode, None) {
            let mut img = t.base;
            if let Some(c) = t.entry.tint_color {
                tint(&mut img, c);
            }
            return img;
        }
    }
    tinted_world_face(blocks, colormaps, b, f)
}

fn flat_block_icon(
    pack: &Pack,
    index: &PackIndex,
    blocks: &BlockTextures,
    colormaps: &ColorMaps,
    b: BlockId,
) -> Rgba8Image {
    // A dedicated item texture (e.g. sugar cane's "reeds").
    if let Some(e) = parse_tex_entry(&index.items[b.def().pack_name]["textures"]) {
        if let Some((img, _)) = pack.load_image_ext(&e.path, true) {
            let s = img.width.min(img.height).max(1);
            return resize_tile(&img.crop(0, 0, s, s), FLAT_ICON_SIZE);
        }
    }
    let img = carried_face(pack, index, blocks, colormaps, b, Face::North);
    resize_tile(&img, FLAT_ICON_SIZE)
}

fn iso_faces(
    pack: &Pack,
    index: &PackIndex,
    blocks: &BlockTextures,
    colormaps: &ColorMaps,
    b: BlockId,
) -> IconFaces {
    let d = b.def();
    let bounds = match d.shape {
        Shape::Inset => (
            [1.0 / 16.0, 0.0, 1.0 / 16.0],
            [15.0 / 16.0, 1.0, 15.0 / 16.0],
        ),
        Shape::Layer(h) => ([0.0; 3], [1.0, (h.max(1) as f32 / 16.0).min(1.0), 1.0]),
        _ => ([0.0; 3], [1.0; 3]),
    };
    IconFaces {
        top: carried_face(pack, index, blocks, colormaps, b, Face::Up),
        left: carried_face(pack, index, blocks, colormaps, b, Face::South),
        right: carried_face(pack, index, blocks, colormaps, b, Face::East),
        bounds,
    }
}

/// Render an isometric block icon (Minecraft inventory style: 30° tilt,
/// 45° turn) with 4×4 supersampled edges and nearest texel sampling.
pub fn render_iso(faces: &IconFaces, size: u32) -> Rgba8Image {
    use glam::{Vec2, Vec3};
    let k = 2f32.sqrt() * 30f32.to_radians().tan();
    let fwd = -Vec3::new(1.0, k, 1.0).normalize();
    let right = fwd.cross(Vec3::Y).normalize();
    let up = right.cross(fwd).normalize();
    let scale = size as f32 * 0.625;
    let c = Vec3::splat(0.5);
    let half = size as f32 * 0.5;
    let proj = |p: Vec3| {
        let q = p - c;
        Vec2::new(half + q.dot(right) * scale, half - q.dot(up) * scale)
    };
    let (mn, mx) = (Vec3::from(faces.bounds.0), Vec3::from(faces.bounds.1));
    // (image, origin, U axis, V axis, texture sub-rect u0 v0 u1 v1, shade)
    let quads: [(&Rgba8Image, Vec3, Vec3, Vec3, [f32; 4], f32); 3] = [
        (
            &faces.top,
            Vec3::new(mn.x, mx.y, mn.z),
            Vec3::X * (mx.x - mn.x),
            Vec3::Z * (mx.z - mn.z),
            [mn.x, mn.z, mx.x, mx.z],
            1.0,
        ),
        (
            &faces.left,
            Vec3::new(mn.x, mx.y, mx.z),
            Vec3::X * (mx.x - mn.x),
            Vec3::NEG_Y * (mx.y - mn.y),
            [mn.x, 1.0 - mx.y, mx.x, 1.0 - mn.y],
            0.8,
        ),
        (
            &faces.right,
            Vec3::new(mx.x, mx.y, mx.z),
            Vec3::NEG_Z * (mx.z - mn.z),
            Vec3::NEG_Y * (mx.y - mn.y),
            [1.0 - mx.z, 1.0 - mx.y, 1.0 - mn.z, 1.0 - mn.y],
            0.6,
        ),
    ];
    let quads2: Vec<_> = quads
        .iter()
        .map(|(img, o, u, v, r, s)| {
            let o2 = proj(*o);
            let u2 = proj(*o + *u) - o2;
            let v2 = proj(*o + *v) - o2;
            (*img, o2, u2, v2, *r, *s, u2.perp_dot(v2))
        })
        .collect();
    const SS: u32 = 4;
    let mut out = Rgba8Image::new(size, size);
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut hits) = (0f32, 0f32, 0f32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let q = Vec2::new(
                        px as f32 + (sx as f32 + 0.5) / SS as f32,
                        py as f32 + (sy as f32 + 0.5) / SS as f32,
                    );
                    for (img, o2, u2, v2, rect, shade, det) in &quads2 {
                        if det.abs() < 1e-6 {
                            continue;
                        }
                        let d = q - *o2;
                        let s = d.perp_dot(*v2) / det;
                        let t = u2.perp_dot(d) / det;
                        if !(0.0..1.0).contains(&s) || !(0.0..1.0).contains(&t) {
                            continue;
                        }
                        let tu = rect[0] + s * (rect[2] - rect[0]);
                        let tv = rect[1] + t * (rect[3] - rect[1]);
                        let x = ((tu * img.width as f32) as u32).min(img.width - 1);
                        let y = ((tv * img.height as f32) as u32).min(img.height - 1);
                        let p = img.get(x, y);
                        if p[3] < 128 {
                            continue;
                        }
                        r += p[0] as f32 * shade;
                        g += p[1] as f32 * shade;
                        b += p[2] as f32 * shade;
                        hits += 1;
                        break;
                    }
                }
            }
            if hits > 0 {
                let h = hits as f32;
                out.put(
                    px,
                    py,
                    [
                        (r / h).round() as u8,
                        (g / h).round() as u8,
                        (b / h).round() as u8,
                        (255.0 * h / (SS * SS) as f32).round() as u8,
                    ],
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::blocks;

    fn icons() -> ItemIcons {
        let pack = Pack::find().expect("pack");
        let cm = ColorMaps::load(&pack);
        let bt = crate::load_block_textures(&pack);
        ItemIcons::load(&pack, &bt, &cm)
    }

    #[test]
    fn every_item_has_an_icon() {
        let ic = icons();
        assert!(ic.placeholders.is_empty(), "{:?}", ic.placeholders);
        for item in ItemId::all() {
            let icon = ic
                .display_icon(item)
                .unwrap_or_else(|| panic!("no icon for {}", item.name()));
            assert!(
                icon.data.chunks_exact(4).any(|p| p[3] > 0),
                "empty icon for {}",
                item.name()
            );
            assert!(ic.atlas_uv(item).is_some());
        }
        // Cube blocks get isometric icons, plants flat ones.
        let stone = ItemId::from_block(blocks::STONE);
        assert!(ic.get(stone).is_none());
        assert_eq!(ic.block_icon(stone).unwrap().width, BLOCK_ICON_SIZE);
        let poppy = ItemId::from_block(blocks::POPPY);
        assert_eq!(ic.get(poppy).unwrap().width, FLAT_ICON_SIZE);
        assert!(ic.get(ItemId::from_block(blocks::TORCH)).is_some());
        assert!(ic.render_block_icon(stone, 48).is_some());
    }

    #[test]
    fn iso_icon_has_shaded_sides() {
        let ic = icons();
        let icon = ic.block_icon(ItemId::from_block(blocks::DIRT)).unwrap();
        let s = BLOCK_ICON_SIZE;
        // Corners stay transparent, the centre column is covered.
        assert_eq!(icon.get(0, 0)[3], 0);
        assert_eq!(icon.get(s - 1, 0)[3], 0);
        assert_eq!(icon.get(s / 2, s / 2)[3], 255);
        // Left side (lit 0.8) brighter than the right side (0.6).
        let l = icon.get(s / 4, s * 3 / 4);
        let r = icon.get(s * 3 / 4, s * 3 / 4);
        let lum = |p: [u8; 4]| p[0] as u32 + p[1] as u32 + p[2] as u32;
        assert!(lum(l) > lum(r), "{l:?} vs {r:?}");
    }
}
