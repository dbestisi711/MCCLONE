//! Resource-pack loading: textures, block texture set, item icons, entity models.
//!
//! The pack is the repository root itself (it contains `blocks.json`,
//! `textures/`, `models/`, ...). Assets are read at runtime from that
//! directory; nothing from the pack is embedded in the binary. All file
//! access goes through [`Pack`] so the crate can later be pointed at another
//! storage backend (web).
//!
//! OWNER: models & textures agent. Public API used by other crates:
//! - [`Pack::open`], [`Pack::find`], [`Pack::load_image`], [`Pack::load_texture`]
//! - [`Assets::load`] → [`BlockTextures`], [`ItemIcons`], [`EntityModels`], [`ColorMaps`]
//! - [`generate_mips`] (alpha-aware mip chain for block tiles)
//! - [`EntityModel::mesh`] / [`EntityModel::bone_matrices`] (posed entity triangles)
//! - [`EntityModels::client_entity`] (`"minecraft:pig"` → geometry ids + texture paths),
//!   [`EntityModels::entity_model`], [`EntityModels::rest_pose`]
//! - [`ItemIcons::display_icon`] (flat or isometric icon per item), the icon
//!   `atlas` + [`ItemIcons::atlas_uv`], [`ItemIcons::render_block_icon`]
//!
//! Visual check: `cargo run -p mc-assets --example dump -- <out_dir>` writes
//! contact sheets of tiles, blocks, item icons and posed mob models.
//!
//! ## Block textures
//! Every block in `mc_core::block::BLOCK_DEFS` is resolved through
//! `blocks.json` → `textures/terrain_texture.json` → image. Tiles are 16×16
//! RGBA. Renderer rules:
//! - Multiply a face by `ColorMaps::block_tint(block, biome)` only when
//!   `BlockTextures::tinted[block][face]` is true (e.g. grass top, leaves,
//!   water; NOT the grass sides/bottom, and not sugar cane whose pack texture
//!   is already coloured).
//! - If `overlay[block][face]` is `Some(tile)`, draw that tile on top of the
//!   face, alpha-blended, **always** multiplied by the block tint (grass block
//!   sides: dirt base + tinted grass fringe). The base tile already has a
//!   default plains-green fringe baked in, so ignoring overlays still looks OK.
//! - Animated tiles ([`TileAnimation`]): at game tick `t` show
//!   `frames[(t / ticks_per_frame) % frames.len()]` in place of `tile`
//!   ([`BlockTextures::animated_tile`]). Extra frames are stored after
//!   `static_tile_count`, so a renderer limited to 256 array layers can upload
//!   only the static tiles and copy the current frame into layer `tile`.
//! - Mipmaps: [`generate_mips`] keeps cutout coverage (leaves don't vanish).
//! - [`BlockTextures::isotropic`]: faces whose texture may be randomly
//!   rotated per block to hide tiling (grass top, sand, dirt...).
//!
//! ## Entity models
//! Model space: blocks, +Y up, feet at y = 0, the model faces **-Z**, and
//! the model's own right-hand side (`rightArm`, `leg0`...) is at **+X**.
//! Geo files use a mirrored X axis; the parser converts. Triangles are
//! counter-clockwise when seen from outside. UVs are normalised by the
//! geometry's `texture_width/height`, so any texture resolution works.
//! `BonePose` values use the **same convention as the pack's `.geo.json` and
//! `animations/*.json`**, so numbers can be copied from those files:
//! - `rotation` (degrees, added to the bone's rest rotation, applied X then Y
//!   then Z around the bone pivot): +X pitches the bone's front (-Z side)
//!   down (head looks down, a hanging arm swings backward; zombie arms
//!   straight forward = `x: -90`); +Y turns the front toward the model's
//!   right; +Z rolls the model's right side up.
//! - `offset` (pixels, 1/16 block, geo axes): +x = toward the model's LEFT
//!   side, +y = up, +z = backward.
//! - `scale`: uniform around the pivot, 0 hides the bone and its children.
//!
//! [`EntityModels::rest_pose`] evaluates the constant part of an entity's
//! pack animations (e.g. the spider's leg spread); add gameplay rotations on
//! top. Draw entities **without back-face culling**: mob cutouts are double
//! sided in the game (chicken legs are painted on the inside of a mostly
//! transparent box).
//!
//! ## Entity textures
//! Bedrock `.tga` entity textures use alpha as a *mask* (e.g. sheep wool
//! tint mask), not as transparency. Load textures for rendering with
//! [`Pack::load_texture`], which makes such masks opaque; then alpha-test
//! (discard alpha < 0.5) and the result matches the game.

