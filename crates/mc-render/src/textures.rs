//! Textures: the block tile array (`@blocks`), animated tiles, and a cache
//! of 2D textures (pack images, dynamic UI textures, item icons).
//!
//! The block array holds one layer per `TileId` (plus a few extra tiles the
//! renderer adds itself: crack stages and animation frames when the asset
//! loader did not provide them). Mipmaps are built on the CPU: tiles used by
//! cutout blocks average colour weighted by alpha and rescale alpha so the
//! fraction of texels passing the alpha test stays constant, which keeps
//! leaves from dissolving in the distance. If the tile count exceeds the
//! device's array-layer limit the tiles are split over several arrays.

use std::sync::Arc;

use mc_assets::{Assets, TileId};
use mc_core::block::Layer;
use mc_core::{BlockId, ItemId, Rgba8Image};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub const BLOCK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// An animated tile: `frames` are layers copied into `layer` in turn.
#[derive(Clone, Debug)]
pub struct Anim {
    pub layer: u32,
    pub frames: Vec<u32>,
    pub ticks_per_frame: u32,
    current: usize,
}

/// CPU side of the block tile array (testable without a GPU).
pub struct TileSet {
    pub size: u32,
    pub levels: u32,
    /// Per layer, per mip level: RGBA bytes.
    pub mips: Vec<Vec<Vec<u8>>>,
    pub anims: Vec<Anim>,
    /// Layers of the 10 crack stages.
    pub destroy: Vec<u32>,
}

fn resize_to(img: &Rgba8Image, size: u32) -> Rgba8Image {
    if img.width == size && img.height == size {
        return img.clone();
    }
    mc_assets::resize_nearest(img, size, size)
}

/// Build the tile list: asset tiles, then extra tiles.
pub fn build_tileset(assets: &Assets) -> TileSet {
    let bt = &assets.blocks;
    let size = bt.tile_size.max(1);
    let mut tiles: Vec<Rgba8Image> = bt.tiles.iter().map(|t| resize_to(t, size)).collect();
    if tiles.is_empty() {
        tiles.push(mc_assets::magenta_checker(size));
    }
    // Which tiles are alpha tested (mip alpha coverage) vs opaque.
    let mut cutout: HashSet<TileId> = HashSet::default();
    for id in BlockId::all() {
        let d = id.def();
        let Some(faces) = bt.faces.get(id.0 as usize) else {
            continue;
        };
        if d.layer != Layer::Opaque {
            cutout.extend(faces.iter().copied());
        }
        if let Some(ov) = bt.overlay.get(id.0 as usize) {
            cutout.extend(ov.iter().flatten().copied());
        }
    }

    // Crack overlay tiles.
    let mut destroy: Vec<u32> = bt.destroy_stages.iter().map(|&t| t as u32).collect();
    if destroy.len() < 10 {
        destroy.clear();
        for i in 0..10 {
            if let Some(img) = assets
                .pack
                .load_image(&format!("textures/environment/destroy_stage_{i}"))
            {
                tiles.push(resize_to(&img, size));
                destroy.push((tiles.len() - 1) as u32);
            }
        }
    }
    for &t in &destroy {
        cutout.insert(t as TileId);
    }

    // Animations: from the assets, or from the pack's flipbook list.
    let mut anims: Vec<Anim> = bt
        .animations
        .iter()
        .filter(|a| !a.frames.is_empty())
        .map(|a| Anim {
            layer: a.tile as u32,
            frames: a.frames.iter().map(|&f| f as u32).collect(),
            ticks_per_frame: a.ticks_per_frame.max(1),
            current: usize::MAX,
        })
        .collect();
    if anims.is_empty() {
        anims = flipbook_fallback(assets, &mut tiles, &mut cutout, size);
    }

    let levels = size.max(1).ilog2() + 1;
    let mips = tiles
        .iter()
        .enumerate()
        .map(|(i, t)| build_mips(t, levels, cutout.contains(&(i as TileId))))
        .collect();
    TileSet {
        size,
        levels,
        mips,
        anims,
        destroy,
    }
}

