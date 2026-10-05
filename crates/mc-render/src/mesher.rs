//! Chunk section meshing (runs on worker threads).
//!
//! Input is an Arc snapshot of a 16³ section and its 26 neighbours (cheap:
//! sections are copy-on-write). The mesher copies them into a padded 18³
//! block/light grid and emits quads into three layers (opaque, cutout,
//! translucent). It also computes which section faces are connected through
//! see-through voxels, for cave culling.
//!
//! ## Vertex format (3 × u32 = 12 bytes, one quad = 4 vertices)
//!
//! | word | bits |
//! |---|---|
//! | 0 | x+16 (9) · y+16 (9) · z+16 (9) in 1/16 block · normal (3) · flags (2) |
//! | 1 | u (5) · v (5) in 1/16 · tile (12) · ao (2) · sky light ×4 (6) |
//! | 2 | tint r, g, b (8 each, sRGB) · block light ×4 (6) |
//!
//! Positions are relative to the section origin; the per-draw instance
//! supplies the camera-relative origin. Quads use a shared index buffer
//! (`0 1 2 0 2 3`); the anisotropy fix for AO rotates the vertex order.

use std::sync::Arc;

use mc_assets::Assets;
use mc_core::block::{Layer, Shape, Tint};
use mc_core::{BiomeId, BlockId, Face, Section, blocks};

pub const LAYER_OPAQUE: usize = 0;
pub const LAYER_CUTOUT: usize = 1;
pub const LAYER_TRANSLUCENT: usize = 2;

/// u32 words per vertex / per quad.
pub const WORDS_PER_VERTEX: usize = 3;
pub const WORDS_PER_QUAD: usize = 4 * WORDS_PER_VERTEX;
pub const BYTES_PER_QUAD: u64 = (WORDS_PER_QUAD * 4) as u64;

/// Vertex flag: gently sways in the wind (leaves, plants).
pub const FLAG_WAVE: u32 = 1;
/// Vertex flag: liquid surface.
pub const FLAG_LIQUID: u32 = 2;

/// All pairs of faces connected.
pub const ALL_CONNECTED: u64 = (1 << 36) - 1;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TintKind {
    None,
    Grass,
    Foliage,
    Water,
    Fixed([u8; 3]),
}

#[derive(Clone, Copy, Debug)]
pub struct BlockInfo {
    pub shape: Shape,
    pub layer: u8,
    pub occludes: bool,
    /// Faces against the same block are hidden (glass, ice, liquids).
    pub cull_same: bool,
    pub fluid: bool,
    pub tint: TintKind,
    /// Random horizontal offset (flowers, grass).
    pub offset: bool,
    pub wave: bool,
}

/// Lookup tables shared by all mesh jobs (built once from the assets).
pub struct MeshTables {
    pub info: Vec<BlockInfo>,
    pub faces: Vec<[u16; 6]>,
    pub face_tint: Vec<[bool; 6]>,
    pub overlay: Vec<[Option<u16>; 6]>,
    /// Faces whose texture may be rotated randomly per block (grass top, sand...).
    pub isotropic: Vec<[bool; 6]>,
    /// Per biome: grass, foliage, water colours.
    pub biome_tints: Vec<[[u8; 3]; 3]>,
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// Alpha statistics of a tile: (has any alpha < 128, all alpha == 255).
pub fn tile_alpha(img: &mc_core::Rgba8Image) -> (bool, bool) {
    let mut low = false;
    let mut full = true;
    for px in img.data.chunks_exact(4) {
        low |= px[3] < 128;
        full &= px[3] == 255;
    }
    (low, full)
}

impl MeshTables {
    pub fn new(assets: &Assets) -> Self {
        let bt = &assets.blocks;
        let alpha: Vec<(bool, bool)> = bt.tiles.iter().map(tile_alpha).collect();
        let tile_alpha = |t: u16| alpha.get(t as usize).copied().unwrap_or((false, true));
        let mut info = Vec::new();
        let mut faces = Vec::new();
        let mut face_tint = Vec::new();
        let mut overlay = Vec::new();
        for id in BlockId::all() {
            let d = id.def();
            let f = bt.faces.get(id.0 as usize).copied().unwrap_or([0; 6]);
            let ov = bt.overlay.get(id.0 as usize).copied().unwrap_or([None; 6]);
            let tinted = bt.tinted.get(id.0 as usize).copied().unwrap_or([false; 6]);
            let leaves = d.name.ends_with("_leaves");
            let mut layer = match d.layer {
                Layer::Opaque => LAYER_OPAQUE,
                Layer::Cutout => LAYER_CUTOUT,
                Layer::Translucent => LAYER_TRANSLUCENT,
            } as u8;
            // Translucent blocks whose textures have no alpha (lava) are
            // drawn with the opaque pass: no sorting, writes depth.
            if layer == LAYER_TRANSLUCENT as u8 && f.iter().all(|&t| tile_alpha(t).1) {
                layer = LAYER_OPAQUE as u8;
            }
            let tint = match d.tint {
                Tint::None => TintKind::None,
                Tint::Grass => TintKind::Grass,
                Tint::Foliage => TintKind::Foliage,
                Tint::Water => TintKind::Water,
                Tint::Fixed(c) => TintKind::Fixed(rgb(c)),
            };
            // Only faces the asset loader marks as tinted get the biome
            // colour (overlays are always tinted).
            let mut ft = [false; 6];
            for i in 0..6 {
                ft[i] = tint != TintKind::None && tinted[i];
            }
            let cube = matches!(d.shape, Shape::Cube);
            info.push(BlockInfo {
                shape: d.shape,
                layer,
                occludes: d.occludes(),
                cull_same: d.fluid || (cube && !d.opaque && !leaves),
                fluid: d.fluid,
                tint,
                offset: matches!(d.shape, Shape::Cross) && d.replaceable,
                wave: leaves || (matches!(d.shape, Shape::Cross) && d.replaceable),
            });
            faces.push(f);
            face_tint.push(ft);
            overlay.push(ov);
        }
        let rep = |t: Tint| {
            BlockId::all()
                .find(|b| b.def().tint == t)
                .unwrap_or(blocks::AIR)
        };
        let (g, fo, w) = (rep(Tint::Grass), rep(Tint::Foliage), rep(Tint::Water));
        let biome_tints = mc_core::biome::BIOME_DEFS
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let id = BiomeId(i as u8);
                [
                    rgb(assets.colormaps.block_tint(g, id)),
                    rgb(assets.colormaps.block_tint(fo, id)),
                    rgb(if w.is_air() {
                        b.water_color
                    } else {
                        assets.colormaps.block_tint(w, id)
                    }),
                ]
            })
            .collect();
        MeshTables {
            info,
            faces,
            face_tint,
            overlay,
            isotropic: (0..BlockId::count())
                .map(|i| bt.isotropic.get(i).copied().unwrap_or([false; 6]))
                .collect(),
            biome_tints,
        }
    }

    #[inline]
    fn info(&self, id: u16) -> &BlockInfo {
        self.info.get(id as usize).unwrap_or(&self.info[0])
    }
}

