//! Per-frame geometry built on the CPU: entities, dropped items, falling
//! blocks, the crack overlay, sun and moon, selection outlines, clouds and
//! the UI. Everything here is rebuilt (or cheaply refreshed) each frame and
//! uploaded into growable buffers.
//!
//! Model-space conventions for `FrameData` instances:
//! - `block_models` / block `items`: a unit cube centred on the origin
//!   (`-0.5..0.5`), transformed by the instance matrix;
//! - non-block `items`: a flat, double-sided icon in the XY plane
//!   (`-0.5..0.5`), facing +Z;
//! - `entities`: the posed model mesh from `mc_assets` (blocks units).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use mc_assets::ModelVertex;
use mc_core::block::Shape;
use mc_core::render_types::{DebugBox, UiDrawList};
use mc_core::{BlockId, Rgba8Image};

use crate::mesher::{MeshTables, TintKind};
use crate::textures::Tex2d;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct DynVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// sRGB tint (rgba8).
    pub color: [u8; 4],
    /// sky/15, block/15, shade, hurt (unorm8).
    pub light: [u8; 4],
    /// Block tile layer (bit 31: opaque tile, alpha is a tint mask).
    pub layer: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct CloudVertex {
    pub pos: [f32; 3],
    pub shade: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub layer: u32,
}

#[derive(Clone)]
pub enum DynKind {
    /// Textured with a 2D texture (entities, item icons).
    Tex(Arc<Tex2d>),
    /// Block tiles (block items, falling blocks).
    Block,
    /// Crack overlay (multiply blend).
    Crack,
    /// Sun / moon (additive, unlit).
    Sprite(Arc<Tex2d>),
}

impl DynKind {
    fn same(&self, o: &DynKind) -> bool {
        match (self, o) {
            (DynKind::Tex(a), DynKind::Tex(b)) | (DynKind::Sprite(a), DynKind::Sprite(b)) => {
                Arc::ptr_eq(a, b)
            }
            (DynKind::Block, DynKind::Block) | (DynKind::Crack, DynKind::Crack) => true,
            _ => false,
        }
    }
}

pub struct DynBatch {
    pub kind: DynKind,
    pub start: u32,
    pub count: u32,
}

/// Triangle list with batches in draw order.
#[derive(Default)]
pub struct DynList {
    pub verts: Vec<DynVertex>,
    pub batches: Vec<DynBatch>,
}

impl DynList {
    pub fn clear(&mut self) {
        self.verts.clear();
        self.batches.clear();
    }

    fn begin(&mut self, kind: &DynKind) {
        let start = self.verts.len() as u32;
        match self.batches.last_mut() {
            Some(b) if b.kind.same(kind) && b.start + b.count == start => {}
            _ => self.batches.push(DynBatch {
                kind: kind.clone(),
                start,
                count: 0,
            }),
        }
    }

    fn end(&mut self) {
        let len = self.verts.len() as u32;
        if let Some(b) = self.batches.last_mut() {
            b.count = len - b.start;
        }
    }

    /// Append a quad (corners counter-clockwise when seen from the front).
    pub fn quad(&mut self, kind: &DynKind, v: [DynVertex; 4]) {
        self.begin(kind);
        self.verts
            .extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
        self.end();
    }

    pub fn triangles(&mut self, kind: &DynKind, v: &[DynVertex]) {
        self.begin(kind);
        self.verts.extend_from_slice(v);
        self.end();
    }
}