/// Read `textures/flipbook_textures.json` and add the frames of animated
/// tiles that blocks use (water, lava, fire...).
fn flipbook_fallback(
    assets: &Assets,
    tiles: &mut Vec<Rgba8Image>,
    cutout: &mut HashSet<TileId>,
    size: u32,
) -> Vec<Anim> {
    let mut out = Vec::new();
    let Some(json) = assets.pack.load_json("textures/flipbook_textures.json") else {
        return out;
    };
    let Some(list) = json.as_array() else {
        return out;
    };
    let used: HashSet<TileId> = assets.blocks.faces.iter().flatten().copied().collect();
    for e in list {
        let (Some(name), Some(path)) = (e["atlas_tile"].as_str(), e["flipbook_texture"].as_str())
        else {
            continue;
        };
        let Some(&base) = assets.blocks.by_name.get(name) else {
            continue;
        };
        if base == 0 || !used.contains(&base) {
            continue;
        }
        let Some(img) = assets.pack.load_image(path) else {
            continue;
        };
        let fw = img.width.max(1);
        let n = (img.height / fw).max(1);
        if n <= 1 {
            continue;
        }
        let frame_imgs: Vec<Rgba8Image> = (0..n)
            .map(|i| resize_to(&img.crop(0, i * fw, fw, fw), size))
            .collect();
        let order: Vec<u32> = match e["frames"].as_array() {
            Some(a) => a
                .iter()
                .filter_map(|v| v.as_u64())
                .map(|v| v as u32)
                .filter(|&v| v < n)
                .collect(),
            None => (0..n).collect(),
        };
        if order.is_empty() {
            continue;
        }
        let first = tiles.len() as u32;
        let is_cut = cutout.contains(&base);
        for f in frame_imgs {
            if is_cut {
                cutout.insert(tiles.len() as TileId);
            }
            tiles.push(f);
        }
        out.push(Anim {
            layer: base as u32,
            frames: order.iter().map(|&i| first + i).collect(),
            ticks_per_frame: e["ticks_per_frame"].as_u64().unwrap_or(1).max(1) as u32,
            current: usize::MAX,
        });
    }
    out
}

/// Fraction of texels with alpha at or above the test threshold.
fn coverage(alpha: &[f32], scale: f32) -> f32 {
    let n = alpha
        .iter()
        .filter(|&&a| (a * scale).min(255.0) >= 127.5)
        .count();
    n as f32 / alpha.len().max(1) as f32
}