/// Snapshot handed to a mesh job.
pub struct MeshInput {
    pub chunk: mc_core::ChunkPos,
    pub sy: usize,
    /// Index `(dy + 1) * 9 + (dz + 1) * 3 + (dx + 1)`; `None` = unloaded or
    /// outside the world.
    pub sections: [Option<Arc<Section>>; 27],
    /// Biome columns of the 3×3 chunks, index `(dz + 1) * 3 + (dx + 1)`.
    pub biomes: [Option<Arc<[BiomeId; 256]>>; 9],
    /// Job generation, echoed back so stale results can be dropped.
    pub generation: u64,
}

pub struct MeshOutput {
    pub chunk: mc_core::ChunkPos,
    pub sy: usize,
    pub generation: u64,
    /// Packed vertex words per layer.
    pub layers: [Vec<u32>; 3],
    pub connectivity: u64,
    pub sections: [Option<Arc<Section>>; 27],
    pub micros: u32,
}

impl MeshOutput {
    pub fn quads(&self, layer: usize) -> usize {
        self.layers[layer].len() / WORDS_PER_QUAD
    }
}

const P: usize = 18;
const P2: usize = P * P;
const PV: usize = P * P * P;

#[inline]
const fn pidx(x: i32, y: i32, z: i32) -> usize {
    ((y + 1) as usize * P + (z + 1) as usize) * P + (x + 1) as usize
}

#[inline]
const fn poff(dx: i32, dy: i32, dz: i32) -> isize {
    (dy * P2 as i32 + dz * P as i32 + dx) as isize
}

/// Face geometry: outward normal, the 4 unit-cube corners (CCW seen from
/// outside: bottom-left, bottom-right, top-right, top-left), and the
/// in-plane "right"/"up" axes used for AO sampling.
struct FaceDef {
    n: [i32; 3],
    corners: [[i32; 3]; 4],
    r: [i32; 3],
    u: [i32; 3],
}

// Order matches `Face::ALL`: Up, Down, North, South, East, West.
const FACES: [FaceDef; 6] = [
    FaceDef {
        n: [0, 1, 0],
        corners: [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]],
        r: [1, 0, 0],
        u: [0, 0, -1],
    },
    FaceDef {
        n: [0, -1, 0],
        corners: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
        r: [1, 0, 0],
        u: [0, 0, 1],
    },
    FaceDef {
        n: [0, 0, -1],
        corners: [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]],
        r: [-1, 0, 0],
        u: [0, 1, 0],
    },
    FaceDef {
        n: [0, 0, 1],
        corners: [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
        r: [1, 0, 0],
        u: [0, 1, 0],
    },
    FaceDef {
        n: [1, 0, 0],
        corners: [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]],
        r: [0, 0, -1],
        u: [0, 1, 0],
    },
    FaceDef {
        n: [-1, 0, 0],
        corners: [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]],
        r: [0, 0, 1],
        u: [0, 1, 0],
    },
];

