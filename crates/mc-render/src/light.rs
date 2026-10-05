//! Light engine: sky light and block light, stored per voxel as
//! `sky << 4 | block` in each `Section` (see `mc_core::chunk`).
//!
//! - **Full lighting** of a chunk runs on a worker thread. The flood fill
//!   covers the chunk and its eight neighbours (a 48×48 column region), so
//!   it is exact: light never travels more than 15 blocks, and the
//!   neighbours' blocks are all it depends on. The result is written into
//!   the centre chunk only.
//! - **Incremental updates** after block edits (`World::take_dirty_blocks`)
//!   run on the main thread: a removal BFS clears light that depended on the
//!   changed voxel, then a propagation BFS refills from the boundary.
//!
//! Propagation rules: every step costs `max(1, opacity)` levels; opaque
//! blocks stop light; sky light at level 15 travels straight down through
//! fully transparent blocks without loss; leaves, fluids and ice have
//! opacity 1 so they dim sky light by one level per block.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use glam::IVec3;
use mc_core::chunk::SECTION_VOLUME;
use mc_core::{BlockId, Chunk, ChunkPos, SECTION_COUNT, Section, WORLD_MAX_Y, WORLD_MIN_Y, World};
use rustc_hash::FxHashSet;

/// Per-block light properties.
pub struct LightTables {
    pub opacity: Vec<u8>,
    pub emit: Vec<u8>,
}

pub fn tables() -> &'static LightTables {
    static T: OnceLock<LightTables> = OnceLock::new();
    T.get_or_init(|| {
        let mut opacity = Vec::new();
        let mut emit = Vec::new();
        for id in BlockId::all() {
            let d = id.def();
            let op = if d.opaque {
                15
            } else if d.fluid
                || d.name.ends_with("_leaves")
                || d.name == "ice"
                || d.name == "cobweb"
            {
                1
            } else {
                0
            };
            opacity.push(op);
            emit.push(d.light.min(15));
        }
        LightTables { opacity, emit }
    })
}

#[inline]
fn opacity_of(t: &LightTables, id: BlockId) -> u8 {
    t.opacity.get(id.0 as usize).copied().unwrap_or(15)
}

#[inline]
fn emit_of(t: &LightTables, id: BlockId) -> u8 {
    t.emit.get(id.0 as usize).copied().unwrap_or(0)
}

/// Everything a full-light job needs: Arc snapshots of a 3×3 chunk area.
pub struct LightInput {
    pub center: ChunkPos,
    /// Index `(dz + 1) * 3 + (dx + 1)`.
    pub sections: [[Arc<Section>; SECTION_COUNT]; 9],
    /// Highest non-air block y per chunk.
    pub top: [i32; 9],
    /// Block revision per chunk when the snapshot was taken (the caller
    /// discards the result if any of them changed meanwhile).
    pub block_revs: [u64; 9],
}

pub struct LightOutput {
    pub center: ChunkPos,
    pub block_revs: [u64; 9],
    /// Packed light per section; `None` = all `0xF0` (open sky, no block light).
    pub sections: Vec<Option<Box<[u8; SECTION_VOLUME]>>>,
    pub micros: u32,
}

/// Snapshot the 3×3 area around `center`. Returns `None` if a neighbour is
/// not loaded. `block_rev` maps a chunk to its block-only revision.
pub fn snapshot(
    world: &World,
    center: ChunkPos,
    block_rev: impl Fn(&Chunk) -> u64,
) -> Option<LightInput> {
    let mut chunks: [Option<&Chunk>; 9] = [None; 9];
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks[((dz + 1) * 3 + dx + 1) as usize] = Some(world.chunk(center.offset(dx, dz))?);
        }
    }
    let chunks = chunks.map(|c| c.unwrap());
    Some(LightInput {
        center,
        sections: chunks.map(|c| c.sections.clone()),
        top: chunks.map(|c| c.heightmap.iter().copied().max().unwrap_or(0) as i32),
        block_revs: chunks.map(&block_rev),
    })
}

const W: usize = 48;

