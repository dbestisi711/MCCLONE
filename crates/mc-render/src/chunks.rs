//! Section mesh management: change detection, background meshing with a
//! per-frame upload budget, GPU storage, and visibility (frustum + cave
//! culling).
//!
//! Change detection is two-level. `Chunk::revision` is compared every frame
//! as a cheap filter; when it moved (for a chunk or a neighbour) each section
//! compares the `Arc` identity of itself and its 26 neighbours against the
//! snapshot its current mesh was built from. Because this module keeps those
//! snapshot `Arc`s alive, any write to a section in the world goes through
//! `Arc::make_mut`'s copy and gets a new identity, so the comparison is
//! exact (block edits *and* light changes) and only touched sections are
//! remeshed. The old mesh stays on screen until the new one is uploaded.

use std::collections::VecDeque;
use std::sync::Arc;

use glam::{Vec3, Vec4};
use mc_core::{BiomeId, ChunkPos, SECTION_COUNT, Section, WORLD_MIN_Y, World};
use rustc_hash::FxHashMap as HashMap;

use crate::arena::{Alloc, Arena};
use crate::mesher::{self, ALL_CONNECTED, MeshInput, MeshOutput, MeshTables};
use crate::tasks::{Jobs, Priority};

type Snapshot = [Option<Arc<Section>>; 27];

#[derive(Default)]
struct SectionState {
    gpu: [Option<(Alloc, u32)>; 3],
    conn: u64,
    /// `conn` is known (meshed or known empty).
    known: bool,
    /// Sections the current mesh was built from (kept alive on purpose).
    held: Option<Box<Snapshot>>,
    generation: u64,
    pending: bool,
    queued: bool,
}

struct ChunkState {
    last_rev: u64,
    biomes: Arc<[BiomeId; 256]>,
    sections: Vec<SectionState>,
    check: bool,
}

/// One section to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    /// Camera-relative section origin.
    pub origin: Vec4,
    pub dist2: f32,
    /// Per layer: (page, first quad, quads).
    pub layers: [Option<(u32, u32, u32)>; 3],
}

/// Camera frustum planes in camera-relative space.
pub struct Frustum {
    planes: [Vec4; 5],
}

impl Frustum {
    /// From a finite-depth view-projection matrix (camera-relative view).
    pub fn from_matrix(m: glam::Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        let norm = |p: Vec4| p / p.truncate().length().max(1e-6);
        Frustum {
            planes: [
                norm(r3 + r0),
                norm(r3 - r0),
                norm(r3 + r1),
                norm(r3 - r1),
                norm(r2),
            ],
        }
    }