const UP: usize = 0;
const DOWN: usize = 1;

/// Texture coordinates (1/16) of a block-local point on face `f`.
#[inline]
fn auto_uv(f: usize, c: [i32; 3]) -> [i32; 2] {
    let [x, y, z] = c;
    match f {
        0 => [x, z],
        1 => [x, 16 - z],
        2 => [16 - x, 16 - y],
        3 => [x, 16 - y],
        4 => [16 - z, 16 - y],
        _ => [z, 16 - y],
    }
}

/// Per face and corner: padded-grid offsets (relative to the voxel in
/// front of the face) of the two side neighbours and the diagonal one.
struct AoOffsets {
    q: [isize; 6],
    side: [[[isize; 3]; 4]; 6],
}

const fn ao_offsets() -> AoOffsets {
    let mut q = [0isize; 6];
    let mut side = [[[0isize; 3]; 4]; 6];
    let mut f = 0;
    while f < 6 {
        let d = &FACES[f];
        q[f] = poff(d.n[0], d.n[1], d.n[2]);
        let mut k = 0;
        while k < 4 {
            let a = if k == 1 || k == 2 { 1 } else { -1 };
            let b = if k >= 2 { 1 } else { -1 };
            let s1 = [d.r[0] * a, d.r[1] * a, d.r[2] * a];
            let s2 = [d.u[0] * b, d.u[1] * b, d.u[2] * b];
            side[f][k] = [
                poff(s1[0], s1[1], s1[2]),
                poff(s2[0], s2[1], s2[2]),
                poff(s1[0] + s2[0], s1[1] + s2[1], s1[2] + s2[2]),
            ];
            k += 1;
        }
        f += 1;
    }
    AoOffsets { q, side }
}

const AO: AoOffsets = ao_offsets();

struct Grid {
    ids: [u16; PV],
    light: [u8; PV],
}

/// Per-job tint tables (17×17 vertex corners) for the biome-tinted kinds.
struct Tints<'a> {
    tables: &'a MeshTables,
    biomes: [[u8; 22]; 22],
    cache: [Option<Box<[[u8; 3]; 17 * 17]>>; 3],
}

impl Tints<'_> {
    fn corner(&mut self, kind: TintKind, cx: i32, cz: i32) -> [u8; 3] {
        let k = match kind {
            TintKind::None => return [255; 3],
            TintKind::Fixed(c) => return c,
            TintKind::Grass => 0,
            TintKind::Foliage => 1,
            TintKind::Water => 2,
        };
        if self.cache[k].is_none() {
            self.cache[k] = Some(self.build(k));
        }
        let t = self.cache[k].as_ref().unwrap();
        t[(cz.clamp(0, 16) * 17 + cx.clamp(0, 16)) as usize]
    }

    /// Box-blur biome colours (radius 2) per column, then average the four
    /// columns around each vertex corner.
    fn build(&self, k: usize) -> Box<[[u8; 3]; 17 * 17]> {
        let mut col = [[0u32; 3]; 22 * 22];
        for z in 0..22 {
            for x in 0..22 {
                let b = self.biomes[z][x] as usize;
                let c = self
                    .tables
                    .biome_tints
                    .get(b)
                    .map(|t| t[k])
                    .unwrap_or([255; 3]);
                col[z * 22 + x] = [c[0] as u32, c[1] as u32, c[2] as u32];
            }
        }
        // Columns -1..=16 (18) blurred over -3..=18 (index = column + 3).
        let mut blur = [[0u32; 3]; 18 * 18];
        for z in 0..18 {
            for x in 0..18 {
                let mut s = [0u32; 3];
                for dz in 0..5 {
                    for dx in 0..5 {
                        let c = col[(z + dz) * 22 + x + dx];
                        s[0] += c[0];
                        s[1] += c[1];
                        s[2] += c[2];
                    }
                }
                blur[z * 18 + x] = s;
            }
        }
        let mut out = Box::new([[0u8; 3]; 17 * 17]);
        for cz in 0..17 {
            for cx in 0..17 {
                let mut s = [0u32; 3];
                for (ox, oz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = blur[(cz + oz) * 18 + cx + ox];
                    s[0] += c[0];
                    s[1] += c[1];
                    s[2] += c[2];
                }
                out[cz * 17 + cx] = [(s[0] / 100) as u8, (s[1] / 100) as u8, (s[2] / 100) as u8];
            }
        }
        out
    }
}

struct Quad {
    pos: [[i32; 3]; 4],
    uv: [[i32; 2]; 4],
    normal: u32,
    tile: u32,
    ao: [u8; 4],
    sky: [u8; 4],
    blk: [u8; 4],
    tint: [[u8; 3]; 4],
    flags: u32,
}