#[derive(Default)]
struct Scratch {
    opac: Vec<u8>,
    sky: Vec<u8>,
    blk: Vec<u8>,
    queue: Vec<u32>,
    b15: Vec<u16>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

/// Compute light for `input.center` (pure function, runs on workers).
pub fn compute(input: &LightInput) -> LightOutput {
    let t0 = web_time_now();
    let out = SCRATCH.with(|s| compute_with(&mut s.borrow_mut(), input));
    LightOutput {
        micros: elapsed_micros(t0),
        ..out
    }
}

fn compute_with(s: &mut Scratch, input: &LightInput) -> LightOutput {
    let t = tables();
    let max_top = input.top.iter().copied().max().unwrap_or(WORLD_MIN_Y);
    // Above `top` every voxel is open sky with no block light (emitters at
    // the highest block reach at most 14 levels up).
    let top = (max_top + 17).clamp(WORLD_MIN_Y + 16, WORLD_MAX_Y);
    let nsec = (((top - WORLD_MIN_Y) + 15) / 16) as usize;
    let h = nsec * 16;
    let n = W * W * h;
    s.opac.clear();
    s.opac.resize(n, 0);
    s.sky.clear();
    s.sky.resize(n, 0);
    s.blk.clear();
    s.blk.resize(n, 0);
    s.queue.clear();

    // Opacity + emitters.
    for ci in 0..9 {
        let (ox, oz) = ((ci % 3) * 16, (ci / 3) * 16);
        for (sy, sec) in input.sections[ci].iter().enumerate().take(nsec) {
            let Some(blocks) = sec.blocks() else { continue };
            for ly in 0..16 {
                let y = sy * 16 + ly;
                for lz in 0..16 {
                    let row = (y * W + oz + lz) * W + ox;
                    let src = (ly << 8) | (lz << 4);
                    for lx in 0..16 {
                        let id = blocks[src | lx];
                        if id.0 == 0 {
                            continue;
                        }
                        let i = row + lx;
                        s.opac[i] = opacity_of(t, id);
                        let e = emit_of(t, id);
                        if e > 0 {
                            s.blk[i] = e;
                            s.queue.push(i as u32);
                        }
                    }
                }
            }
        }
    }

    // Block light flood fill.
    flood(&mut s.blk, &s.opac, &mut s.queue, h, false);

    // Sky: vertical pass, remembering where full sky light ends per column.
    s.b15.clear();
    s.b15.resize(W * W, 0);
    for z in 0..W {
        for x in 0..W {
            let mut level = 15u8;
            let mut b15 = h;
            for y in (0..h).rev() {
                let i = (y * W + z) * W + x;
                let op = s.opac[i];
                level = if op >= 15 {
                    0
                } else if op == 0 && level == 15 {
                    15
                } else {
                    level.saturating_sub(op.max(1))
                };
                s.sky[i] = level;
                if level == 15 {
                    b15 = y;
                } else if level == 0 {
                    break;
                }
            }
            s.b15[z * W + x] = b15 as u16;
        }
    }
    // Seeds: sky-15 voxels that have a darker horizontal neighbour, plus the
    // dimmed voxels under leaves/water.
    s.queue.clear();
    for z in 0..W {
        for x in 0..W {
            let own = s.b15[z * W + x] as usize;
            let mut hi = own;
            if x > 0 {
                hi = hi.max(s.b15[z * W + x - 1] as usize);
            }
            if x + 1 < W {
                hi = hi.max(s.b15[z * W + x + 1] as usize);
            }
            if z > 0 {
                hi = hi.max(s.b15[(z - 1) * W + x] as usize);
            }
            if z + 1 < W {
                hi = hi.max(s.b15[(z + 1) * W + x] as usize);
            }
            for y in own..hi.min(h) {
                s.queue.push(((y * W + z) * W + x) as u32);
            }
            let mut y = own;
            while y > 0 {
                y -= 1;
                let i = (y * W + z) * W + x;
                if s.sky[i] <= 1 {
                    break;
                }
                s.queue.push(i as u32);
            }
        }
    }
    flood(&mut s.sky, &s.opac, &mut s.queue, h, true);

    // Pack the centre chunk.
    let mut sections = Vec::with_capacity(SECTION_COUNT);
    for sy in 0..SECTION_COUNT {
        if sy >= nsec {
            sections.push(None);
            continue;
        }
        let mut arr = Box::new([0u8; SECTION_VOLUME]);
        let mut all_sky = true;
        for ly in 0..16 {
            let y = sy * 16 + ly;
            for lz in 0..16 {
                let row = (y * W + 16 + lz) * W + 16;
                let dst = (ly << 8) | (lz << 4);
                for lx in 0..16 {
                    let v = (s.sky[row + lx] << 4) | s.blk[row + lx];
                    all_sky &= v == 0xF0;
                    arr[dst | lx] = v;
                }
            }
        }
        sections.push(if all_sky { None } else { Some(arr) });
    }
    LightOutput {
        center: input.center,
        block_revs: input.block_revs,
        sections,
        micros: 0,
    }
}

/// BFS flood over the 48×h×48 region from the voxels in `queue`.
fn flood(light: &mut [u8], opac: &[u8], queue: &mut Vec<u32>, h: usize, sky: bool) {
    let mut head = 0;
    while head < queue.len() {
        let i = queue[head] as usize;
        head += 1;
        let l = light[i];
        if l <= 1 {
            continue;
        }
        let x = i % W;
        let z = (i / W) % W;
        let y = i / (W * W);
        let mut visit = |j: usize, down: bool| {
            let op = opac[j];
            if op >= 15 {
                return;
            }
            let nl = if sky && down && l == 15 && op == 0 {
                15
            } else {
                l.saturating_sub(op.max(1))
            };
            if nl > light[j] {
                light[j] = nl;
                queue.push(j as u32);
            }
        };
        if x > 0 {
            visit(i - 1, false);
        }
        if x + 1 < W {
            visit(i + 1, false);
        }
        if z > 0 {
            visit(i - W, false);
        }
        if z + 1 < W {
            visit(i + W, false);
        }
        if y > 0 {
            visit(i - W * W, true);
        }
        if y + 1 < h {
            visit(i + W * W, false);
        }
    }
    queue.clear();
}

/// Write a finished light result into its chunk. Sections whose light did
/// not change are left alone (so their meshes stay valid).
pub fn apply(chunk: &mut Chunk, out: &LightOutput) {
    for (sy, data) in out.sections.iter().enumerate() {
        let sec = &chunk.sections[sy];
        match data {
            Some(arr) => {
                if sec.light_data().is_some_and(|l| l[..] == arr[..]) {
                    continue;
                }
                Arc::make_mut(&mut chunk.sections[sy])
                    .light_data_mut()
                    .copy_from_slice(&arr[..]);
            }
            None => {
                if sec
                    .light_data()
                    .is_some_and(|l| l.iter().any(|&v| v != 0xF0))
                {
                    Arc::make_mut(&mut chunk.sections[sy])
                        .light_data_mut()
                        .fill(0xF0);
                }
            }
        }
    }
    chunk.light_ready = true;
}

// ---------------------------------------------------------------------------
// Incremental updates
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Sky,
    Block,
}

