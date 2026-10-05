//! Resource-pack loading: textures, block texture set, item icons, entity models.
//!
//! The pack is the repository root itself (it contains `blocks.json`,
//! `textures/`, `models/`, ...). Assets are read at runtime from that
//! directory; nothing from the pack is embedded in the binary.
//!
//! OWNER: models & textures agent. Public API used by other crates:
//! - [`Pack::open`], [`Pack::find`], [`Pack::load_image`]
//! - [`Assets::load`] → [`BlockTextures`], [`ItemIcons`], [`EntityModels`], [`ColorMaps`]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_core::{BlockId, Face, ItemId, Rgba8Image};
use rustc_hash::FxHashMap as HashMap;

pub mod models;

pub use models::{EntityModel, EntityModels, ModelVertex};

/// Handle to the resource pack directory.
#[derive(Clone, Debug)]
pub struct Pack {
    pub root: PathBuf,
}

impl Pack {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Pack { root: root.into() }
    }

    /// Locate the pack: `$MC_PACK_DIR`, else walk up from the current dir and
    /// the executable's dir looking for `blocks.json` + `textures/`.
    pub fn find() -> Option<Pack> {
        if let Ok(p) = std::env::var("MC_PACK_DIR") {
            return Some(Pack::open(p));
        }
        let mut starts = vec![];
        if let Ok(cwd) = std::env::current_dir() {
            starts.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                starts.push(d.to_path_buf());
            }
        }
        // Compile-time fallback: the workspace root.
        starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        for start in starts {
            let mut dir: Option<&Path> = Some(&start);
            while let Some(d) = dir {
                if d.join("blocks.json").is_file() && d.join("textures").is_dir() {
                    return Some(Pack::open(d));
                }
                dir = d.parent();
            }
        }
        None
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Load an image by pack-relative path. The extension may be omitted, in
    /// which case `.png` then `.tga` are tried.
    pub fn load_image(&self, rel: &str) -> Option<Rgba8Image> {
        let base = self.root.join(rel);
        let candidates: Vec<PathBuf> = if base.extension().is_some() && base.is_file() {
            vec![base]
        } else {
            vec![
                base.with_extension("png"),
                base.with_extension("tga"),
                PathBuf::from(format!("{}.png", base.display())),
                PathBuf::from(format!("{}.tga", base.display())),
            ]
        };
        for p in candidates {
            if p.is_file() {
                match image::open(&p) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        return Some(Rgba8Image {
                            width: rgba.width(),
                            height: rgba.height(),
                            data: rgba.into_raw(),
                        });
                    }
                    Err(e) => log::warn!("failed to decode {}: {e}", p.display()),
                }
            }
        }
        None
    }

    pub fn load_json(&self, rel: &str) -> Option<serde_json::Value> {
        mc_core::json::load_lenient(&self.root.join(rel))
            .map_err(|e| log::warn!("{e}"))
            .ok()
    }
}

/// Index of a 16×16 tile in [`BlockTextures::tiles`].
pub type TileId = u16;

/// All block face textures as equally sized tiles (suitable for a 2D texture
/// array, which avoids atlas bleeding and lets greedy meshes repeat UVs).
#[derive(Clone, Debug, Default)]
pub struct BlockTextures {
    /// Tile edge length in pixels (16 for the vanilla pack).
    pub tile_size: u32,
    /// Tile images, all `tile_size` × `tile_size`. Tile 0 is the "missing" texture.
    pub tiles: Vec<Rgba8Image>,
    /// Per block, per face (`Face::index()` order) tile index.
    pub faces: Vec<[TileId; 6]>,
    /// Per block, per face: apply the block's biome tint to this face.
    pub tinted: Vec<[bool; 6]>,
    /// Optional overlay tile drawn on top of a face, tinted (grass block sides).
    pub overlay: Vec<[Option<TileId>; 6]>,
    /// Tile name → index (texture key from `terrain_texture.json`).
    pub by_name: HashMap<String, TileId>,
    /// Animated tiles: (tile, frames, ticks per frame). Frames are additional tiles.
    pub animations: Vec<TileAnimation>,
    /// Block-breaking crack overlay tiles, stage 0..10 (`textures/environment/destroy_stage_N`).
    pub destroy_stages: Vec<TileId>,
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
}

/// Item icons, one image per item (block items may be `None`; the UI then
/// draws an isometric cube from the block textures).
#[derive(Clone, Debug, Default)]
pub struct ItemIcons {
    pub icons: Vec<Option<Arc<Rgba8Image>>>,
}

impl ItemIcons {
    pub fn get(&self, item: ItemId) -> Option<&Arc<Rgba8Image>> {
        self.icons.get(item.0 as usize).and_then(|i| i.as_ref())
    }
}

/// Grass/foliage colormaps from `textures/colormap/`.
#[derive(Clone, Debug, Default)]
pub struct ColorMaps {
    pub grass: Option<Rgba8Image>,
    pub foliage: Option<Rgba8Image>,
}