#[inline]
fn push_quad(out: &mut Vec<u32>, q: &Quad, flip: bool) {
    for i in 0..4 {
        let k = if flip { (i + 1) & 3 } else { i };
        let p = q.pos[k];
        let w0 = ((p[0] + 16) as u32 & 511)
            | (((p[1] + 16) as u32 & 511) << 9)
            | (((p[2] + 16) as u32 & 511) << 18)
            | (q.normal << 27)
            | (q.flags << 30);
        let w1 = (q.uv[k][0] as u32 & 31)
            | ((q.uv[k][1] as u32 & 31) << 5)
            | ((q.tile & 4095) << 10)
            | ((q.ao[k] as u32 & 3) << 22)
            | ((q.sky[k] as u32 & 63) << 24);
        let t = q.tint[k];
        let w2 = t[0] as u32
            | ((t[1] as u32) << 8)
            | ((t[2] as u32) << 16)
            | ((q.blk[k] as u32 & 63) << 24);
        out.extend_from_slice(&[w0, w1, w2]);
    }
}

/// Choose the quad diagonal that joins the brighter pair of corners.
#[inline]
fn should_flip(q: &Quad) -> bool {
    let b = |k: usize| (q.ao[k] as u32 + 1) * (q.sky[k].max(q.blk[k]) as u32 + 8);
    b(0) + b(2) < b(1) + b(3)
}

struct Mesher<'a> {
    t: &'a MeshTables,
    g: &'a Grid,
    tints: Tints<'a>,
    out: [Vec<u32>; 3],
    /// World block coordinates of the section origin.
    origin: [i32; 3],
    /// Quarter turns applied to full-face UVs (isotropic faces).
    rot: usize,
}