    /// AABB (camera-relative) touches the frustum.
    #[inline]
    pub fn aabb(&self, min: Vec3, max: Vec3) -> bool {
        for p in &self.planes {
            let v = Vec3::new(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            if p.truncate().dot(v) + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ChunkStats {
    pub sections_meshed_total: u64,
    pub mesh_micros_total: u64,
    pub uploaded_this_frame: u32,
    pub upload_bytes_this_frame: u64,
    pub queued: u32,
    pub in_flight: u32,
    pub ready: u32,
    pub visible: u32,
    pub visited: u32,
    pub frustum_culled: u32,
    pub sections_with_mesh: u32,
    pub quads_total: u64,
    /// The visibility pass reached open sky (top of the world or the edge
    /// of the render distance); when false the sky and clouds are skipped.
    pub sky_visible: bool,
}

pub struct CullSettings {
    pub frustum: bool,
    pub occlusion: bool,
}

pub struct ChunkMeshes {
    pub tables: Arc<MeshTables>,
    chunks: HashMap<ChunkPos, ChunkState>,
    jobs: Jobs<MeshOutput>,
    ready: VecDeque<MeshOutput>,
    queue: Vec<(ChunkPos, u8)>,
    generation: u64,
    pub arena: Arena,
    pub stats: ChunkStats,
    /// Max mesh jobs in flight.
    pub max_in_flight: usize,
    /// Bytes uploaded per frame before the rest waits for the next frame.
    pub upload_budget: u64,
    /// Camera chunk and render distance. A chunk waits for its neighbours'
    /// light before its first mesh (avoids meshing twice), except for
    /// neighbours beyond this radius, which will never be lit.
    pub frontier: Option<(ChunkPos, i32)>,
    visit_stamp: Vec<u32>,
    stamp: u32,
    to_check: Vec<ChunkPos>,
}

const NEIGHBOURS8: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Face direction vectors in `Face::ALL` order (Up, Down, North, South, East, West).
const DIRS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (1, 0, 0),
    (-1, 0, 0),
];

impl ChunkMeshes {
    pub fn new(tables: Arc<MeshTables>, max_buffer_size: u64) -> Self {
        ChunkMeshes {
            tables,
            chunks: HashMap::default(),
            jobs: Jobs::new(),
            ready: VecDeque::new(),
            queue: Vec::new(),
            generation: 0,
            arena: Arena::new(max_buffer_size),
            stats: ChunkStats::default(),
            max_in_flight: crate::tasks::pool().threads() * 24,
            upload_budget: 6 << 20,
            frontier: None,
            visit_stamp: Vec::new(),
            stamp: 0,
            to_check: Vec::new(),
        }
    }

    fn free_section(arena: &mut Arena, s: &mut SectionState) {
        for g in s.gpu.iter_mut() {
            if let Some((a, _)) = g.take() {
                arena.free(a);
            }
        }
    }

    /// Detect changes, schedule mesh jobs, upload finished meshes.
    /// `unlimited` disables the per-frame budgets (blocking preparation).
    pub fn update(
        &mut self,
        world: &World,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cam: Vec3,
        frustum: Option<&Frustum>,
        unlimited: bool,
    ) {
        self.detect_changes(world);
        self.check_sections(world);
        self.schedule(world, cam, frustum, unlimited);
        self.apply_results(device, queue, unlimited);
        self.stats.queued = self.queue.len() as u32;
        self.stats.in_flight = self.jobs.in_flight() as u32;
        self.stats.ready = self.ready.len() as u32;
    }

    /// Nothing queued, running or waiting for upload.
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.jobs.in_flight() == 0 && self.ready.is_empty()
    }

    /// Block until at least one mesh job finishes (for blocking preparation).
    pub fn wait_one(&mut self) {
        if let Some(r) = self.jobs.recv_blocking() {
            self.ready.push_back(r);
        }
    }

    fn mark(&mut self, p: ChunkPos) {
        if let Some(st) = self.chunks.get_mut(&p)
            && !st.check
        {
            st.check = true;
            self.to_check.push(p);
        }
    }

    fn mark_with_neighbours(&mut self, p: ChunkPos) {
        self.mark(p);
        for (dx, dz) in NEIGHBOURS8 {
            self.mark(p.offset(dx, dz));
        }
    }

    fn detect_changes(&mut self, world: &World) {
        let mut changed: Vec<ChunkPos> = Vec::new();
        for c in world.chunks() {
            match self.chunks.get_mut(&c.pos) {
                Some(st) => {
                    if st.last_rev != c.revision {
                        st.last_rev = c.revision;
                        changed.push(c.pos);
                    }
                }
                None => {
                    self.chunks.insert(
                        c.pos,
                        ChunkState {
                            last_rev: c.revision,
                            biomes: Arc::new(*c.biomes),
                            sections: (0..SECTION_COUNT)
                                .map(|_| SectionState::default())
                                .collect(),
                            check: false,
                        },
                    );
                    changed.push(c.pos);
                }
            }
        }
        if self.chunks.len() != world.chunk_count() {
            let arena = &mut self.arena;
            let mut removed = Vec::new();
            self.chunks.retain(|p, st| {
                let keep = world.has_chunk(*p);
                if !keep {
                    for s in st.sections.iter_mut() {
                        Self::free_section(arena, s);
                    }
                    removed.push(*p);
                }
                keep
            });
            changed.extend(removed);
        }
        for p in changed {
            self.mark_with_neighbours(p);
        }
    }

    fn check_sections(&mut self, world: &World) {
        let list = std::mem::take(&mut self.to_check);
        for pos in list {
            let Some(st) = self.chunks.get_mut(&pos) else {
                continue;
            };
            st.check = false;
            let Some(chunk) = world.chunk(pos) else {
                continue;
            };
            let mut cols: [Option<&mc_core::Chunk>; 9] = [None; 9];
            let mut meshable = chunk.light_ready;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let np = pos.offset(dx, dz);
                    let c = world.chunk(np);
                    meshable &= c.is_some();
                    if let (Some(c), Some((cc, r))) = (c, self.frontier)
                        && !c.light_ready
                        && np.chebyshev(cc) <= r
                    {
                        meshable = false;
                    }
                    cols[((dz + 1) * 3 + dx + 1) as usize] = c;
                }
            }
            if !meshable {
                continue;
            }
            for sy in 0..SECTION_COUNT {
                let s = &mut st.sections[sy];
                if s.pending {
                    continue;
                }
                let same = match &s.held {
                    Some(h) => (0..27).all(|i| {
                        let cur = section_at(&cols, sy, i);
                        match (&h[i], cur) {
                            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                            (None, None) => true,
                            _ => false,
                        }
                    }),
                    None => false,
                };
                if same {
                    continue;
                }
                let center_empty = chunk.sections[sy].is_empty();
                if center_empty {
                    // Nothing to draw; remember the snapshot so later edits
                    // are noticed. Drop any outdated mesh.
                    Self::free_section(&mut self.arena, s);
                    s.conn = ALL_CONNECTED;
                    s.known = true;
                    s.generation += 1;
                    s.held = Some(Box::new(std::array::from_fn(|i| {
                        section_at(&cols, sy, i).cloned()
                    })));
                    continue;
                }
                if !s.queued {
                    s.queued = true;
                    self.queue.push((pos, sy as u8));
                }
            }
        }
    }

