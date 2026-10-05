//! Block face textures: `blocks.json` → `textures/terrain_texture.json` →
//! image, flipbook animations, grass-side overlays and destroy stages.

use mc_core::block::{Layer, Tint};
use mc_core::{BlockId, Face, Rgba8Image, blocks};
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;

use crate::image_ops::{
    dilate_transparent, is_greyscale, magenta_checker, resize_tile, set_opaque,
};
use crate::{ColorMaps, Pack};

/// Index of a 16×16 tile in [`BlockTextures::tiles`].
pub type TileId = u16;

/// Edge length of every block tile.
pub const TILE_SIZE: u32 = 16;

/// All block face textures as equally sized tiles (suitable for a 2D texture
/// array, which avoids atlas bleeding and lets greedy meshes repeat UVs).
#[derive(Clone, Debug, Default)]
pub struct BlockTextures {
    /// Tile edge length in pixels (always 16; larger pack textures are downscaled).
    pub tile_size: u32,
    /// Tile images, all `tile_size` × `tile_size`. Tile 0 is the "missing" texture.
    pub tiles: Vec<Rgba8Image>,
    /// Per block, per face (`Face::index()` order) tile index.
    pub faces: Vec<[TileId; 6]>,
    /// Per block, per face: multiply this face by the block's biome tint.
    pub tinted: Vec<[bool; 6]>,
    /// Optional overlay tile drawn on top of a face, always tinted (grass block sides).
    pub overlay: Vec<[Option<TileId>; 6]>,
    /// Tile name → index (texture key from `terrain_texture.json`).
    pub by_name: HashMap<String, TileId>,
    /// Animated tiles. `frames` is the full playback sequence (`frames[0] == tile`);
    /// the extra frames are additional tiles stored after `static_tile_count`.
    pub animations: Vec<TileAnimation>,
    /// Tiles `0..static_tile_count` are everything faces, overlays and
    /// destroy stages reference; the rest are animation frames only. A
    /// renderer short on texture-array layers can upload just the static
    /// tiles and animate by rewriting layer `tile` with `tiles[frame]`.
    pub static_tile_count: usize,
    /// Block-breaking crack overlay tiles, stage 0..10 (`textures/environment/destroy_stage_N`).
    pub destroy_stages: Vec<TileId>,
    /// Per block, per face: the pack marks this face "isotropic" (its texture
    /// may be randomly rotated per block to hide tiling: grass top, sand...).
    pub isotropic: Vec<[bool; 6]>,
    /// Number of (non-air) block faces that fell back to the missing tile.
    pub missing_faces: usize,
    /// Human readable problems found while loading.
    pub failures: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TileAnimation {
    pub tile: TileId,
    pub frames: Vec<TileId>,
    pub ticks_per_frame: u32,
}

impl BlockTextures {
    pub fn face(&self, block: BlockId, face: Face) -> TileId {
        self.faces
            .get(block.0 as usize)
            .map(|f| f[face.index()])
            .unwrap_or(0)
    }
    pub fn is_tinted(&self, block: BlockId, face: Face) -> bool {
        self.tinted
            .get(block.0 as usize)
            .is_some_and(|f| f[face.index()])
    }
    pub fn overlay(&self, block: BlockId, face: Face) -> Option<TileId> {
        self.overlay
            .get(block.0 as usize)
            .and_then(|f| f[face.index()])
    }
    pub fn tile(&self, id: TileId) -> &Rgba8Image {
        self.tiles.get(id as usize).unwrap_or(&self.tiles[0])
    }
    /// The tile to display for `tile` at game tick `tick` (animations resolved).
    pub fn animated_tile(&self, tile: TileId, tick: u64) -> TileId {
        for a in &self.animations {
            if a.tile == tile && !a.frames.is_empty() {
                let i = (tick / a.ticks_per_frame.max(1) as u64) as usize % a.frames.len();
                return a.frames[i];
            }
        }
        tile
    }
    /// Tile name for a tile id (reverse of `by_name`, for debugging).
    pub fn tile_name(&self, id: TileId) -> Option<&str> {
        self.by_name
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.as_str())
    }
}