/// Stable per-block hash (world coordinates).
#[inline]
fn block_hash(x: i32, y: i32, z: i32) -> u32 {
    (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (z as u32).wrapping_mul(0x85EB_CA77)
        ^ (y as u32).wrapping_mul(0xC2B2_AE3D)
}

impl Mesher<'_> {
    #[inline]
    fn occ(&self, i: usize) -> bool {
        self.t.info(self.g.ids[i]).occludes
    }

    /// Smooth light at the 4 corners of face `f` of the voxel at padded index
    /// `pi`, plus ambient occlusion if `with_ao`.
    fn face_light(&self, pi: usize, f: usize, with_ao: bool) -> ([u8; 4], [u8; 4], [u8; 4]) {
        let q = (pi as isize + AO.q[f]) as usize;
        let lq = self.g.light[q];
        let mut ao = [3u8; 4];
        let mut sky = [0u8; 4];
        let mut blk = [0u8; 4];
        for k in 0..4 {
            let [o1, o2, oc] = AO.side[f][k];
            let i1 = (q as isize + o1) as usize;
            let i2 = (q as isize + o2) as usize;
            let ic = (q as isize + oc) as usize;
            let (s1, s2, sc) = (self.occ(i1), self.occ(i2), self.occ(ic));
            if with_ao {
                ao[k] = if s1 && s2 {
                    0
                } else {
                    3 - s1 as u8 - s2 as u8 - sc as u8
                };
            }
            let mut ss = (lq >> 4) as u32;
            let mut sb = (lq & 15) as u32;
            let mut n = 1u32;
            if !s1 {
                let l = self.g.light[i1];
                ss += (l >> 4) as u32;
                sb += (l & 15) as u32;
                n += 1;
            }
            if !s2 {
                let l = self.g.light[i2];
                ss += (l >> 4) as u32;
                sb += (l & 15) as u32;
                n += 1;
            }
            if !sc && !(s1 && s2) {
                let l = self.g.light[ic];
                ss += (l >> 4) as u32;
                sb += (l & 15) as u32;
                n += 1;
            }
            sky[k] = ((ss * 4 + n / 2) / n) as u8;
            blk[k] = ((sb * 4 + n / 2) / n) as u8;
        }
        (ao, sky, blk)
    }

    fn flat_light(&self, pi: usize) -> ([u8; 4], [u8; 4]) {
        let l = self.g.light[pi];
        ([(l >> 4) * 4; 4], [(l & 15) * 4; 4])
    }

    fn tint4(&mut self, kind: TintKind, corners: &[[i32; 3]; 4], bx: i32, bz: i32) -> [[u8; 3]; 4] {
        if kind == TintKind::None {
            return [[255; 3]; 4];
        }
        let mut t = [[255; 3]; 4];
        for k in 0..4 {
            let cx = bx + (corners[k][0] + 8).div_euclid(16);
            let cz = bz + (corners[k][2] + 8).div_euclid(16);
            t[k] = self.tints.corner(kind, cx, cz);
        }
        t
    }

    /// Emit face `f` of an axis-aligned box `[lo, hi]` (1/16 units, block
    /// local) of the block at (x, y, z).
    #[allow(clippy::too_many_arguments)]
    fn box_face(
        &mut self,
        layer: usize,
        (x, y, z): (i32, i32, i32),
        f: usize,
        lo: [i32; 3],
        hi: [i32; 3],
        tile: u16,
        tint: TintKind,
        light: ([u8; 4], [u8; 4], [u8; 4]),
        uv_full: bool,
        flags: u32,
    ) {
        let d = &FACES[f];
        let mut pos = [[0i32; 3]; 4];
        let mut uv = [[0i32; 2]; 4];
        for k in 0..4 {
            let c = d.corners[k];
            let local = [
                if c[0] == 1 { hi[0] } else { lo[0] },
                if c[1] == 1 { hi[1] } else { lo[1] },
                if c[2] == 1 { hi[2] } else { lo[2] },
            ];
            uv[k] = if uv_full {
                [[0, 16], [16, 16], [16, 0], [0, 0]][k]
            } else {
                auto_uv(f, local)
            };
            pos[k] = [x * 16 + local[0], y * 16 + local[1], z * 16 + local[2]];
        }
        if self.rot != 0 {
            let r = self.rot;
            uv = [uv[r & 3], uv[(r + 1) & 3], uv[(r + 2) & 3], uv[(r + 3) & 3]];
        }
        let tint4 = self.tint4(tint, &pos, 0, 0);
        let (ao, sky, blk) = light;
        let q = Quad {
            pos,
            uv,
            normal: f as u32,
            tile: tile as u32,
            ao,
            sky,
            blk,
            tint: tint4,
            flags,
        };
        let flip = should_flip(&q);
        push_quad(&mut self.out[layer], &q, flip);
    }

    fn block(&mut self, x: i32, y: i32, z: i32) {
        let pi = pidx(x, y, z);
        let id = self.g.ids[pi];
        let info = *self.t.info(id);
        let layer = info.layer as usize;
        let faces = self.t.faces.get(id as usize).copied().unwrap_or([0; 6]);
        let ftint = self
            .t
            .face_tint
            .get(id as usize)
            .copied()
            .unwrap_or([false; 6]);
        let tint_for = |f: usize| if ftint[f] { info.tint } else { TintKind::None };
        let wave = if info.wave { FLAG_WAVE } else { 0 };
        match info.shape {
            Shape::None => {}
            Shape::Cube => {
                for f in 0..6 {
                    let iso = self.t.isotropic.get(id as usize).is_some_and(|i| i[f]);
                    let ni = (pi as isize + AO.q[f]) as usize;
                    let nid = self.g.ids[ni];
                    let n = self.t.info(nid);
                    if n.occludes || (info.cull_same && nid == id) {
                        continue;
                    }
                    let light = self.face_light(pi, f, true);
                    self.rot = if iso {
                        let [ox, oy, oz] = self.origin;
                        ((block_hash(ox + x, oy + y, oz + z).wrapping_mul(0x27D4_EB2F) >> 13) & 3)
                            as usize
                    } else {
                        0
                    };
                    self.box_face(
                        layer,
                        (x, y, z),
                        f,
                        [0; 3],
                        [16; 3],
                        faces[f],
                        tint_for(f),
                        light,
                        false,
                        wave,
                    );
                    if let Some(ov) = self.t.overlay.get(id as usize).and_then(|o| o[f]) {
                        self.box_face(
                            LAYER_CUTOUT,
                            (x, y, z),
                            f,
                            [0; 3],
                            [16; 3],
                            ov,
                            info.tint,
                            light,
                            false,
                            0,
                        );
                    }
                    self.rot = 0;
                }
            }
            Shape::Liquid => {
                let above = self.g.ids[(pi as isize + AO.q[UP]) as usize];
                let h = if above == id { 16 } else { 14 };
                for f in 0..6 {
                    let ni = (pi as isize + AO.q[f]) as usize;
                    let nid = self.g.ids[ni];
                    if nid == id {
                        continue;
                    }
                    if f != UP && self.t.info(nid).occludes {
                        continue;
                    }
                    let light = self.face_light(pi, f, false);
                    self.box_face(
                        layer,
                        (x, y, z),
                        f,
                        [0; 3],
                        [16, h, 16],
                        faces[f],
                        tint_for(f),
                        light,
                        false,
                        FLAG_LIQUID,
                    );
                }
            }
            Shape::Layer(h) => {
                let h = (h as i32).clamp(1, 16);
                for f in 0..6 {
                    if h <= 1 && f >= 2 {
                        continue;
                    }
                    let ni = (pi as isize + AO.q[f]) as usize;
                    let n = self.t.info(self.g.ids[ni]);
                    let hidden = match f {
                        UP => h == 16 && n.occludes,
                        _ => n.occludes,
                    };
                    if hidden {
                        continue;
                    }
                    let light = if f == UP {
                        self.face_light(pi, f, false)
                    } else {
                        let (s, b) = self.flat_light(pi);
                        ([3; 4], s, b)
                    };
                    self.box_face(
                        layer,
                        (x, y, z),
                        f,
                        [0; 3],
                        [16, h, 16],
                        faces[f],
                        tint_for(f),
                        light,
                        false,
                        0,
                    );
                }
            }
            Shape::Inset => {
                for f in 0..6 {
                    let (lo, hi) = match f {
                        UP | DOWN => ([0, 0, 0], [16, 16, 16]),
                        2 => ([0, 0, 1], [16, 16, 16]),
                        3 => ([0, 0, 0], [16, 16, 15]),
                        4 => ([0, 0, 0], [15, 16, 16]),
                        _ => ([1, 0, 0], [16, 16, 16]),
                    };
                    if f <= DOWN {
                        let ni = (pi as isize + AO.q[f]) as usize;
                        if self.occ(ni) {
                            continue;
                        }
                    }
                    let light = self.face_light(pi, f, true);
                    self.box_face(
                        layer,
                        (x, y, z),
                        f,
                        lo,
                        hi,
                        faces[f],
                        tint_for(f),
                        light,
                        true,
                        0,
                    );
                }
            }
            Shape::Torch => {
                let (s, b) = self.flat_light(pi);
                for f in 0..6 {
                    self.box_face(
                        layer,
                        (x, y, z),
                        f,
                        [7, 0, 7],
                        [9, 10, 9],
                        faces[f],
                        tint_for(f),
                        ([3; 4], s, b),
                        false,
                        0,
                    );
                }
            }
            Shape::Cross => {
                self.cross(x, y, z, pi, faces[2], tint_for(2), layer, wave, info.offset)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cross(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        pi: usize,
        tile: u16,
        tint: TintKind,
        layer: usize,
        flags: u32,
        offset: bool,
    ) {
        let (ox, oz) = if offset {
            let wx = (self.origin[0] + x) as u32;
            let wz = (self.origin[2] + z) as u32;
            let h = (wx.wrapping_mul(0x9E37_79B1)
                ^ wz.wrapping_mul(0x85EB_CA77)
                ^ ((self.origin[1] + y) as u32).wrapping_mul(0xC2B2_AE3D))
            .wrapping_mul(0x27D4_EB2F);
            (((h >> 8) % 7) as i32 - 3, ((h >> 16) % 7) as i32 - 3)
        } else {
            (0, 0)
        };
        let (s, b) = self.flat_light(pi);
        let (a, c) = (1, 15);
        let diagonals = [
            [[a, 0, a], [c, 0, c], [c, 16, c], [a, 16, a]],
            [[c, 0, a], [a, 0, c], [a, 16, c], [c, 16, a]],
        ];
        for d in diagonals {
            let base: [[i32; 3]; 4] =
                d.map(|p| [x * 16 + p[0] + ox, y * 16 + p[1], z * 16 + p[2] + oz]);
            let tint4 = self.tint4(tint, &base, 0, 0);
            let front = Quad {
                pos: base,
                uv: [[0, 16], [16, 16], [16, 0], [0, 0]],
                normal: 6,
                tile: tile as u32,
                ao: [3; 4],
                sky: s,
                blk: b,
                tint: tint4,
                flags,
            };
            push_quad(&mut self.out[layer], &front, false);
            let back = Quad {
                pos: [base[1], base[0], base[3], base[2]],
                uv: [[16, 16], [0, 16], [0, 0], [16, 0]],
                tint: [tint4[1], tint4[0], tint4[3], tint4[2]],
                ..front
            };
            push_quad(&mut self.out[layer], &back, false);
        }
    }
}

fn fill_grid(input: &MeshInput, g: &mut Grid) {
    let above_world = |dy: i32| input.sy as i32 + dy >= mc_core::SECTION_COUNT as i32;
    for py in 0..P as i32 {
        let y = py - 1;
        let (dy, ly) = if y < 0 {
            (-1, 15)
        } else if y > 15 {
            (1, 0)
        } else {
            (0, y)
        };
        for pz in 0..P as i32 {
            let z = pz - 1;
            let (dz, lz) = if z < 0 {
                (-1, 15)
            } else if z > 15 {
                (1, 0)
            } else {
                (0, z)
            };
            let row = (py as usize * P + pz as usize) * P;
            for (dx, x0, x1, lx0) in [(-1, 0usize, 1usize, 15usize), (0, 1, 17, 0), (1, 17, 18, 0)]
            {
                let si = ((dy + 1) * 9 + (dz + 1) * 3 + dx + 1) as usize;
                match &input.sections[si] {
                    Some(sec) => {
                        let base = ((ly as usize) << 8) | ((lz as usize) << 4);
                        match sec.blocks() {
                            Some(b) => {
                                for (k, px) in (x0..x1).enumerate() {
                                    g.ids[row + px] = b[base + lx0 + k].0;
                                }
                            }
                            None => g.ids[row + x0..row + x1].fill(0),
                        }
                        match sec.light_data() {
                            Some(l) => {
                                for (k, px) in (x0..x1).enumerate() {
                                    g.light[row + px] = l[base + lx0 + k];
                                }
                            }
                            None => g.light[row + x0..row + x1].fill(0xF0),
                        }
                    }
                    None => {
                        if dy < 0 && input.sy == 0 {
                            // Below the world: treat as solid so nothing faces down into the void.
                            g.ids[row + x0..row + x1].fill(blocks::BEDROCK.0);
                            g.light[row + x0..row + x1].fill(0);
                        } else {
                            g.ids[row + x0..row + x1].fill(0);
                            let l = if above_world(dy) || dy >= 0 { 0xF0 } else { 0 };
                            g.light[row + x0..row + x1].fill(l);
                        }
                    }
                }
            }
        }
    }
}

/// Faces of the section reachable from each other through see-through voxels.
fn connectivity(g: &Grid, t: &MeshTables) -> u64 {
    let mut visited = [0u64; 64];
    let mut any_solid = false;
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                if t.info(g.ids[pidx(x, y, z)]).occludes {
                    let i = (y << 8 | z << 4 | x) as usize;
                    visited[i >> 6] |= 1 << (i & 63);
                    any_solid = true;
                }
            }
        }
    }
    if !any_solid {
        return ALL_CONNECTED;
    }
    let mut conn = 0u64;
    let mut stack: Vec<u16> = Vec::with_capacity(512);
    for start in 0..4096usize {
        if visited[start >> 6] & (1 << (start & 63)) != 0 {
            continue;
        }
        visited[start >> 6] |= 1 << (start & 63);
        stack.push(start as u16);
        let mut faces = 0u8;
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
            if y == 15 {
                faces |= 1 << Face::Up as u8;
            }
            if y == 0 {
                faces |= 1 << Face::Down as u8;
            }
            if z == 0 {
                faces |= 1 << Face::North as u8;
            }
            if z == 15 {
                faces |= 1 << Face::South as u8;
            }
            if x == 15 {
                faces |= 1 << Face::East as u8;
            }
            if x == 0 {
                faces |= 1 << Face::West as u8;
            }
            let mut go = |j: usize| {
                if visited[j >> 6] & (1 << (j & 63)) == 0 {
                    visited[j >> 6] |= 1 << (j & 63);
                    stack.push(j as u16);
                }
            };
            if x > 0 {
                go(i - 1);
            }
            if x < 15 {
                go(i + 1);
            }
            if z > 0 {
                go(i - 16);
            }
            if z < 15 {
                go(i + 16);
            }
            if y > 0 {
                go(i - 256);
            }
            if y < 15 {
                go(i + 256);
            }
        }
        for a in 0..6 {
            if faces & (1 << a) == 0 {
                continue;
            }
            for b in 0..6 {
                if faces & (1 << b) != 0 {
                    conn |= 1 << (a * 6 + b);
                }
            }
        }
    }
    conn
}