/// Mip chain for one square tile.
pub fn build_mips(tile: &Rgba8Image, levels: u32, alpha_tested: bool) -> Vec<Vec<u8>> {
    let mut out = vec![tile.data.clone()];
    let binary = tile.data.chunks_exact(4).all(|p| p[3] < 16 || p[3] > 240);
    let coverage_fix = alpha_tested && binary;
    let base_alpha: Vec<f32> = tile.data.chunks_exact(4).map(|p| p[3] as f32).collect();
    let target = coverage(&base_alpha, 1.0);
    let mut size = tile.width;
    for _ in 1..levels {
        let prev = out.last().unwrap();
        let ns = (size / 2).max(1);
        let mut next = vec![0u8; (ns * ns * 4) as usize];
        let mut alphas = vec![0f32; (ns * ns) as usize];
        for y in 0..ns {
            for x in 0..ns {
                let mut c = [0f32; 3];
                let mut cw = [0f32; 3];
                let mut a = 0f32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(size - 1);
                    let sy = (y * 2 + dy).min(size - 1);
                    let i = ((sy * size + sx) * 4) as usize;
                    let pa = prev[i + 3] as f32;
                    for k in 0..3 {
                        c[k] += prev[i + k] as f32;
                        cw[k] += prev[i + k] as f32 * pa;
                    }
                    a += pa;
                }
                let o = ((y * ns + x) * 4) as usize;
                for k in 0..3 {
                    // Colour weighted by alpha so transparent texels don't
                    // bleed dark fringes into the mips.
                    let v = if alpha_tested && a > 0.0 {
                        cw[k] / a
                    } else {
                        c[k] / 4.0
                    };
                    next[o + k] = v.round().clamp(0.0, 255.0) as u8;
                }
                alphas[(y * ns + x) as usize] = a / 4.0;
            }
        }
        let scale = if coverage_fix && target > 0.0 {
            // Binary search the alpha scale that keeps coverage constant.
            let (mut lo, mut hi) = (0.0f32, 8.0f32);
            for _ in 0..20 {
                let mid = (lo + hi) * 0.5;
                if coverage(&alphas, mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            hi
        } else {
            1.0
        };
        for (i, a) in alphas.iter().enumerate() {
            next[i * 4 + 3] = (a * scale).round().clamp(0.0, 255.0) as u8;
        }
        out.push(next);
        size = ns;
    }
    out
}

/// GPU block tile array(s).
pub struct BlockArray {
    pub textures: Vec<wgpu::Texture>,
    pub views: Vec<wgpu::TextureView>,
    pub layers_per_array: u32,
    pub layer_count: u32,
    pub levels: u32,
    pub size: u32,
    pub anims: Vec<Anim>,
    pub destroy: Vec<u32>,
}

impl BlockArray {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, set: TileSet, max_layers: u32) -> Self {
        let total = set.mips.len() as u32;
        let per = max_layers.max(1);
        let arrays = total.div_ceil(per).max(1);
        let mut textures = Vec::new();
        let mut views = Vec::new();
        for a in 0..arrays {
            let first = a * per;
            let n = (total - first).min(per).max(1);
            // A 1-layer array view would be reinterpreted as 2D by some
            // backends; keep at least 2 layers.
            let alloc = n.max(2);
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("block tiles"),
                size: wgpu::Extent3d {
                    width: set.size,
                    height: set.size,
                    depth_or_array_layers: alloc,
                },
                mip_level_count: set.levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: BLOCK_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            for level in 0..set.levels {
                let s = (set.size >> level).max(1);
                let mut data = Vec::with_capacity((s * s * 4 * n) as usize);
                for l in first..first + n {
                    if let Some(m) = set.mips.get(l as usize) {
                        data.extend_from_slice(&m[level as usize]);
                    }
                }
                let layers = (data.len() / (s * s * 4) as usize) as u32;
                if layers == 0 {
                    continue;
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: level,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(s * 4),
                        rows_per_image: Some(s),
                    },
                    wgpu::Extent3d {
                        width: s,
                        height: s,
                        depth_or_array_layers: layers,
                    },
                );
            }
            views.push(tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }));
            textures.push(tex);
        }
        BlockArray {
            textures,
            views,
            layers_per_array: per,
            layer_count: total,
            levels: set.levels,
            size: set.size,
            anims: set.anims,
            destroy: set.destroy,
        }
    }

    fn locate(&self, layer: u32) -> (usize, u32) {
        (
            (layer / self.layers_per_array) as usize,
            layer % self.layers_per_array,
        )
    }

    /// Advance animated tiles to game tick `tick` (GPU-side layer copies).
    pub fn animate(&mut self, enc: &mut wgpu::CommandEncoder, tick: u64) -> u32 {
        let mut copies = 0;
        for i in 0..self.anims.len() {
            let a = &self.anims[i];
            let idx = ((tick / a.ticks_per_frame as u64) % a.frames.len() as u64) as usize;
            if idx == a.current {
                continue;
            }
            let (src_l, dst_l) = (a.frames[idx], a.layer);
            if src_l >= self.layer_count || dst_l >= self.layer_count {
                continue;
            }
            let (sa, sl) = self.locate(src_l);
            let (da, dl) = (self.locate(dst_l).0, self.locate(dst_l).1);
            for level in 0..self.levels {
                let s = (self.size >> level).max(1);
                enc.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.textures[sa],
                        mip_level: level,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: sl },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.textures[da],
                        mip_level: level,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: dl },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: s,
                        height: s,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.anims[i].current = idx;
            copies += 1;
        }
        copies
    }
}

