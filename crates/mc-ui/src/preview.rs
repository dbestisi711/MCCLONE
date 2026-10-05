//! A small CPU rasteriser for [`UiDrawList`]s.
//!
//! Used for UI previews (`cargo run -p mc-ui --example ui_preview`) and
//! tests, so the UI can be checked without a GPU. It mirrors what the GPU
//! renderer is expected to do: nearest-neighbour sampling, straight alpha
//! blending, quads split into triangles (0,1,2) and (0,2,3).

use std::sync::Arc;

use glam::Vec2;
use mc_assets::Assets;
use mc_core::Rgba8Image;
use mc_core::render_types::{UiDrawList, UiQuad};
use rustc_hash::FxHashMap as HashMap;

pub struct SoftwareRenderer<'a> {
    assets: &'a Assets,
    dynamic: HashMap<String, Arc<Rgba8Image>>,
    cache: HashMap<String, Option<Arc<Rgba8Image>>>,
    white: Arc<Rgba8Image>,
}

impl<'a> SoftwareRenderer<'a> {
    pub fn new(assets: &'a Assets) -> Self {
        SoftwareRenderer {
            assets,
            dynamic: HashMap::default(),
            cache: HashMap::default(),
            white: Arc::new(Rgba8Image::filled(1, 1, [255; 4])),
        }
    }

    fn texture(&mut self, key: &str, layer: u32) -> Option<Arc<Rgba8Image>> {
        match key {
            "@white" => Some(self.white.clone()),
            "@blocks" => {
                let k = format!("@blocks#{layer}");
                if let Some(t) = self.cache.get(&k) {
                    return t.clone();
                }
                let t = self
                    .assets
                    .blocks
                    .tiles
                    .get(layer as usize)
                    .map(|t| Arc::new(t.clone()));
                self.cache.insert(k, t.clone());
                t
            }
            k if k.starts_with('@') => self.dynamic.get(k).cloned(),
            k => {
                if let Some(t) = self.cache.get(k) {
                    return t.clone();
                }
                let t = self.assets.pack.load_image(k).map(Arc::new);
                self.cache.insert(k.to_string(), t.clone());
                t
            }
        }
    }

    /// Draw `list` over `target` (which holds the "world" image).
    pub fn render(&mut self, list: &UiDrawList, target: &mut Rgba8Image) {
        for (k, img) in &list.upload {
            self.dynamic.insert(k.as_str().to_string(), img.clone());
        }
        if list.dim_world {
            for px in target.data.chunks_exact_mut(4) {
                for c in &mut px[..3] {
                    *c = (*c as f32 * 0.25 + 16.0 * 0.75) as u8;
                }
            }
        }
        for q in &list.quads {
            let Some(tex) = self.texture(q.texture.as_str(), q.layer) else {
                continue;
            };
            draw_quad(target, q, &tex);
        }
    }
}

fn draw_quad(target: &mut Rgba8Image, q: &UiQuad, tex: &Rgba8Image) {
    for tri in [[0usize, 1, 2], [0, 2, 3]] {
        let p = tri.map(|i| q.pos[i]);
        let uv = tri.map(|i| q.uv[i]);
        draw_triangle(target, p, uv, tex, q.color);
    }
}

fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn draw_triangle(
    target: &mut Rgba8Image,
    p: [Vec2; 3],
    uv: [Vec2; 3],
    tex: &Rgba8Image,
    color: [f32; 4],
) {
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let minx = p
        .iter()
        .map(|v| v.x)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as i32;
    let miny = p
        .iter()
        .map(|v| v.y)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as i32;
    let maxx = p
        .iter()
        .map(|v| v.x)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(target.width as f32) as i32;
    let maxy = p
        .iter()
        .map(|v| v.y)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(target.height as f32) as i32;
    for y in miny..maxy {
        for x in minx..maxx {
            let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p[1], p[2], c) / area;
            let w1 = edge(p[2], p[0], c) / area;
            let w2 = edge(p[0], p[1], c) / area;
            // Half-open coverage so shared edges are not drawn twice.
            let eps = -1e-5;
            if w0 < eps || w1 < eps || w2 < eps {
                continue;
            }
            if (w0 == 0.0 && edge_is_excluded(p[1], p[2], area))
                || (w1 == 0.0 && edge_is_excluded(p[2], p[0], area))
                || (w2 == 0.0 && edge_is_excluded(p[0], p[1], area))
            {
                continue;
            }
            let t = uv[0] * w0 + uv[1] * w1 + uv[2] * w2;
            let tx =
                ((t.x * tex.width as f32).floor() as i64).clamp(0, tex.width as i64 - 1) as u32;
            let ty =
                ((t.y * tex.height as f32).floor() as i64).clamp(0, tex.height as i64 - 1) as u32;
            let s = tex.get(tx, ty);
            let a = s[3] as f32 / 255.0 * color[3];
            if a <= 0.0 {
                continue;
            }
            let d = target.get(x as u32, y as u32);
            let mut o = [0u8; 4];
            for i in 0..3 {
                let src = s[i] as f32 * color[i].clamp(0.0, 1.0);
                o[i] = (src * a + d[i] as f32 * (1.0 - a))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            o[3] = 255;
            target.put(x as u32, y as u32, o);
        }
    }
}

/// Top-left fill rule: exclude right/bottom edges.
fn edge_is_excluded(a: Vec2, b: Vec2, area: f32) -> bool {
    let d = if area > 0.0 { b - a } else { a - b };
    !(d.y < 0.0 || (d.y == 0.0 && d.x > 0.0))
}

/// A generic preview backdrop: a sky gradient over rows of block tiles.
pub fn backdrop(assets: &Assets, w: u32, h: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(w, h);
    let horizon = h * 3 / 5;
    for y in 0..h {
        for x in 0..w {
            let c = if y < horizon {
                let t = y as f32 / horizon as f32;
                [(120.0 + 60.0 * t) as u8, (167.0 + 50.0 * t) as u8, 255, 255]
            } else {
                [0, 0, 0, 255]
            };
            img.put(x, y, c);
        }
    }
    let grass = mc_core::blocks::GRASS_BLOCK;
    let top = assets.blocks.face(grass, mc_core::Face::Up);
    let dirt = assets.blocks.face(mc_core::blocks::DIRT, mc_core::Face::Up);
    let tint = assets.colormaps.grass(0.8, 0.4);
    let tile_px = 48u32;
    for y in horizon..h {
        for x in 0..w {
            let row = (y - horizon) / tile_px;
            let tile = if row == 0 { top } else { dirt };
            let Some(t) = assets.blocks.tiles.get(tile as usize) else {
                continue;
            };
            let tx = (x % tile_px) * t.width / tile_px;
            let ty = ((y - horizon) % tile_px) * t.height / tile_px;
            let mut c = t.get(tx, ty);
            if row == 0 {
                c[0] = (c[0] as u32 * (tint >> 16 & 0xFF) / 255) as u8;
                c[1] = (c[1] as u32 * (tint >> 8 & 0xFF) / 255) as u8;
                c[2] = (c[2] as u32 * (tint & 0xFF) / 255) as u8;
            }
            c[3] = 255;
            img.put(x, y, c);
        }
    }
    img
}