/// Mesh one section (pure function; runs on worker threads).
pub fn mesh_section(input: MeshInput, tables: &MeshTables) -> MeshOutput {
    let t0 = crate::light::web_time_now();
    let center_empty = input.sections[13].as_ref().is_none_or(|s| s.is_empty());
    let mut layers = [Vec::new(), Vec::new(), Vec::new()];
    let mut conn = ALL_CONNECTED;
    if !center_empty {
        let mut g = Box::new(Grid {
            ids: [0; PV],
            light: [0; PV],
        });
        fill_grid(&input, &mut g);
        let mut biomes = [[0u8; 22]; 22];
        for (z, row) in biomes.iter_mut().enumerate() {
            for (x, b) in row.iter_mut().enumerate() {
                let (wx, wz) = (x as i32 - 3, z as i32 - 3);
                let ci = ((wz.div_euclid(16) + 1) * 3 + wx.div_euclid(16) + 1) as usize;
                *b = input.biomes[ci]
                    .as_ref()
                    .map(|bs| bs[(wz.rem_euclid(16) * 16 + wx.rem_euclid(16)) as usize].0)
                    .unwrap_or(0);
            }
        }
        let mut m = Mesher {
            t: tables,
            g: &g,
            tints: Tints {
                tables,
                biomes,
                cache: [None, None, None],
            },
            out: [Vec::new(), Vec::new(), Vec::new()],
            origin: [
                input.chunk.x * 16,
                mc_core::WORLD_MIN_Y + input.sy as i32 * 16,
                input.chunk.z * 16,
            ],
            rot: 0,
        };
        let sec = input.sections[13].as_ref().unwrap();
        let blocks = sec.blocks().unwrap();
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    if blocks[(y << 8 | z << 4 | x) as usize].0 != 0 {
                        m.block(x, y, z);
                    }
                }
            }
        }
        layers = m.out;
        conn = connectivity(&g, tables);
    }
    MeshOutput {
        chunk: input.chunk,
        sy: input.sy,
        generation: input.generation,
        layers,
        connectivity: conn,
        sections: input.sections,
        micros: crate::light::elapsed_micros(t0),
    }
}