/// Parsed pack JSON indexes shared by block and item loading.
pub(crate) struct PackIndex {
    /// `blocks.json`
    pub blocks: Value,
    /// `terrain_texture.json` → `texture_data`
    pub terrain: Value,
    /// `item_texture.json` → `texture_data`
    pub items: Value,
    /// Flipbook entries by `atlas_tile`.
    pub flipbooks: HashMap<String, Flipbook>,
}

#[derive(Clone, Debug)]
pub(crate) struct Flipbook {
    pub texture: String,
    pub ticks_per_frame: u32,
    pub frames: Option<Vec<u32>>,
    pub replicate: u32,
}

impl PackIndex {
    pub fn load(pack: &Pack) -> PackIndex {
        let blocks = pack.load_json("blocks.json").unwrap_or_default();
        let terrain = pack
            .load_json("textures/terrain_texture.json")
            .map(|v| v["texture_data"].clone())
            .unwrap_or_default();
        let items = pack
            .load_json("textures/item_texture.json")
            .map(|v| v["texture_data"].clone())
            .unwrap_or_default();
        let mut flipbooks = HashMap::default();
        if let Some(Value::Array(list)) = pack.load_json("textures/flipbook_textures.json") {
            for e in list {
                let (Some(tile), Some(tex)) =
                    (e["atlas_tile"].as_str(), e["flipbook_texture"].as_str())
                else {
                    continue;
                };
                let frames = e["frames"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|f| f.as_u64().map(|f| f as u32))
                        .collect::<Vec<_>>()
                });
                flipbooks.entry(tile.to_string()).or_insert(Flipbook {
                    texture: tex.to_string(),
                    ticks_per_frame: e["ticks_per_frame"].as_u64().unwrap_or(1).max(1) as u32,
                    frames: frames.filter(|f| !f.is_empty()),
                    replicate: e["replicate"].as_u64().unwrap_or(1).max(1) as u32,
                });
            }
        }
        PackIndex {
            blocks,
            terrain,
            items,
            flipbooks,
        }
    }
}

/// One entry of a `textures` value in `terrain_texture.json` / `item_texture.json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TexEntry {
    pub path: String,
    /// Colour applied to the alpha-masked part of the texture (grass sides).
    pub overlay_color: Option<u32>,
    /// Fixed tint for the whole texture (lily pad).
    pub tint_color: Option<u32>,
}

pub(crate) fn parse_hex_color(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(v),
        8 => Some(v & 0xFFFFFF),
        _ => None,
    }
}

/// Parse a `textures` value: a path string, a `{path, overlay_color,
/// tint_color}` object, or an array of variants (the first is used).
pub(crate) fn parse_tex_entry(v: &Value) -> Option<TexEntry> {
    parse_tex_entry_variant(v, 0)
}

/// Like [`parse_tex_entry`] but picks variant `variant` of an array
/// (clamped to the last one).
pub(crate) fn parse_tex_entry_variant(v: &Value, variant: usize) -> Option<TexEntry> {
    if let Value::Array(a) = v {
        return a
            .get(variant)
            .or_else(|| a.last())
            .and_then(|e| parse_tex_entry_variant(e, 0));
    }
    match v {
        Value::String(s) => Some(TexEntry {
            path: s.clone(),
            overlay_color: None,
            tint_color: None,
        }),
        Value::Object(m) => Some(TexEntry {
            path: m.get("path")?.as_str()?.to_string(),
            overlay_color: m
                .get("overlay_color")
                .and_then(Value::as_str)
                .and_then(parse_hex_color),
            tint_color: m
                .get("tint_color")
                .and_then(Value::as_str)
                .and_then(parse_hex_color),
        }),
        _ => None,
    }
}

/// Which variant of a texture array a block uses. The pack gives carved,
/// lit and plain pumpkins the same keys; the game picks the array entry by
/// block type (plain pumpkins use the uncarved side texture as their face).
/// Huge mushroom blocks index their arrays by the vanilla "huge mushroom
/// bits" state; variant 14 is skin on every face, which is what worldgen places.
pub(crate) fn texture_variant(pack_name: &str) -> usize {
    match pack_name {
        "lit_pumpkin" => 1,
        "pumpkin" => 2,
        "brown_mushroom_block" | "red_mushroom_block" => 14,
        _ => 0,
    }
}