impl ColorMaps {
    fn sample(img: &Option<Rgba8Image>, temperature: f32, downfall: f32, fallback: u32) -> u32 {
        let Some(img) = img else { return fallback };
        let t = temperature.clamp(0.0, 1.0);
        let d = downfall.clamp(0.0, 1.0) * t;
        let x = ((1.0 - t) * (img.width - 1) as f32) as u32;
        let y = ((1.0 - d) * (img.height - 1) as f32) as u32;
        let [r, g, b, _] = img.get(x, y);
        (r as u32) << 16 | (g as u32) << 8 | b as u32
    }
    pub fn grass(&self, temperature: f32, downfall: f32) -> u32 {
        Self::sample(&self.grass, temperature, downfall, 0x79C05A)
    }
    pub fn foliage(&self, temperature: f32, downfall: f32) -> u32 {
        Self::sample(&self.foliage, temperature, downfall, 0x59AE30)
    }
    /// Tint for a block in a biome, 0xRRGGBB (0xFFFFFF = no tint).
    pub fn block_tint(&self, block: BlockId, biome: mc_core::BiomeId) -> u32 {
        use mc_core::block::Tint;
        let b = biome.def();
        match block.def().tint {
            Tint::None => 0xFFFFFF,
            Tint::Grass => b
                .grass_color
                .unwrap_or_else(|| self.grass(b.temperature, b.downfall)),
            Tint::Foliage => b
                .foliage_color
                .unwrap_or_else(|| self.foliage(b.temperature, b.downfall)),
            Tint::Water => b.water_color,
            Tint::Fixed(c) => c,
        }
    }
}

/// Everything loaded from the pack at startup.
pub struct Assets {
    pub pack: Pack,
    pub blocks: BlockTextures,
    pub items: ItemIcons,
    pub models: EntityModels,
    pub colormaps: ColorMaps,
}

impl Assets {
    pub fn load(pack: Pack) -> Assets {
        let blocks = load_block_textures(&pack);
        let colormaps = ColorMaps {
            grass: pack.load_image("textures/colormap/grass"),
            foliage: pack.load_image("textures/colormap/foliage"),
        };
        let models = EntityModels::load(&pack);
        Assets {
            items: ItemIcons::default(),
            blocks,
            models,
            colormaps,
            pack,
        }
    }
}

/// Minimal block texture loader: resolves `blocks.json` → `terrain_texture.json`
/// → image for every registered block. (The assets agent will extend this
/// with overlays, animations, carried textures, mipmaps etc.)
pub fn load_block_textures(pack: &Pack) -> BlockTextures {
    let mut bt = BlockTextures {
        tile_size: 16,
        ..Default::default()
    };
    let missing = magenta_checker(16);
    bt.tiles.push(missing);
    bt.by_name.insert("missing".into(), 0);

    let blocks_json = pack.load_json("blocks.json").unwrap_or_default();
    let terrain = pack
        .load_json("textures/terrain_texture.json")
        .unwrap_or_default();
    let tex_data = &terrain["texture_data"];

    let tile_for = |bt: &mut BlockTextures, key: &str| -> TileId {
        if let Some(&t) = bt.by_name.get(key) {
            return t;
        }
        let path = match &tex_data[key]["textures"] {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(a) => a.first().and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                o => o["path"].as_str().map(str::to_string),
            }),
            o => o["path"].as_str().map(str::to_string),
        };
        let img = path.and_then(|p| pack.load_image(&p));
        let id = match img {
            Some(img) => {
                // Use the first square frame (animated textures are vertical strips).
                let s = img.width.min(img.height);
                let mut tile = img.crop(0, 0, s, s);
                if s != 16 {
                    tile = resize_nearest(&tile, 16, 16);
                }
                bt.tiles.push(tile);
                (bt.tiles.len() - 1) as TileId
            }
            None => 0,
        };
        bt.by_name.insert(key.to_string(), id);
        id
    };

    for b in BlockId::all() {
        let def = b.def();
        let entry = &blocks_json[def.pack_name]["textures"];
        let mut faces = [0; 6];
        for f in Face::ALL {
            let key = match entry {
                serde_json::Value::String(s) => Some(s.as_str()),
                serde_json::Value::Object(m) => m
                    .get(f.pack_key())
                    .or_else(|| match f {
                        Face::North | Face::South | Face::East | Face::West => m.get("side"),
                        _ => None,
                    })
                    .and_then(|v| v.as_str()),
                _ => None,
            };
            faces[f.index()] = key.map(|k| tile_for(&mut bt, k)).unwrap_or(0);
        }
        bt.faces.push(faces);
        let tint_all = !matches!(def.tint, mc_core::block::Tint::None);
        let tinted = if b == mc_core::blocks::GRASS_BLOCK {
            [true, false, false, false, false, false]
        } else {
            [tint_all; 6]
        };
        bt.tinted.push(tinted);
        bt.overlay.push([None; 6]);
    }
    bt
}

pub fn magenta_checker(size: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / (size / 2)) + (y / (size / 2))) % 2 == 0;
            img.put(
                x,
                y,
                if on {
                    [248, 0, 248, 255]
                } else {
                    [0, 0, 0, 255]
                },
            );
        }
    }
    img
}

pub fn resize_nearest(src: &Rgba8Image, w: u32, h: u32) -> Rgba8Image {
    let mut out = Rgba8Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.put(x, y, src.get(x * src.width / w, y * src.height / h));
        }
    }
    out
}