const DIRS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
];
const DOWN: usize = 5;

struct Access<'a> {
    world: &'a mut World,
    touched: &'a mut FxHashSet<ChunkPos>,
}

impl Access<'_> {
    /// Voxel is inside a loaded, already-lit chunk.
    #[inline]
    fn ready(&self, p: IVec3) -> bool {
        if p.y < WORLD_MIN_Y || p.y >= WORLD_MAX_Y {
            return false;
        }
        self.world
            .chunk(ChunkPos::new(p.x >> 4, p.z >> 4))
            .is_some_and(|c| c.light_ready)
    }
    #[inline]
    fn get(&self, p: IVec3, ch: Channel) -> u8 {
        let l = self.world.light(p);
        match ch {
            Channel::Sky => l >> 4,
            Channel::Block => l & 15,
        }
    }
    fn set(&mut self, p: IVec3, ch: Channel, v: u8) {
        let (cp, l) = Chunk::split_pos(p);
        if let Some(c) = self.world.chunk_mut(cp) {
            let old = c.light(l.x, l.y, l.z);
            let new = match ch {
                Channel::Sky => (v << 4) | (old & 15),
                Channel::Block => (old & 0xF0) | v,
            };
            if new != old {
                c.set_light(l.x, l.y, l.z, new);
                self.touched.insert(cp);
            }
        }
    }
    #[inline]
    fn opacity(&self, p: IVec3) -> u8 {
        opacity_of(tables(), self.world.block(p))
    }
    #[inline]
    fn emit(&self, p: IVec3) -> u8 {
        emit_of(tables(), self.world.block(p))
    }
}