/// All texture paths mentioned by a `textures` value (every variant).
pub(crate) fn all_tex_paths(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Object(m) => {
            if let Some(p) = m.get("path").and_then(Value::as_str) {
                out.push(p.to_string());
            }
        }
        Value::Array(a) => a.iter().for_each(|e| all_tex_paths(e, out)),
        _ => {}
    }
}

/// How a texture's alpha channel is interpreted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum AlphaMode {
    /// Opaque block: alpha is ignored (or used as a tint mask when the entry
    /// has an `overlay_color`).
    Opaque,
    /// Cutout / translucent block: alpha is transparency.
    Keep,
}

/// A texture key resolved to tile-ready images.
#[derive(Clone, Debug)]
pub(crate) struct LoadedTex {
    pub base: Rgba8Image,
    /// Tint-masked part as a separate layer (grass block sides).
    pub overlay: Option<Rgba8Image>,
    /// Animation frames (frames[0] == base) and ticks per frame.
    pub anim: Option<(Vec<Rgba8Image>, u32)>,
    pub entry: TexEntry,
}

/// Turn a decoded frame into a 16×16 tile according to `mode`.
fn finish_tile(img: &Rgba8Image, mode: AlphaMode) -> Rgba8Image {
    let mut t = resize_tile(img, TILE_SIZE);
    match mode {
        AlphaMode::Opaque => set_opaque(&mut t),
        AlphaMode::Keep => dilate_transparent(&mut t),
    }
    t
}

/// Resolve a terrain texture key into tiles. `mask_tint` colours the masked
/// part of overlay textures in the base tile (the overlay itself stays grey);
/// when `None` the entry's own `overlay_color` is used.
pub(crate) fn load_terrain_texture(
    pack: &Pack,
    index: &PackIndex,
    key: &str,
    variant: usize,
    mode: AlphaMode,
    mask_tint: Option<u32>,
) -> Result<LoadedTex, String> {
    let entry = parse_tex_entry_variant(&index.terrain[key]["textures"], variant)
        .ok_or_else(|| format!("terrain texture key '{key}' not found"))?;

    if let Some(fb) = index.flipbooks.get(key) {
        if let Some((img, _)) = pack.load_image_ext(&fb.texture, true) {
            let fw = img.width.max(1);
            let count = (img.height / fw).max(1);
            let crop = (fw / fb.replicate).max(1);
            let order: Vec<u32> = fb
                .frames
                .clone()
                .unwrap_or_else(|| (0..count).collect())
                .into_iter()
                .filter(|&f| f < count)
                .collect();
            if !order.is_empty() {
                let frames: Vec<Rgba8Image> = order
                    .iter()
                    .map(|&f| finish_tile(&img.crop(0, f * fw, crop, crop), mode))
                    .collect();
                return Ok(LoadedTex {
                    base: frames[0].clone(),
                    overlay: None,
                    anim: (frames.len() > 1).then(|| (frames, fb.ticks_per_frame)),
                    entry,
                });
            }
        }
    }

    let (img, _) = pack
        .load_image_ext(&entry.path, true)
        .ok_or_else(|| format!("image '{}' (key '{key}') not found", entry.path))?;
    // Animated strips without a flipbook entry: use the first square frame.
    let s = img.width.min(img.height).max(1);
    let frame = img.crop(0, 0, s, s);
    let frame = resize_tile(&frame, TILE_SIZE);

    let has_clear = frame.data.chunks_exact(4).any(|p| p[3] == 0);
    let has_mask = frame.data.chunks_exact(4).any(|p| p[3] > 0);
    if mode == AlphaMode::Opaque && entry.overlay_color.is_some() && has_clear && has_mask {
        // Bedrock tint mask: alpha 0 = untinted base colour, alpha > 0 = the
        // greyscale part that is multiplied by the biome colour.
        let tint = mask_tint.or(entry.overlay_color).unwrap_or(0xFFFFFF);
        let t = [(tint >> 16) & 0xFF, (tint >> 8) & 0xFF, tint & 0xFF];
        let mut base = frame.clone();
        let mut overlay = frame.clone();
        for (b, o) in base
            .data
            .chunks_exact_mut(4)
            .zip(overlay.data.chunks_exact_mut(4))
        {
            let a = b[3] as u32;
            for c in 0..3 {
                let tinted = b[c] as u32 * t[c] / 255;
                b[c] = ((b[c] as u32 * (255 - a) + tinted * a) / 255) as u8;
            }
            b[3] = 255;
            o[3] = a as u8;
        }
        dilate_transparent(&mut overlay);
        return Ok(LoadedTex {
            base,
            overlay: Some(overlay),
            anim: None,
            entry,
        });
    }
    Ok(LoadedTex {
        base: finish_tile(&frame, mode),
        overlay: None,
        anim: None,
        entry,
    })
}