    fn schedule(&mut self, world: &World, cam: Vec3, frustum: Option<&Frustum>, unlimited: bool) {
        if self.queue.is_empty() {
            return;
        }
        let cap = if unlimited {
            crate::tasks::pool().threads() * 32
        } else {
            self.max_in_flight
        };
        let free = cap.saturating_sub(self.jobs.in_flight());
        if free == 0 {
            return;
        }
        // Priority: distance, with sections in view first.
        let mut scored: Vec<(f32, ChunkPos, u8)> = Vec::with_capacity(self.queue.len());
        for &(p, sy) in &self.queue {
            let min = Vec3::new(
                (p.x * 16) as f32,
                (WORLD_MIN_Y + sy as i32 * 16) as f32,
                (p.z * 16) as f32,
            );
            let rel = min + Vec3::splat(8.0) - cam;
            let mut d = rel.length_squared();
            if let Some(f) = frustum
                && !f.aabb(min - cam, min - cam + Vec3::splat(16.0))
            {
                d = d * 4.0 + 4096.0;
            }
            scored.push((d, p, sy));
        }
        let take = free.min(scored.len());
        if take < scored.len() {
            scored.select_nth_unstable_by(take, |a, b| a.0.total_cmp(&b.0));
        }
        let mut submitted = 0;
        let mut taken = vec![false; scored.len()];
        for (i, &(d, p, sy)) in scored.iter().enumerate().take(take) {
            taken[i] = true;
            if self.submit(world, p, sy as usize, d < 48.0 * 48.0) {
                submitted += 1;
            }
        }
        // Keep the rest queued (sections that became invalid were dropped).
        self.queue = scored
            .iter()
            .zip(&taken)
            .filter(|(_, t)| !**t)
            .map(|((_, p, sy), _)| (*p, *sy))
            .collect();
        let _ = submitted;
    }

    /// Start a mesh job for one section. Returns false if it can't be meshed.
    fn submit(&mut self, world: &World, pos: ChunkPos, sy: usize, near: bool) -> bool {
        let mut cols: [Option<&mc_core::Chunk>; 9] = [None; 9];
        for dz in -1..=1 {
            for dx in -1..=1 {
                cols[((dz + 1) * 3 + dx + 1) as usize] = world.chunk(pos.offset(dx, dz));
            }
        }
        let mut biomes: [Option<Arc<[BiomeId; 256]>>; 9] = Default::default();
        for (i, b) in biomes.iter_mut().enumerate() {
            let p = pos.offset(i as i32 % 3 - 1, i as i32 / 3 - 1);
            *b = self.chunks.get(&p).map(|c| c.biomes.clone());
        }
        let Some(st) = self.chunks.get_mut(&pos) else {
            return false;
        };
        let s = &mut st.sections[sy];
        s.queued = false;
        if cols.iter().any(|c| c.is_none()) || s.pending {
            return false;
        }
        self.generation += 1;
        s.generation = self.generation;
        s.pending = true;
        let input = MeshInput {
            chunk: pos,
            sy,
            sections: std::array::from_fn(|i| section_at(&cols, sy, i).cloned()),
            biomes,
            generation: self.generation,
        };
        let tables = self.tables.clone();
        let prio = if near && s.known {
            Priority::High
        } else {
            Priority::Normal
        };
        self.jobs
            .submit(prio, move || mesher::mesh_section(input, &tables));
        true
    }