/// Update light around changed blocks. Returns the chunks whose light
/// changed. Voxels in unloaded or not-yet-lit chunks are treated as
/// boundaries (those chunks get a full computation later).
pub fn update_blocks(world: &mut World, dirty: &[IVec3]) -> FxHashSet<ChunkPos> {
    let mut touched = FxHashSet::default();
    if dirty.is_empty() {
        return touched;
    }
    let mut a = Access {
        world,
        touched: &mut touched,
    };
    let mut rem: Vec<(IVec3, u8)> = Vec::new();
    let mut add: VecDeque<IVec3> = VecDeque::new();
    for ch in [Channel::Sky, Channel::Block] {
        for &p in dirty {
            if !a.ready(p) {
                continue;
            }
            let old = a.get(p, ch);
            let e = match ch {
                Channel::Block => a.emit(p),
                Channel::Sky => {
                    if p.y == WORLD_MAX_Y - 1 && a.opacity(p) == 0 {
                        15
                    } else {
                        0
                    }
                }
            };
            a.set(p, ch, e);
            if old > 0 {
                rem.push((p, old));
            }
            if e > 0 {
                add.push_back(p);
            }
            for d in DIRS {
                let n = p + d;
                if a.ready(n) && a.get(n, ch) > 0 {
                    add.push_back(n);
                }
            }
        }
        // Removal: clear everything that was lit through the changed voxels.
        while let Some((q, lvl)) = rem.pop() {
            for (di, d) in DIRS.iter().enumerate() {
                let n = q + *d;
                if !a.ready(n) {
                    continue;
                }
                let nl = a.get(n, ch);
                if nl == 0 {
                    continue;
                }
                let dependent =
                    nl < lvl || (ch == Channel::Sky && di == DOWN && lvl == 15 && nl == 15);
                if dependent {
                    let e = if ch == Channel::Block { a.emit(n) } else { 0 };
                    a.set(n, ch, e);
                    rem.push((n, nl));
                    if e > 0 {
                        add.push_back(n);
                    }
                } else {
                    add.push_back(n);
                }
            }
        }
        // Propagation from the boundary and new sources.
        while let Some(q) = add.pop_front() {
            if !a.ready(q) {
                continue;
            }
            let l = a.get(q, ch);
            if l <= 1 {
                continue;
            }
            for (di, d) in DIRS.iter().enumerate() {
                let n = q + *d;
                if !a.ready(n) {
                    continue;
                }
                let op = a.opacity(n);
                if op >= 15 {
                    continue;
                }
                let nl = if ch == Channel::Sky && di == DOWN && l == 15 && op == 0 {
                    15
                } else {
                    l.saturating_sub(op.max(1))
                };
                if nl > a.get(n, ch) {
                    a.set(n, ch, nl);
                    add.push_back(n);
                }
            }
        }
    }
    touched
}