use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap as HashMap;

use mc_core::{BlockId, Rgba8Image};

pub mod animations;
pub mod blocks;
pub mod image_ops;
pub mod items;
pub mod models;

pub use blocks::{BlockTextures, TileAnimation, TileId, load_block_textures};
pub use image_ops::{generate_mips, magenta_checker, resize_nearest};
pub use items::{BLOCK_ICON_SIZE, ItemIcons};
pub use models::{Bone, ClientEntity, Cube, EntityModel, EntityModels, FaceUv, ModelVertex};

/// Handle to the resource pack: a directory on disk (desktop) or an
/// in-memory bundle of files (web, see [`Pack::from_bundle`]).
#[derive(Clone, Debug)]
pub struct Pack {
    /// Directory the pack was opened from (empty for in-memory packs).
    pub root: PathBuf,
    /// In-memory files keyed by pack-relative path with `/` separators.
    files: Option<Arc<HashMap<String, Arc<[u8]>>>>,
    /// When set, every file that is successfully read is recorded here (used
    /// by the web bundler to find out which files the game needs).
    access_log: Option<Arc<Mutex<BTreeSet<String>>>>,
}

/// Image file formats the pack uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFormat {
    Png,
    Tga,
}

/// Magic header of a pack bundle file.
const BUNDLE_MAGIC: &[u8; 8] = b"MCPACK01";