/// A 2D texture with its bind group (group 1 of the textured pipelines).
pub struct Tex2d {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

/// Lazily loaded 2D textures by `TextureKey`.
pub struct TexCache {
    pub layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    map: HashMap<String, Arc<Tex2d>>,
    pub white: Arc<Tex2d>,
    pub missing: Arc<Tex2d>,
}

impl TexCache {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tex2d"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let white_img = Rgba8Image::filled(1, 1, [255; 4]);
        let white = Arc::new(make_tex(
            device, queue, &layout, &sampler, &white_img, "white",
        ));
        let missing_img = mc_assets::magenta_checker(16);
        let missing = Arc::new(make_tex(
            device,
            queue,
            &layout,
            &sampler,
            &missing_img,
            "missing",
        ));
        TexCache {
            layout,
            sampler,
            map: HashMap::default(),
            white,
            missing,
        }
    }

    /// Texture for a key (`@white`, `@<dynamic>` or a pack path). Unknown
    /// dynamic keys and unloadable paths give the magenta "missing" texture.
    pub fn get(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &Assets,
        key: &str,
    ) -> Arc<Tex2d> {
        if key == "@white" {
            return self.white.clone();
        }
        if let Some(t) = self.map.get(key) {
            return t.clone();
        }
        let tex = if key.starts_with('@') {
            self.missing.clone()
        } else {
            match assets.pack.load_image(key) {
                Some(img) => Arc::new(make_tex(
                    device,
                    queue,
                    &self.layout,
                    &self.sampler,
                    &img,
                    key,
                )),
                None => {
                    log::warn!("texture not found: {key}");
                    self.missing.clone()
                }
            }
        };
        self.map.insert(key.to_string(), tex.clone());
        tex
    }

    /// Item icon texture (`assets.items`), cached under `@item:<id>`.
    pub fn item_icon(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &Assets,
        item: ItemId,
    ) -> Option<Arc<Tex2d>> {
        let key = format!("@item:{}", item.0);
        if let Some(t) = self.map.get(&key) {
            return Some(t.clone());
        }
        let img = assets.items.get(item)?;
        let t = Arc::new(make_tex(
            device,
            queue,
            &self.layout,
            &self.sampler,
            img,
            &key,
        ));
        self.map.insert(key, t.clone());
        Some(t)
    }

    /// (Re)upload a dynamic texture.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        img: &Rgba8Image,
    ) {
        if img.width == 0 || img.height == 0 {
            return;
        }
        if let Some(t) = self.map.get(key) {
            if t.width == img.width && t.height == img.height && !Arc::ptr_eq(t, &self.missing) {
                write_rgba(queue, &t.texture, img);
                return;
            }
        }
        let t = Arc::new(make_tex(
            device,
            queue,
            &self.layout,
            &self.sampler,
            img,
            key,
        ));
        self.map.insert(key.to_string(), t);
    }
}

fn write_rgba(queue: &wgpu::Queue, tex: &wgpu::Texture, img: &Rgba8Image) {
    queue.write_texture(
        tex.as_image_copy(),
        &img.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(img.width * 4),
            rows_per_image: Some(img.height),
        },
        wgpu::Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
    );
}

fn make_tex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    img: &Rgba8Image,
    label: &str,
) -> Tex2d {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: img.width.max(1),
            height: img.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: BLOCK_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_rgba(queue, &texture, img);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    Tex2d {
        texture,
        bind_group,
        width: img.width,
        height: img.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_preserve_cutout_coverage() {
        // Sparse checker: 25% opaque texels in a leaf-like pattern.
        let mut img = Rgba8Image::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                let on = (x % 2 == 0) && (y % 2 == 0);
                img.put(x, y, if on { [40, 160, 40, 255] } else { [0, 0, 0, 0] });
            }
        }
        let mips = build_mips(&img, 5, true);
        assert_eq!(mips.len(), 5);
        // Naive averaging would make every 8x8 texel alpha 64 (fails the
        // alpha test everywhere); the coverage fix keeps some visible.
        let lvl1 = &mips[1];
        let visible = lvl1.chunks_exact(4).filter(|p| p[3] >= 128).count();
        assert!(visible > 0);
        // Colour is not darkened by transparent black neighbours.
        assert!(lvl1.chunks_exact(4).all(|p| p[1] > 100));
        let plain = build_mips(&img, 5, false);
        assert!(plain[1].chunks_exact(4).all(|p| p[3] < 128));
    }
}