/// Faces `a` and `b` of a section with connectivity `conn` see each other.
#[inline]
pub fn connected(conn: u64, a: usize, b: usize) -> bool {
    conn & (1 << (a * 6 + b)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_assets::{BlockTextures, ColorMaps, EntityModels, ItemIcons, Pack};
    use mc_core::ChunkPos;

    fn tables() -> MeshTables {
        let n = BlockId::count();
        let assets = Assets {
            pack: Pack::open("."),
            blocks: BlockTextures {
                tile_size: 16,
                tiles: vec![mc_core::Rgba8Image::filled(16, 16, [255; 4])],
                faces: vec![[0; 6]; n],
                tinted: vec![[false; 6]; n],
                overlay: vec![[None; 6]; n],
                ..Default::default()
            },
            items: ItemIcons::default(),
            models: EntityModels::default(),
            colormaps: ColorMaps::default(),
            report: Default::default(),
        };
        MeshTables::new(&assets)
    }

    fn input_with(blocks_at: &[(i32, i32, i32, BlockId)]) -> MeshInput {
        let mut sec = Section::empty();
        for &(x, y, z, b) in blocks_at {
            sec.set(x as usize, y as usize, z as usize, b);
        }
        let mut sections: [Option<Arc<Section>>; 27] =
            std::array::from_fn(|_| Some(Arc::new(Section::empty())));
        sections[13] = Some(Arc::new(sec));
        MeshInput {
            chunk: ChunkPos::new(0, 0),
            sy: 8,
            sections,
            biomes: Default::default(),
            generation: 0,
        }
    }

    #[test]
    fn single_and_adjacent_cubes() {
        let t = tables();
        let out = mesh_section(input_with(&[(5, 5, 5, blocks::STONE)]), &t);
        assert_eq!(out.quads(LAYER_OPAQUE), 6);
        let out = mesh_section(
            input_with(&[(5, 5, 5, blocks::STONE), (6, 5, 5, blocks::STONE)]),
            &t,
        );
        assert_eq!(out.quads(LAYER_OPAQUE), 10);
    }

    #[test]
    fn water_and_glass_culling() {
        let t = tables();
        let out = mesh_section(
            input_with(&[(5, 5, 5, blocks::WATER), (6, 5, 5, blocks::WATER)]),
            &t,
        );
        // (The test tile has no alpha, so water lands in the opaque layer.)
        assert_eq!(out.quads(LAYER_TRANSLUCENT) + out.quads(LAYER_OPAQUE), 10);
        let out = mesh_section(
            input_with(&[(5, 5, 5, blocks::GLASS), (5, 6, 5, blocks::GLASS)]),
            &t,
        );
        assert_eq!(out.quads(LAYER_CUTOUT), 10);
        // Leaves keep inner faces (fancy leaves).
        let out = mesh_section(
            input_with(&[(5, 5, 5, blocks::OAK_LEAVES), (5, 6, 5, blocks::OAK_LEAVES)]),
            &t,
        );
        assert_eq!(out.quads(LAYER_CUTOUT), 12);
        // A plant is two double-sided quads.
        let out = mesh_section(input_with(&[(5, 5, 5, blocks::POPPY)]), &t);
        assert_eq!(out.quads(LAYER_CUTOUT), 4);
    }

    #[test]
    fn ambient_occlusion_darkens_corners() {
        let t = tables();
        // Floor block with a wall block next to it, both on y = 5.
        let out = mesh_section(
            input_with(&[(5, 5, 5, blocks::STONE), (6, 6, 5, blocks::STONE)]),
            &t,
        );
        let words = &out.layers[LAYER_OPAQUE];
        let mut found_dark = false;
        for v in words.chunks_exact(3) {
            let normal = (v[0] >> 27) & 7;
            let ao = (v[1] >> 22) & 3;
            if normal == 0 && ao < 3 {
                found_dark = true;
            }
        }
        assert!(found_dark, "top face next to a wall should have AO");
    }

    #[test]
    fn connectivity_of_wall() {
        let t = tables();
        // A full stone wall at x = 8 splits west from east.
        let mut b = vec![];
        for y in 0..16 {
            for z in 0..16 {
                b.push((8, y, z, blocks::STONE));
            }
        }
        let out = mesh_section(input_with(&b), &t);
        let (w, e, up) = (Face::West as usize, Face::East as usize, Face::Up as usize);
        assert!(!connected(out.connectivity, w, e));
        assert!(connected(out.connectivity, w, up));
        assert!(connected(out.connectivity, e, up));
        let empty = mesh_section(input_with(&[]), &t);
        assert_eq!(empty.connectivity, ALL_CONNECTED);
    }
}