/// The terrain key a `blocks.json` `textures` value assigns to `face`.
pub(crate) fn face_key(textures: &Value, face: Face) -> Option<&str> {
    match textures {
        Value::String(s) => Some(s.as_str()),
        Value::Object(m) => {
            let side = matches!(face, Face::North | Face::South | Face::East | Face::West);
            m.get(face.pack_key())
                .or_else(|| side.then(|| m.get("side")).flatten())
                .or_else(|| m.get("side"))
                .or_else(|| m.get("up"))
                .or_else(|| m.values().next())
                .and_then(Value::as_str)
        }
        _ => None,
    }
}

fn isotropic_flags(v: &Value) -> [bool; 6] {
    match v {
        Value::Bool(b) => [*b; 6],
        Value::Object(m) => {
            let mut out = [false; 6];
            for f in Face::ALL {
                out[f.index()] = m
                    .get(f.pack_key())
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
            out
        }
        _ => [false; 6],
    }
}

struct Builder<'a> {
    pack: &'a Pack,
    index: &'a PackIndex,
    bt: BlockTextures,
    cache: HashMap<(String, usize, AlphaMode), (TileId, Option<TileId>, bool)>,
    path_cache: HashMap<String, (TileId, Option<TileId>, bool)>,
    default_grass: u32,
    /// Animation frames (after the first) waiting to be appended at the end.
    pending_frames: Vec<(TileId, Vec<Rgba8Image>, u32)>,
}

impl Builder<'_> {
    fn push(&mut self, img: Rgba8Image) -> TileId {
        self.bt.tiles.push(img);
        (self.bt.tiles.len() - 1) as TileId
    }

    /// (tile, overlay tile, greyscale) for a terrain key.
    fn tile(
        &mut self,
        key: &str,
        variant: usize,
        mode: AlphaMode,
    ) -> (TileId, Option<TileId>, bool) {
        if let Some(&r) = self.cache.get(&(key.to_string(), variant, mode)) {
            return r;
        }
        // Many keys share one image (grass_bottom / flattened_dirt / ...):
        // reuse the tile when path, colours and mode match and it isn't animated.
        let entry = parse_tex_entry_variant(&self.index.terrain[key]["textures"], variant);
        let path_key = entry
            .as_ref()
            .filter(|_| !self.index.flipbooks.contains_key(key))
            .map(|e| {
                format!(
                    "{}|{:?}|{:?}|{:?}",
                    e.path, e.overlay_color, e.tint_color, mode
                )
            });
        if let Some(pk) = &path_key {
            if let Some(&r) = self.path_cache.get(pk) {
                self.bt.by_name.entry(key.to_string()).or_insert(r.0);
                self.cache.insert((key.to_string(), variant, mode), r);
                return r;
            }
        }
        let r = match load_terrain_texture(
            self.pack,
            self.index,
            key,
            variant,
            mode,
            Some(self.default_grass),
        ) {
            Ok(t) => {
                let grey = is_greyscale(&t.base);
                let id = self.push(t.base);
                let ov = t.overlay.map(|o| self.push(o));
                if let Some((frames, ticks)) = t.anim {
                    self.pending_frames
                        .push((id, frames.into_iter().skip(1).collect(), ticks));
                }
                self.bt.by_name.entry(key.to_string()).or_insert(id);
                (id, ov, grey)
            }
            Err(e) => {
                self.bt.failures.push(e);
                (0, None, false)
            }
        };
        if let (Some(pk), true) = (path_key, r.0 != 0) {
            self.path_cache.insert(pk, r);
        }
        self.cache.insert((key.to_string(), variant, mode), r);
        r
    }
}