// Small time helpers (std::time::Instant is unavailable on wasm32-unknown-unknown).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn web_time_now() -> std::time::Instant {
    std::time::Instant::now()
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn elapsed_micros(t: std::time::Instant) -> u32 {
    t.elapsed().as_micros().min(u32::MAX as u128) as u32
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn web_time_now() {}
#[cfg(target_arch = "wasm32")]
pub(crate) fn elapsed_micros(_: ()) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::blocks;

    fn flat_world(radius: i32) -> World {
        let mut w = World::new(1);
        for cz in -radius..=radius {
            for cx in -radius..=radius {
                let mut c = Chunk::new(ChunkPos::new(cx, cz));
                for z in 0..16 {
                    for x in 0..16 {
                        for y in 50..=60 {
                            c.set_raw(x, y, z, blocks::STONE);
                        }
                    }
                }
                c.recompute_heightmap();
                w.insert_chunk(c);
            }
        }
        w
    }

    fn light_all(w: &mut World, radius: i32) {
        let mut outs = vec![];
        for cz in -radius..=radius {
            for cx in -radius..=radius {
                let inp = snapshot(w, ChunkPos::new(cx, cz), |c| c.revision).unwrap();
                outs.push(compute(&inp));
            }
        }
        for o in outs {
            apply(w.chunk_mut(o.center).unwrap(), &o);
        }
    }

    #[test]
    fn open_sky_and_ground() {
        let mut w = flat_world(2);
        light_all(&mut w, 1);
        assert_eq!(w.light(IVec3::new(5, 61, 5)) >> 4, 15);
        assert_eq!(w.light(IVec3::new(5, 200, 5)) >> 4, 15);
        assert_eq!(w.light(IVec3::new(5, 55, 5)), 0);
        assert_eq!(w.light(IVec3::new(5, 40, 5)) >> 4, 0);
    }

    #[test]
    fn tunnel_and_torch() {
        let mut w = flat_world(2);
        // Vertical shaft at (2, 51..=60, 2) and a tunnel along +x at y = 51.
        for y in 51..=60 {
            w.set_block(IVec3::new(2, y, 2), blocks::AIR);
        }
        for x in 3..12 {
            w.set_block(IVec3::new(x, 51, 2), blocks::AIR);
        }
        w.set_block(IVec3::new(11, 51, 2), blocks::TORCH);
        w.take_dirty_blocks();
        light_all(&mut w, 1);
        assert_eq!(w.light(IVec3::new(2, 51, 2)) >> 4, 15);
        assert_eq!(w.light(IVec3::new(3, 51, 2)) >> 4, 14);
        assert_eq!(w.light(IVec3::new(6, 51, 2)) >> 4, 11);
        assert_eq!(w.light(IVec3::new(11, 51, 2)) & 15, 14);
        assert_eq!(w.light(IVec3::new(9, 51, 2)) & 15, 12);
    }

    /// Incremental updates must agree with a full recomputation.
    #[test]
    fn incremental_matches_full() {
        let mut w = flat_world(2);
        light_all(&mut w, 1);
        // Pseudo-random edits inside the centre chunk.
        let mut seed = 12345u32;
        let mut rnd = |n: i32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 8) % n as u32) as i32
        };
        let palette = [
            blocks::AIR,
            blocks::STONE,
            blocks::GLASS,
            blocks::TORCH,
            blocks::OAK_LEAVES,
            blocks::WATER,
            blocks::GLOWSTONE,
        ];
        for round in 0..40 {
            let p = IVec3::new(rnd(16), 48 + rnd(18), rnd(16));
            let b = palette[rnd(palette.len() as i32) as usize];
            w.set_block(p, b);
            // Also carve some caves so light has interesting paths.
            if round % 3 == 0 {
                for dy in 0..3 {
                    w.set_block(p - IVec3::new(0, dy + 1, 0), blocks::AIR);
                }
            }
            let dirty = w.take_dirty_blocks();
            update_blocks(&mut w, &dirty);
        }
        let incremental: Vec<u8> = (0..16)
            .flat_map(|z| (40..80).flat_map(move |y| (0..16).map(move |x| IVec3::new(x, y, z))))
            .map(|p| w.light(p))
            .collect();
        let inp = snapshot(&w, ChunkPos::new(0, 0), |c| c.revision).unwrap();
        let out = compute(&inp);
        apply(w.chunk_mut(ChunkPos::new(0, 0)).unwrap(), &out);
        let full: Vec<u8> = (0..16)
            .flat_map(|z| (40..80).flat_map(move |y| (0..16).map(move |x| IVec3::new(x, y, z))))
            .map(|p| w.light(p))
            .collect();
        let diffs = incremental
            .iter()
            .zip(&full)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            diffs, 0,
            "incremental light differs from full in {diffs} voxels"
        );
    }
}