    fn apply_results(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, unlimited: bool) {
        while let Some(r) = self.jobs.try_recv() {
            self.ready.push_back(r);
        }
        self.stats.uploaded_this_frame = 0;
        self.stats.upload_bytes_this_frame = 0;
        while let Some(out) = self.ready.pop_front() {
            let bytes: u64 = out.layers.iter().map(|l| l.len() as u64 * 4).sum();
            if !unlimited
                && self.stats.uploaded_this_frame > 0
                && self.stats.upload_bytes_this_frame + bytes > self.upload_budget
            {
                self.ready.push_front(out);
                break;
            }
            self.stats.sections_meshed_total += 1;
            self.stats.mesh_micros_total += out.micros as u64;
            let Some(st) = self.chunks.get_mut(&out.chunk) else {
                continue;
            };
            let s = &mut st.sections[out.sy];
            if s.generation != out.generation {
                continue;
            }
            s.pending = false;
            Self::free_section(&mut self.arena, s);
            for (layer, words) in out.layers.iter().enumerate() {
                let quads = (words.len() / mesher::WORDS_PER_QUAD) as u32;
                if quads == 0 {
                    continue;
                }
                let a = self.arena.alloc(device, quads);
                self.arena.write(queue, a, words);
                s.gpu[layer] = Some((a, quads));
            }
            s.conn = out.connectivity;
            s.known = true;
            s.held = Some(Box::new(out.sections));
            self.stats.uploaded_this_frame += 1;
            self.stats.upload_bytes_this_frame += bytes;
            // Re-verify against the world (it may have changed meanwhile).
            if !st.check {
                st.check = true;
                self.to_check.push(out.chunk);
            }
        }
    }