/// Block texture loader: resolves `blocks.json` → `terrain_texture.json` →
/// image for every registered block, with overlays, animations and tint flags.
pub fn load_block_textures(pack: &Pack) -> BlockTextures {
    let index = PackIndex::load(pack);
    let colormaps = ColorMaps::load(pack);
    load_block_textures_with(pack, &index, &colormaps)
}

pub(crate) fn load_block_textures_with(
    pack: &Pack,
    index: &PackIndex,
    colormaps: &ColorMaps,
) -> BlockTextures {
    let mut b = Builder {
        pack,
        index,
        bt: BlockTextures {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
        cache: HashMap::default(),
        path_cache: HashMap::default(),
        default_grass: colormaps.default_tint(blocks::GRASS_BLOCK),
        pending_frames: Vec::new(),
    };
    b.push(magenta_checker(TILE_SIZE));
    b.bt.by_name.insert("missing".into(), 0);

    for block in BlockId::all() {
        let def = block.def();
        let mut faces = [0; 6];
        let mut tinted = [false; 6];
        let mut overlay = [None; 6];
        let mut iso = [false; 6];
        if !block.is_air() {
            let entry = &index.blocks[def.pack_name];
            if entry.is_null() {
                b.bt.failures.push(format!(
                    "block '{}': pack name '{}' not in blocks.json",
                    def.name, def.pack_name
                ));
            }
            iso = isotropic_flags(&entry["isotropic"]);
            let mode = match def.layer {
                Layer::Opaque => AlphaMode::Opaque,
                Layer::Cutout | Layer::Translucent => AlphaMode::Keep,
            };
            for f in Face::ALL {
                let Some(key) = face_key(&entry["textures"], f) else {
                    continue;
                };
                let (tile, ov, grey) = b.tile(key, texture_variant(def.pack_name), mode);
                faces[f.index()] = tile;
                overlay[f.index()] = ov;
                tinted[f.index()] = def.tint != Tint::None && ov.is_none() && grey;
            }
            for f in Face::ALL {
                if faces[f.index()] == 0 {
                    b.bt.missing_faces += 1;
                    b.bt.failures.push(format!(
                        "block '{}' face {:?} uses the missing texture",
                        def.name, f
                    ));
                }
            }
        }
        b.bt.faces.push(faces);
        b.bt.tinted.push(tinted);
        b.bt.overlay.push(overlay);
        b.bt.isotropic.push(iso);
    }

    for n in 0..10 {
        let tile =
            match pack.load_image_ext(&format!("textures/environment/destroy_stage_{n}"), true) {
                Some((img, _)) => {
                    let s = img.width.min(img.height).max(1);
                    let mut t = resize_tile(&img.crop(0, 0, s, s), TILE_SIZE);
                    dilate_transparent(&mut t);
                    t
                }
                None => {
                    b.bt.failures
                        .push(format!("destroy_stage_{n} not found, using a placeholder"));
                    placeholder_crack(n)
                }
            };
        let id = b.push(tile);
        b.bt.destroy_stages.push(id);
    }

    b.bt.static_tile_count = b.bt.tiles.len();
    for (tile, frames, ticks) in std::mem::take(&mut b.pending_frames) {
        let mut ids = vec![tile];
        for f in frames {
            ids.push(b.push(f));
        }
        b.bt.animations.push(TileAnimation {
            tile,
            frames: ids,
            ticks_per_frame: ticks,
        });
    }
    b.bt
}

/// Generic placeholder crack overlay: dark speckles that get denser per stage.
fn placeholder_crack(stage: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(TILE_SIZE, TILE_SIZE);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let h = (x * 73 + y * 151 + x * y * 7) % 10;
            if h <= stage {
                img.put(x, y, [20, 20, 20, 160]);
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Pack {
        Pack::find().expect("resource pack not found")
    }

    #[test]
    fn tex_entry_forms() {
        let v: Value = serde_json::json!("textures/blocks/stone");
        assert_eq!(parse_tex_entry(&v).unwrap().path, "textures/blocks/stone");
        let v: Value = serde_json::json!([{"path": "a", "overlay_color": "#79c05a"}, "b"]);
        let e = parse_tex_entry(&v).unwrap();
        assert_eq!(e.path, "a");
        assert_eq!(e.overlay_color, Some(0x79c05a));
        let v: Value = serde_json::json!({"path": "c", "tint_color": "#208030"});
        assert_eq!(parse_tex_entry(&v).unwrap().tint_color, Some(0x208030));
    }

    #[test]
    fn every_block_has_real_textures() {
        let bt = load_block_textures(&pack());
        assert_eq!(bt.faces.len(), BlockId::count());
        for b in BlockId::all().skip(1) {
            for f in Face::ALL {
                assert_ne!(
                    bt.face(b, f),
                    0,
                    "block {} face {:?} resolves to the missing texture",
                    b.def().name,
                    f
                );
            }
        }
        assert_eq!(bt.missing_faces, 0, "{:?}", bt.failures);
        for t in &bt.tiles {
            assert_eq!((t.width, t.height), (16, 16));
        }
    }

    #[test]
    fn grass_block_faces() {
        let bt = load_block_textures(&pack());
        let g = blocks::GRASS_BLOCK;
        assert!(bt.is_tinted(g, Face::Up));
        assert!(!bt.is_tinted(g, Face::Down));
        for f in [Face::North, Face::South, Face::East, Face::West] {
            assert!(!bt.is_tinted(g, f));
            let ov = bt.overlay(g, f).expect("grass side overlay");
            let img = bt.tile(ov);
            // The overlay covers the top fringe and leaves the dirt part clear.
            assert!(img.get(8, 0)[3] > 0 || img.get(0, 0)[3] > 0);
            assert_eq!(img.get(8, 15)[3], 0);
            // The base side is opaque (dirt below the fringe).
            assert!(
                bt.tile(bt.face(g, f))
                    .data
                    .chunks_exact(4)
                    .all(|p| p[3] == 255)
            );
        }
        assert_eq!(bt.face(g, Face::Down), bt.face(blocks::DIRT, Face::Up));
        assert!(bt.is_tinted(blocks::OAK_LEAVES, Face::North));
        assert!(bt.is_tinted(blocks::WATER, Face::Up));
        assert!(bt.is_tinted(blocks::SHORT_GRASS, Face::North));
        assert!(!bt.is_tinted(blocks::STONE, Face::Up));
        assert!(!bt.is_tinted(blocks::OAK_LOG, Face::Up));
        // Leaves keep their cutout alpha, stone is opaque.
        let leaves = bt.tile(bt.face(blocks::OAK_LEAVES, Face::Up));
        assert!(leaves.data.chunks_exact(4).any(|p| p[3] == 0));
    }

    #[test]
    fn variants_and_tint_flags() {
        let bt = load_block_textures(&pack());
        // A natural pumpkin has no carved face.
        assert_eq!(
            bt.face(blocks::PUMPKIN, Face::South),
            bt.face(blocks::PUMPKIN, Face::North)
        );
        // Jungle leaves have coloured specks but are still biome tinted;
        // sugar cane art is pre-coloured and must not be tinted again.
        assert!(bt.is_tinted(blocks::JUNGLE_LEAVES, Face::Up));
        assert!(bt.is_tinted(blocks::VINE, Face::North));
        assert!(bt.is_tinted(blocks::LILY_PAD, Face::Up));
        assert!(!bt.is_tinted(blocks::SUGAR_CANE, Face::North));
        assert!(!bt.is_tinted(blocks::CHERRY_LEAVES, Face::Up));
    }

    #[test]
    fn liquids_are_animated() {
        let bt = load_block_textures(&pack());
        for b in [blocks::WATER, blocks::LAVA] {
            for f in [Face::Up, Face::North] {
                let t = bt.face(b, f);
                let anim = bt
                    .animations
                    .iter()
                    .find(|a| a.tile == t)
                    .unwrap_or_else(|| panic!("{} {f:?} not animated", b.def().name));
                assert!(anim.frames.len() > 1);
                assert_eq!(anim.frames[0], t);
                assert!((t as usize) < bt.static_tile_count);
                assert!(
                    anim.frames[1..]
                        .iter()
                        .all(|&f| f as usize >= bt.static_tile_count)
                );
                assert_ne!(bt.animated_tile(t, 1000), 0);
            }
        }
        assert_eq!(bt.destroy_stages.len(), 10);
    }
}