impl Pack {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Pack {
            root: root.into(),
            files: None,
            access_log: None,
        }
    }

    /// Pack backed by in-memory files (pack-relative path → contents).
    pub fn from_files(files: HashMap<String, Arc<[u8]>>) -> Self {
        Pack {
            root: PathBuf::new(),
            files: Some(Arc::new(files)),
            access_log: None,
        }
    }

    /// Parse a bundle produced by [`Pack::write_bundle`].
    pub fn from_bundle(bytes: &[u8]) -> Result<Self, String> {
        let mut r = bytes;
        let mut take = |n: usize| -> Result<&[u8], String> {
            if r.len() < n {
                return Err("truncated pack bundle".into());
            }
            let (a, b) = r.split_at(n);
            r = b;
            Ok(a)
        };
        if take(8)? != BUNDLE_MAGIC {
            return Err("not a pack bundle".into());
        }
        let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let count = u32_at(take(4)?);
        let mut files = HashMap::default();
        for _ in 0..count {
            let n = u32_at(take(4)?);
            let name = String::from_utf8(take(n)?.to_vec()).map_err(|e| e.to_string())?;
            let n = u32_at(take(4)?);
            files.insert(name, Arc::from(take(n)?));
        }
        Ok(Self::from_files(files))
    }

    /// Serialise the given pack-relative files (read from this pack) into a
    /// bundle for [`Pack::from_bundle`]. Missing files are skipped.
    pub fn write_bundle<'a>(&self, paths: impl IntoIterator<Item = &'a str>) -> Vec<u8> {
        let mut entries = Vec::new();
        for p in paths {
            if let Some(data) = self.read_bytes(p) {
                entries.push((p.to_string(), data));
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(BUNDLE_MAGIC);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (name, data) in entries {
            out.extend_from_slice(&(name.len() as u32).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
        out
    }

    /// Start recording every file read through this pack (and its clones).
    pub fn record_access(&mut self) -> Arc<Mutex<BTreeSet<String>>> {
        let log = Arc::new(Mutex::new(BTreeSet::new()));
        self.access_log = Some(log.clone());
        log
    }

    fn note(&self, rel: &str) {
        if let Some(log) = &self.access_log {
            log.lock().unwrap().insert(rel.to_string());
        }
    }

    /// Locate the pack: `$MC_PACK_DIR`, else a `pack.bin` bundle next to the
    /// executable (packaged builds), else walk up from the current dir and
    /// the executable's dir looking for `blocks.json` + `textures/`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn find() -> Option<Pack> {
        if let Ok(p) = std::env::var("MC_PACK_DIR") {
            return Some(Pack::open(p));
        }
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            let bundle = dir.join("pack.bin");
            if let Ok(bytes) = std::fs::read(&bundle) {
                match Pack::from_bundle(&bytes) {
                    Ok(mut pack) => {
                        pack.root = bundle;
                        return Some(pack);
                    }
                    Err(e) => log::warn!("{}: {e}", bundle.display()),
                }
            }
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

    /// Normalise a pack-relative path for in-memory lookups.
    fn key(rel: &str) -> String {
        rel.trim_start_matches("./").replace('\\', "/")
    }

    /// Read a whole file.
    pub fn read_bytes(&self, rel: &str) -> Option<Arc<[u8]>> {
        let data = match &self.files {
            Some(files) => files.get(&Self::key(rel)).cloned(),
            None => std::fs::read(self.root.join(rel)).ok().map(Arc::from),
        };
        if data.is_some() {
            self.note(rel);
        }
        data
    }

    /// Does a pack-relative file exist?
    pub fn exists(&self, rel: &str) -> bool {
        match &self.files {
            Some(files) => files.contains_key(&Self::key(rel)),
            None => self.root.join(rel).is_file(),
        }
    }

    /// Pack-relative paths (with `/` separators) of the files directly inside
    /// `rel_dir` whose name ends with `suffix`, sorted.
    pub fn list_files(&self, rel_dir: &str, suffix: &str) -> Vec<String> {
        let dir = rel_dir.trim_end_matches('/');
        let mut out = Vec::new();
        match &self.files {
            Some(files) => {
                let prefix = format!("{dir}/");
                for k in files.keys() {
                    if let Some(name) = k.strip_prefix(&prefix) {
                        if !name.contains('/') && name.ends_with(suffix) {
                            out.push(k.clone());
                        }
                    }
                }
            }
            None => {
                let Ok(rd) = std::fs::read_dir(self.root.join(rel_dir)) else {
                    return out;
                };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.ends_with(suffix) && e.path().is_file() {
                        out.push(format!("{dir}/{name}"));
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Resolve an image path. With an explicit `.png`/`.tga` extension that
    /// file is used; otherwise `<rel>.png` and `<rel>.tga` are tried in the
    /// given order.
    fn image_path(&self, rel: &str, prefer_tga: bool) -> Option<(String, ImageFormat)> {
        let lower = rel.to_ascii_lowercase();
        if lower.ends_with(".png") || lower.ends_with(".tga") {
            let fmt = if lower.ends_with(".tga") {
                ImageFormat::Tga
            } else {
                ImageFormat::Png
            };
            return self.exists(rel).then(|| (rel.to_string(), fmt));
        }
        let png = (format!("{rel}.png"), ImageFormat::Png);
        let tga = (format!("{rel}.tga"), ImageFormat::Tga);
        let order = if prefer_tga { [tga, png] } else { [png, tga] };
        order.into_iter().find(|(p, _)| self.exists(p))
    }

    /// Decode an image file into RGBA8.
    fn decode(&self, rel: &str, fmt: ImageFormat) -> Option<Rgba8Image> {
        let bytes = self.read_bytes(rel)?;
        let format = match fmt {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Tga => image::ImageFormat::Tga,
        };
        match image::load_from_memory_with_format(&bytes, format) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                Some(Rgba8Image {
                    width: rgba.width(),
                    height: rgba.height(),
                    data: rgba.into_raw(),
                })
            }
            Err(e) => {
                log::warn!("failed to decode {rel}: {e}");
                None
            }
        }
    }

    /// Load an image by pack-relative path, raw (alpha exactly as stored).
    /// The extension may be omitted, in which case `.png` then `.tga` are tried.
    pub fn load_image(&self, rel: &str) -> Option<Rgba8Image> {
        self.load_image_ext(rel, false).map(|(img, _)| img)
    }

    /// Like [`Pack::load_image`] but chooses the order of `.png`/`.tga` and
    /// reports which format was found. Where a pack ships both, the `.tga`
    /// is the current art (the `.png` is a legacy copy), so block/item
    /// loading prefers TGA.
    pub fn load_image_ext(&self, rel: &str, prefer_tga: bool) -> Option<(Rgba8Image, ImageFormat)> {
        let (path, fmt) = self.image_path(rel, prefer_tga)?;
        self.decode(&path, fmt).map(|img| (img, fmt))
    }

    /// Load a texture for rendering (entity textures, GUI images...): like
    /// [`Pack::load_image`], but for `.tga` files any non-zero alpha becomes
    /// fully opaque, because Bedrock TGAs store tint/emissive masks in alpha
    /// (sheep wool, spider eyes...). Fully transparent texels stay transparent,
    /// so alpha-tested cutouts (skeleton ribs) keep working.
    pub fn load_texture(&self, rel: &str) -> Option<Rgba8Image> {
        let (mut img, fmt) = self.load_image_ext(rel, true)?;
        if fmt == ImageFormat::Tga {
            for p in img.data.chunks_exact_mut(4) {
                if p[3] > 0 {
                    p[3] = 255;
                }
            }
        }
        Some(img)
    }

    /// Read a text file.
    pub fn read_string(&self, rel: &str) -> Option<String> {
        let bytes = self.read_bytes(rel)?;
        String::from_utf8(bytes.to_vec()).ok()
    }

    pub fn load_json(&self, rel: &str) -> Option<serde_json::Value> {
        let s = self.read_string(rel)?;
        mc_core::json::parse_lenient(&s)
            .map_err(|e| log::warn!("{rel}: {e}"))
            .ok()
    }
}

/// Grass/foliage colormaps from `textures/colormap/`.
#[derive(Clone, Debug, Default)]
pub struct ColorMaps {
    pub grass: Option<Rgba8Image>,
    pub foliage: Option<Rgba8Image>,
}

impl ColorMaps {
    pub fn load(pack: &Pack) -> ColorMaps {
        ColorMaps {
            grass: pack.load_image("textures/colormap/grass"),
            foliage: pack.load_image("textures/colormap/foliage"),
        }
    }
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
    /// Tint for a block in a biome, 0xRRGGBB (0xFFFFFF = no tint). Apply it
    /// only to faces flagged in `BlockTextures::tinted` and to overlays.
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
    /// The block's tint in plains (used for item icons and other places
    /// without a biome).
    pub fn default_tint(&self, block: BlockId) -> u32 {
        self.block_tint(block, mc_core::biome::biomes::PLAINS)
    }
}

/// Counts reported by [`Assets::load`].
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub tiles: usize,
    /// Tiles referenced by faces/overlays/destroy stages (the rest are animation frames).
    pub static_tiles: usize,
    pub animated_tiles: usize,
    /// Block faces that fell back to the missing texture.
    pub missing_faces: usize,
    pub items: usize,
    pub items_with_icons: usize,
    /// Items whose icon is a generated placeholder (texture not found).
    pub placeholder_icons: usize,
    pub models: usize,
    pub model_failures: usize,
    pub client_entities: usize,
    pub failures: Vec<String>,
}

/// Everything loaded from the pack at startup.
pub struct Assets {
    pub pack: Pack,
    pub blocks: BlockTextures,
    pub items: ItemIcons,
    pub models: EntityModels,
    pub colormaps: ColorMaps,
    pub report: LoadReport,
}

impl Assets {
    pub fn load(pack: Pack) -> Assets {
        #[cfg(not(target_arch = "wasm32"))]
        let t0 = web_time::Instant::now();
        let index = blocks::PackIndex::load(&pack);
        let colormaps = ColorMaps::load(&pack);
        let blocks = blocks::load_block_textures_with(&pack, &index, &colormaps);
        let items = ItemIcons::load_with(&pack, &index, &blocks, &colormaps);
        let models = EntityModels::load(&pack);

        let mut report = LoadReport {
            tiles: blocks.tiles.len(),
            static_tiles: blocks.static_tile_count,
            animated_tiles: blocks.animations.len(),
            missing_faces: blocks.missing_faces,
            items: mc_core::ItemId::count() - 1,
            items_with_icons: (1..mc_core::ItemId::count())
                .filter(|&i| items.display_icon(mc_core::ItemId(i as u16)).is_some())
                .count(),
            placeholder_icons: items.placeholders.len(),
            models: models.by_id.len(),
            model_failures: models.failures.len(),
            client_entities: models.entities.len(),
            failures: Vec::new(),
        };
        report.failures.extend(blocks.failures.iter().cloned());
        report.failures.extend(items.placeholders.iter().cloned());
        report.failures.extend(models.failures.iter().cloned());
        for f in &report.failures {
            log::warn!("asset problem: {f}");
        }
        #[cfg(not(target_arch = "wasm32"))]
        let took = format!(" in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
        #[cfg(target_arch = "wasm32")]
        let took = String::new();
        log::info!(
            "assets loaded{took}: {} tiles ({} static, {} animated), {} missing block faces, {}/{} items with icons ({} placeholders), {} models ({} failed files), {} client entities",
            report.tiles,
            report.static_tiles,
            report.animated_tiles,
            report.missing_faces,
            report.items_with_icons,
            report.items,
            report.placeholder_icons,
            report.models,
            report.model_failures,
            report.client_entities,
        );
        Assets {
            items,
            blocks,
            models,
            colormaps,
            report,
            pack,
        }
    }
}