    /// Sections to draw, front to back. `rd` is the render distance in chunks.
    pub fn collect_visible(
        &mut self,
        cam: Vec3,
        frustum: &Frustum,
        rd: i32,
        cull: &CullSettings,
        out: &mut Vec<DrawItem>,
    ) {
        out.clear();
        let cc = ChunkPos::from_world(cam);
        let d = (2 * rd + 3) as usize;
        let n = d * d * SECTION_COUNT;
        if self.visit_stamp.len() != n {
            self.visit_stamp = vec![0; n];
            self.stamp = 0;
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.visit_stamp.fill(0);
            self.stamp = 1;
        }
        let stamp = self.stamp;
        let vidx = |cx: i32, sy: i32, cz: i32| -> usize {
            let x = (cx - cc.x + rd + 1) as usize;
            let z = (cz - cc.z + rd + 1) as usize;
            (sy as usize * d + z) * d + x
        };
        let max_h = ((rd * 16 + 24) * (rd * 16 + 24)) as f32;
        let in_range = |cx: i32, cz: i32| -> bool {
            if (cx - cc.x).abs() > rd || (cz - cc.z).abs() > rd {
                return false;
            }
            let dx = (cx * 16 + 8) as f32 - cam.x;
            let dz = (cz * 16 + 8) as f32 - cam.z;
            dx * dx + dz * dz <= max_h
        };
        let origin_of = |cx: i32, sy: i32, cz: i32| -> Vec3 {
            Vec3::new(
                (cx * 16) as f32,
                (WORLD_MIN_Y + sy * 16) as f32,
                (cz * 16) as f32,
            ) - cam
        };
        let mut stats_visited = 0u32;
        let mut frustum_culled = 0u32;
        let mut sky_visible = !cull.occlusion;
        let push = |st: &SectionState, cx: i32, sy: i32, cz: i32, out: &mut Vec<DrawItem>| {
            if st.gpu.iter().all(|g| g.is_none()) {
                return;
            }
            let o = origin_of(cx, sy, cz);
            let c = o + Vec3::splat(8.0);
            out.push(DrawItem {
                origin: o.extend(0.0),
                dist2: c.length_squared(),
                layers: st.gpu.map(|g| g.map(|(a, q)| (a.page, a.start, q))),
            });
        };

        if !cull.occlusion {
            // Plain loop over every loaded section in range.
            for (p, st) in &self.chunks {
                if !in_range(p.x, p.z) {
                    continue;
                }
                for sy in 0..SECTION_COUNT as i32 {
                    let s = &st.sections[sy as usize];
                    if s.gpu.iter().all(|g| g.is_none()) {
                        continue;
                    }
                    stats_visited += 1;
                    let o = origin_of(p.x, sy, p.z);
                    if cull.frustum && !frustum.aabb(o, o + Vec3::splat(16.0)) {
                        frustum_culled += 1;
                        continue;
                    }
                    push(s, p.x, sy, p.z, out);
                }
            }
        } else {
            // Flood fill from the camera's section through see-through
            // section faces, never stepping back toward the camera.
            let cam_sy =
                ((cam.y.floor() as i32 - WORLD_MIN_Y) >> 4).clamp(0, SECTION_COUNT as i32 - 1);
            let mut queue: VecDeque<(i32, i32, i32, u8, u8)> = VecDeque::new();
            if self.chunks.contains_key(&cc) {
                self.visit_stamp[vidx(cc.x, cam_sy, cc.z)] = stamp;
                queue.push_back((cc.x, cam_sy, cc.z, 6, 0));
            } else {
                sky_visible = true;
            }
            if cam.y >= (WORLD_MIN_Y + SECTION_COUNT as i32 * 16) as f32 {
                sky_visible = true;
            }
            while let Some((cx, sy, cz, from, dirs)) = queue.pop_front() {
                let Some(st) = self.chunks.get(&ChunkPos::new(cx, cz)) else {
                    continue;
                };
                let s = &st.sections[sy as usize];
                stats_visited += 1;
                push(s, cx, sy, cz, out);
                if sy == SECTION_COUNT as i32 - 1
                    || (cx - cc.x).abs() >= rd
                    || (cz - cc.z).abs() >= rd
                {
                    sky_visible = true;
                }
                let conn = if s.known { s.conn } else { ALL_CONNECTED };
                for (f, &(dx, dy, dz)) in DIRS.iter().enumerate() {
                    if dirs & (1 << (f ^ 1)) != 0 {
                        continue;
                    }
                    if from != 6 && !mesher::connected(conn, from as usize, f) {
                        continue;
                    }
                    let (nx, ny, nz) = (cx + dx, sy + dy, cz + dz);
                    if ny < 0 || ny >= SECTION_COUNT as i32 {
                        continue;
                    }
                    if !in_range(nx, nz) {
                        // Looking out past the loaded area: the sky shows.
                        sky_visible = true;
                        continue;
                    }
                    let vi = vidx(nx, ny, nz);
                    if self.visit_stamp[vi] == stamp {
                        continue;
                    }
                    if !self.chunks.contains_key(&ChunkPos::new(nx, nz)) {
                        continue;
                    }
                    let o = origin_of(nx, ny, nz);
                    if cull.frustum && !frustum.aabb(o, o + Vec3::splat(16.0)) {
                        continue;
                    }
                    self.visit_stamp[vi] = stamp;
                    queue.push_back((nx, ny, nz, (f ^ 1) as u8, dirs | (1 << f)));
                }
            }
        }
        out.sort_unstable_by(|a, b| a.dist2.total_cmp(&b.dist2));
        let with_mesh = self
            .chunks
            .values()
            .flat_map(|c| c.sections.iter())
            .filter(|s| s.gpu.iter().any(|g| g.is_some()))
            .count() as u32;
        self.stats.sections_with_mesh = with_mesh;
        self.stats.visible = out.len() as u32;
        self.stats.sky_visible = sky_visible;
        self.stats.visited = stats_visited;
        self.stats.frustum_culled = frustum_culled;
        self.stats.quads_total = self
            .chunks
            .values()
            .flat_map(|c| c.sections.iter())
            .flat_map(|s| s.gpu.iter().flatten())
            .map(|(_, q)| *q as u64)
            .sum();
    }

    /// Count of sections in range that have a mesh (for culling stats).
    pub fn meshed_in_range(&self, cam: Vec3, rd: i32) -> u32 {
        let cc = ChunkPos::from_world(cam);
        self.chunks
            .iter()
            .filter(|(p, _)| p.chebyshev(cc) <= rd)
            .flat_map(|(_, c)| c.sections.iter())
            .filter(|s| s.gpu.iter().any(|g| g.is_some()))
            .count() as u32
    }
}

/// Section `i` of the 27-neighbourhood (index `(dy+1)*9 + (dz+1)*3 + (dx+1)`)
/// around section `sy` of the centre chunk.
fn section_at<'a>(
    cols: &[Option<&'a mc_core::Chunk>; 9],
    sy: usize,
    i: usize,
) -> Option<&'a Arc<Section>> {
    let dy = (i / 9) as i32 - 1;
    let ci = i % 9;
    let y = sy as i32 + dy;
    if y < 0 || y >= SECTION_COUNT as i32 {
        return None;
    }
    cols[ci].map(|c| &c.sections[y as usize])
}