/// Unpack `sky << 4 | block` into the vertex light bytes.
pub fn light_bytes(packed: u8, shade: f32, hurt: f32) -> [u8; 4] {
    [
        ((packed >> 4) as u32 * 17) as u8,
        ((packed & 15) as u32 * 17) as u8,
        (shade.clamp(0.0, 1.0) * 255.0) as u8,
        (hurt.clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

fn rgba8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

const SHADES: [f32; 6] = [1.0, 0.5, 0.8, 0.8, 0.6, 0.6];

/// Unit cube corners per face (`Face::ALL` order), CCW from outside, with
/// texture coordinates (0..1, v down).
const CUBE: [[[f32; 3]; 4]; 6] = [
    [[0., 1., 1.], [1., 1., 1.], [1., 1., 0.], [0., 1., 0.]],
    [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
    [[1., 0., 0.], [0., 0., 0.], [0., 1., 0.], [1., 1., 0.]],
    [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
    [[1., 0., 1.], [1., 0., 0.], [1., 1., 0.], [1., 1., 1.]],
    [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
];
const QUAD_UV: [[f32; 2]; 4] = [[0., 1.], [1., 1.], [1., 0.], [0., 0.]];

/// Tint colour of a block's face for drawing outside the world (plains).
fn block_tint(tables: &MeshTables, block: BlockId, face: usize) -> [u8; 4] {
    let info = &tables.info[block.0 as usize];
    let tinted = tables
        .face_tint
        .get(block.0 as usize)
        .is_some_and(|t| t[face]);
    if !tinted {
        return [255; 4];
    }
    let bt = tables.biome_tints.first().copied().unwrap_or([[255; 3]; 3]);
    let c = match info.tint {
        TintKind::None => [255; 3],
        TintKind::Grass => bt[0],
        TintKind::Foliage => bt[1],
        TintKind::Water => bt[2],
        TintKind::Fixed(c) => c,
    };
    [c[0], c[1], c[2], 255]
}

/// A block drawn as a model (dropped block item, falling block, held block).
#[allow(clippy::too_many_arguments)]
pub fn block_model(
    list: &mut DynList,
    tables: &MeshTables,
    opaque_tiles: &[bool],
    block: BlockId,
    transform: Mat4,
    cam: Vec3,
    light: u8,
    kind: &DynKind,
) {
    let Some(info) = tables.info.get(block.0 as usize) else {
        return;
    };
    let faces = tables.faces[block.0 as usize];
    let tile_layer = |t: u16| {
        let opaque = opaque_tiles.get(t as usize).copied().unwrap_or(true);
        t as u32
            | if opaque && info.layer == 0 {
                1 << 31
            } else {
                0
            }
    };
    match info.shape {
        Shape::None => {}
        Shape::Cross | Shape::Torch => {
            let layer = tile_layer(faces[2]);
            let tint = block_tint(tables, block, 2);
            for (a, b) in [
                ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                ([0.5, -0.5, -0.5], [-0.5, 0.5, 0.5]),
            ] {
                let corners = [
                    Vec3::new(a[0], -0.5, a[2]),
                    Vec3::new(b[0], -0.5, b[2]),
                    Vec3::new(b[0], 0.5, b[2]),
                    Vec3::new(a[0], 0.5, a[2]),
                ];
                for back in [false, true] {
                    let mut v = [DynVertex::default(); 4];
                    for k in 0..4 {
                        let kk = if back { [1, 0, 3, 2][k] } else { k };
                        let p = transform.transform_point3(corners[kk]) - cam;
                        v[k] = DynVertex {
                            pos: p.to_array(),
                            uv: QUAD_UV[kk],
                            color: tint,
                            light: light_bytes(light, 1.0, 0.0),
                            layer,
                        };
                    }
                    list.quad(kind, v);
                }
            }
        }
        _ => {
            let height = match info.shape {
                Shape::Layer(h) => (h as f32 / 16.0).clamp(1.0 / 16.0, 1.0),
                Shape::Liquid => 14.0 / 16.0,
                _ => 1.0,
            };
            for f in 0..6 {
                let layer = tile_layer(faces[f]);
                let tint = block_tint(tables, block, f);
                let mut v = [DynVertex::default(); 4];
                for k in 0..4 {
                    let c = CUBE[f][k];
                    let local = Vec3::new(c[0] - 0.5, c[1] * height - 0.5, c[2] - 0.5);
                    let p = transform.transform_point3(local) - cam;
                    let mut uv = QUAD_UV[k];
                    if f >= 2 && c[1] > 0.5 {
                        uv[1] = 1.0 - height;
                    }
                    v[k] = DynVertex {
                        pos: p.to_array(),
                        uv,
                        color: tint,
                        light: light_bytes(light, SHADES[f], 0.0),
                        layer,
                    };
                }
                list.quad(kind, v);
            }
        }
    }
}

/// Flat double-sided icon quad in the model's XY plane.
pub fn icon_quad(list: &mut DynList, kind: &DynKind, transform: Mat4, cam: Vec3, light: u8) {
    let corners = [
        Vec3::new(-0.5, -0.5, 0.0),
        Vec3::new(0.5, -0.5, 0.0),
        Vec3::new(0.5, 0.5, 0.0),
        Vec3::new(-0.5, 0.5, 0.0),
    ];
    for back in [false, true] {
        let mut v = [DynVertex::default(); 4];
        for k in 0..4 {
            let kk = if back { [1, 0, 3, 2][k] } else { k };
            v[k] = DynVertex {
                pos: (transform.transform_point3(corners[kk]) - cam).to_array(),
                uv: QUAD_UV[kk],
                color: [255; 4],
                light: light_bytes(light, 1.0, 0.0),
                layer: 0,
            };
        }
        list.quad(kind, v);
    }
}

/// Simple directional shading for entity models.
fn entity_shade(n: Vec3) -> f32 {
    let l1 = Vec3::new(0.2, 1.0, -0.7).normalize();
    let l2 = Vec3::new(-0.2, 1.0, 0.7).normalize();
    (0.45 + 0.45 * (n.dot(l1).max(0.0) + n.dot(l2).max(0.0))).min(1.0)
}

/// Posed model triangles → world vertices.
#[allow(clippy::too_many_arguments)]
pub fn entity_mesh(
    list: &mut DynList,
    kind: &DynKind,
    mesh: &[ModelVertex],
    transform: Mat4,
    cam: Vec3,
    tint: [f32; 4],
    light: u8,
    hurt: f32,
) {
    let normal_m = glam::Mat3::from_mat4(transform).inverse().transpose();
    let color = rgba8(tint);
    let mut out = Vec::with_capacity(mesh.len());
    for v in mesh {
        let p = transform.transform_point3(v.pos) - cam;
        let n = (normal_m * v.normal).normalize_or_zero();
        out.push(DynVertex {
            pos: p.to_array(),
            uv: v.uv.to_array(),
            color,
            light: light_bytes(light, entity_shade(n), hurt),
            layer: 0,
        });
    }
    let n = out.len() / 3 * 3;
    list.triangles(kind, &out[..n]);
}

/// A textured box used when an entity model has no geometry (yet): the
/// whole texture on every face, so the pipeline can be exercised.
pub fn fallback_box() -> Vec<ModelVertex> {
    let (lo, hi) = (Vec3::new(-0.45, 0.0, -0.45), Vec3::new(0.45, 0.9, 0.45));
    let normals = [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::NEG_Z,
        Vec3::Z,
        Vec3::X,
        Vec3::NEG_X,
    ];
    let mut out = Vec::new();
    for f in 0..6 {
        let c = CUBE[f].map(|c| lo + (hi - lo) * Vec3::from(c));
        for k in [0, 1, 2, 0, 2, 3] {
            out.push(ModelVertex {
                pos: c[k],
                normal: normals[f],
                uv: Vec2::from(QUAD_UV[k]),
            });
        }
    }
    out
}

/// Crack overlay on the block at `pos`.
pub fn crack(list: &mut DynList, pos: glam::IVec3, layer: u32, cam: Vec3) {
    let e = 0.003;
    let base = pos.as_vec3() - Vec3::splat(e);
    let size = 1.0 + 2.0 * e;
    for f in 0..6 {
        let mut v = [DynVertex::default(); 4];
        for k in 0..4 {
            let p = base + Vec3::from(CUBE[f][k]) * size - cam;
            v[k] = DynVertex {
                pos: p.to_array(),
                uv: QUAD_UV[k],
                color: [255; 4],
                light: [255, 255, 255, 0],
                layer,
            };
        }
        list.quad(&DynKind::Crack, v);
    }
}

/// Camera-facing quad at direction `dir` (unit), `size` across, far away.
pub fn sky_quad(
    list: &mut DynList,
    kind: &DynKind,
    dir: Vec3,
    size: f32,
    uv: [f32; 4],
    color: [f32; 4],
) {
    let dist = 100.0;
    let c = dir * dist;
    let right = Vec3::Z;
    let up = dir.cross(right).normalize_or_zero();
    let h = size * 0.5;
    let corners = [
        c - right * h - up * h,
        c + right * h - up * h,
        c + right * h + up * h,
        c - right * h + up * h,
    ];
    let uvs = [
        [uv[0], uv[3]],
        [uv[2], uv[3]],
        [uv[2], uv[1]],
        [uv[0], uv[1]],
    ];
    let col = rgba8(color);
    let mut v = [DynVertex::default(); 4];
    for k in 0..4 {
        v[k] = DynVertex {
            pos: corners[k].to_array(),
            uv: uvs[k],
            color: col,
            light: [255; 4],
            layer: 0,
        };
    }
    list.quad(kind, v);
}

/// Outline boxes as a line list.
pub fn box_lines(out: &mut Vec<LineVertex>, b: &DebugBox, cam: Vec3) {
    let (lo, hi) = (b.min - cam, b.max - cam);
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { hi.x } else { lo.x },
            if y { hi.y } else { lo.y },
            if z { hi.z } else { lo.z },
        )
    };
    let edges = [
        ((false, false, false), (true, false, false)),
        ((false, false, true), (true, false, true)),
        ((false, true, false), (true, true, false)),
        ((false, true, true), (true, true, true)),
        ((false, false, false), (false, true, false)),
        ((true, false, false), (true, true, false)),
        ((false, false, true), (false, true, true)),
        ((true, false, true), (true, true, true)),
        ((false, false, false), (false, false, true)),
        ((true, false, false), (true, false, true)),
        ((false, true, false), (false, true, true)),
        ((true, true, false), (true, true, true)),
    ];
    for (a, b2) in edges {
        out.push(LineVertex {
            pos: c(a.0, a.1, a.2).to_array(),
            color: b.color,
        });
        out.push(LineVertex {
            pos: c(b2.0, b2.1, b2.2).to_array(),
            color: b.color,
        });
    }
}

// ------------------------------------------------------------------ clouds

pub const CLOUD_CELL: f32 = 12.0;
pub const CLOUD_HEIGHT: f32 = 192.0;
pub const CLOUD_THICKNESS: f32 = 4.0;

/// Cloud occupancy map from `textures/environment/clouds.png`.
pub struct CloudMap {
    pub w: i32,
    pub h: i32,
    pub cells: Vec<bool>,
}

impl CloudMap {
    pub fn new(img: Option<&Rgba8Image>) -> Self {
        match img {
            Some(img) if img.width > 0 && img.height > 0 => CloudMap {
                w: img.width as i32,
                h: img.height as i32,
                cells: img.data.chunks_exact(4).map(|p| p[3] > 127).collect(),
            },
            _ => CloudMap {
                w: 1,
                h: 1,
                cells: vec![false],
            },
        }
    }

    #[inline]
    pub fn filled(&self, x: i32, z: i32) -> bool {
        self.cells[(z.rem_euclid(self.h) * self.w + x.rem_euclid(self.w)) as usize]
    }

    /// Boxy clouds for cells `(cx0..cx0+n, cz0..cz0+n)`, positions relative
    /// to the corner of cell `(cx0, cz0)` at the cloud base. Faces between
    /// neighbouring filled cells are skipped; tops and bottoms are greedily
    /// merged into rectangles (at most 8×8 cells, so the per-vertex fog
    /// stays smooth) and side faces into runs. Quads, 4 vertices each.
    pub fn mesh(&self, cx0: i32, cz0: i32, n: i32, out: &mut Vec<CloudVertex>) {
        out.clear();
        let s = CLOUD_CELL;
        let t = CLOUD_THICKNESS;
        let nu = n as usize;
        let filled: Vec<bool> = (0..n * n)
            .map(|k| self.filled(cx0 + k % n, cz0 + k / n))
            .collect();
        let at = |i: i32, j: i32| -> bool {
            if i >= 0 && j >= 0 && i < n && j < n {
                filled[(j * n + i) as usize]
            } else {
                self.filled(cx0 + i, cz0 + j)
            }
        };
        let mut q = |p: [[f32; 3]; 4], shade: f32| {
            for c in p {
                out.push(CloudVertex { pos: c, shade });
            }
        };
        // Tops and bottoms.
        let mut used = vec![false; nu * nu];
        for j in 0..nu {
            for i in 0..nu {
                if used[j * nu + i] || !filled[j * nu + i] {
                    continue;
                }
                let mut w = 1;
                while i + w < nu && w < 8 && filled[j * nu + i + w] && !used[j * nu + i + w] {
                    w += 1;
                }
                let mut h = 1;
                'rows: while j + h < nu && h < 8 {
                    for c in 0..w {
                        let k = (j + h) * nu + i + c;
                        if !filled[k] || used[k] {
                            break 'rows;
                        }
                    }
                    h += 1;
                }
                for r in 0..h {
                    for c in 0..w {
                        used[(j + r) * nu + i + c] = true;
                    }
                }
                let (x0, z0) = (i as f32 * s, j as f32 * s);
                let (x1, z1) = (x0 + w as f32 * s, z0 + h as f32 * s);
                q([[x0, t, z1], [x1, t, z1], [x1, t, z0], [x0, t, z0]], 1.0);
                q(
                    [[x0, 0., z0], [x1, 0., z0], [x1, 0., z1], [x0, 0., z1]],
                    0.7,
                );
            }
        }
        // Side faces, merged into runs along each edge line.
        for j in 0..n {
            for (dz, shade) in [(-1, 0.8), (1, 0.8)] {
                let mut i = 0;
                while i < n {
                    if !(at(i, j) && !at(i, j + dz)) {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < n && i - start < 8 && at(i, j) && !at(i, j + dz) {
                        i += 1;
                    }
                    let (xa, xb) = (start as f32 * s, i as f32 * s);
                    let z = if dz < 0 {
                        j as f32 * s
                    } else {
                        (j + 1) as f32 * s
                    };
                    if dz < 0 {
                        q([[xb, 0., z], [xa, 0., z], [xa, t, z], [xb, t, z]], shade);
                    } else {
                        q([[xa, 0., z], [xb, 0., z], [xb, t, z], [xa, t, z]], shade);
                    }
                }
            }
        }
        for i in 0..n {
            for (dx, shade) in [(-1, 0.9), (1, 0.9)] {
                let mut j = 0;
                while j < n {
                    if !(at(i, j) && !at(i + dx, j)) {
                        j += 1;
                        continue;
                    }
                    let start = j;
                    while j < n && j - start < 8 && at(i, j) && !at(i + dx, j) {
                        j += 1;
                    }
                    let (za, zb) = (start as f32 * s, j as f32 * s);
                    let x = if dx < 0 {
                        i as f32 * s
                    } else {
                        (i + 1) as f32 * s
                    };
                    if dx < 0 {
                        q([[x, 0., za], [x, 0., zb], [x, t, zb], [x, t, za]], shade);
                    } else {
                        q([[x, 0., zb], [x, 0., za], [x, t, za], [x, t, zb]], shade);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ UI

#[derive(Clone)]
pub enum UiKind {
    Tex(Arc<Tex2d>),
    Blocks,
}

pub struct UiBatch {
    pub kind: UiKind,
    pub start: u32,
    pub count: u32,
}

/// Turn the UI draw list into triangles, batching consecutive quads that
/// share a texture (painter's order is kept).
pub fn build_ui(
    ui: &UiDrawList,
    mut texture: impl FnMut(&str) -> Arc<Tex2d>,
    dim: Option<(f32, f32)>,
    verts: &mut Vec<UiVertex>,
    batches: &mut Vec<UiBatch>,
) {
    verts.clear();
    batches.clear();
    let push = |kind: UiKind,
                pos: [Vec2; 4],
                uv: [Vec2; 4],
                color: [f32; 4],
                layer: u32,
                verts: &mut Vec<UiVertex>,
                batches: &mut Vec<UiBatch>| {
        let start = verts.len() as u32;
        let same = match (batches.last(), &kind) {
            (
                Some(UiBatch {
                    kind: UiKind::Tex(a),
                    ..
                }),
                UiKind::Tex(b),
            ) => Arc::ptr_eq(a, b),
            (
                Some(UiBatch {
                    kind: UiKind::Blocks,
                    ..
                }),
                UiKind::Blocks,
            ) => true,
            _ => false,
        };
        if !same {
            batches.push(UiBatch {
                kind,
                start,
                count: 0,
            });
        }
        for k in [0, 1, 2, 0, 2, 3] {
            verts.push(UiVertex {
                pos: pos[k].to_array(),
                uv: uv[k].to_array(),
                color,
                layer,
            });
        }
        batches.last_mut().unwrap().count += 6;
    };
    if let Some((w, h)) = dim {
        let white = texture("@white");
        push(
            UiKind::Tex(white),
            [
                Vec2::ZERO,
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ],
            [Vec2::ZERO; 4],
            [0.02, 0.02, 0.03, 0.55],
            0,
            verts,
            batches,
        );
    }
    for q in &ui.quads {
        let kind = if q.texture.as_str() == "@blocks" {
            UiKind::Blocks
        } else {
            UiKind::Tex(texture(q.texture.as_str()))
        };
        push(kind, q.pos, q.uv, q.color, q.layer, verts, batches);
    }
}
