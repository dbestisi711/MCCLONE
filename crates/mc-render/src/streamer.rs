//! Chunk streaming: background world generation, lighting and unloading.
//!
//! Each frame [`ChunkStreamer::update`]:
//! 1. relights around edited blocks (`World::take_dirty_blocks`; the
//!    positions stay available through [`ChunkStreamer::last_dirty_blocks`]);
//! 2. inserts finished chunks (nearest first, bounded per frame);
//! 3. applies finished light jobs (discarding ones whose input changed);
//! 4. unloads chunks beyond the render distance plus a margin;
//! 5. starts light jobs for chunks whose 8 neighbours are loaded;
//! 6. starts generation jobs, nearest first, favouring the view direction,
//!    with a bounded number in flight.
//!
//! Generation radius is `render_distance + 1` so every chunk inside the
//! render distance can be lit and meshed (both need their neighbours).

use std::sync::Arc;

use glam::{IVec3, Vec3};
use mc_core::{Chunk, ChunkPos, World};
use mc_worldgen::WorldGenerator;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::light::{self, LightOutput};
use crate::tasks::{self, Jobs, Priority};

#[derive(Clone, Copy, Debug, Default)]
pub struct StreamStats {
    pub generated_total: u64,
    pub gen_micros_total: u64,
    pub lit_total: u64,
    pub light_micros_total: u64,
    pub light_discarded: u64,
    pub gen_in_flight: u32,
    pub light_in_flight: u32,
    pub waiting_insert: u32,
    pub unlit: u32,
    /// Dirty blocks relit this frame and the time it took.
    pub relit_blocks: u32,
    pub relight_micros: u32,
}

/// Loads/generates chunks around the player and unloads far ones.
pub struct ChunkStreamer {
    pub generator: Arc<WorldGenerator>,
    /// Radius in chunks.
    pub render_distance: i32,
    pub stats: StreamStats,
    /// Max chunks inserted into the world per frame.
    pub insert_budget: usize,
    view_dir: Vec3,
    gen_jobs: Jobs<(Chunk, u32)>,
    gen_pending: HashSet<ChunkPos>,
    finished: Vec<Chunk>,
    light: Jobs<LightOutput>,
    light_pending: HashSet<ChunkPos>,
    /// Revision bumps caused by light writes (block revision = revision - bumps).
    light_bumps: HashMap<ChunkPos, u64>,
    unlit: HashSet<ChunkPos>,
    known_count: usize,
    order: Vec<(i32, i32)>,
    order_key: (i32, i32),
    last_dirty: Vec<IVec3>,
}

impl ChunkStreamer {
    pub fn new(generator: Arc<WorldGenerator>, render_distance: i32) -> Self {
        ChunkStreamer {
            generator,
            render_distance,
            stats: StreamStats::default(),
            insert_budget: 24,
            view_dir: Vec3::NEG_Z,
            gen_jobs: Jobs::new(),
            gen_pending: HashSet::default(),
            finished: Vec::new(),
            light: Jobs::new(),
            light_pending: HashSet::default(),
            light_bumps: HashMap::default(),
            unlit: HashSet::default(),
            known_count: 0,
            order: Vec::new(),
            order_key: (-1, -1),
            last_dirty: Vec::new(),
        }
    }

    /// Camera look direction, used to generate what's in front first.
    pub fn set_view_direction(&mut self, dir: Vec3) {
        self.view_dir = dir;
    }

    pub fn set_render_distance(&mut self, rd: i32) {
        self.render_distance = rd.clamp(2, 64);
    }

    /// Block positions changed last frame (already consumed from the world's
    /// dirty queue by the light engine), for other systems that need them.
    pub fn last_dirty_blocks(&self) -> &[IVec3] {
        &self.last_dirty
    }

    fn block_rev(&self, c: &Chunk) -> u64 {
        c.revision
            .wrapping_sub(self.light_bumps.get(&c.pos).copied().unwrap_or(0))
    }

    fn bump(&mut self, world: &mut World, p: ChunkPos) {
        if let Some(c) = world.chunk_mut(p) {
            c.revision += 1;
            *self.light_bumps.entry(p).or_insert(0) += 1;
        }
    }

    fn on_insert(&mut self, c: &Chunk) {
        self.light_bumps.remove(&c.pos);
        if !c.light_ready {
            self.unlit.insert(c.pos);
        }
    }

    /// Re-scan the world if chunks were added/removed behind our back.
    fn sync(&mut self, world: &World) {
        if world.chunk_count() == self.known_count {
            return;
        }
        self.unlit
            .retain(|p| world.chunk(*p).is_some_and(|c| !c.light_ready));
        for c in world.chunks() {
            if !c.light_ready {
                self.unlit.insert(c.pos);
            }
        }
        self.light_bumps.retain(|p, _| world.has_chunk(*p));
        self.known_count = world.chunk_count();
    }

    /// Called every frame. Kicks off generation for missing chunks near
    /// `center`, inserts finished ones into `world`, unloads far ones.
    pub fn update(&mut self, world: &mut World, center: ChunkPos) {
        self.sync(world);
        // 1. Edits → incremental light.
        let dirty = world.take_dirty_blocks();
        self.stats.relit_blocks = dirty.len() as u32;
        if !dirty.is_empty() {
            let t0 = light::web_time_now();
            let touched = light::update_blocks(world, &dirty);
            for p in touched {
                self.bump(world, p);
            }
            self.stats.relight_micros = light::elapsed_micros(t0);
        }
        self.last_dirty = dirty;

        let rd = self.render_distance;
        let gen_r = rd + 1;
        // 2. Finished generation.
        while let Some((chunk, us)) = self.gen_jobs.try_recv() {
            self.gen_pending.remove(&chunk.pos);
            self.stats.generated_total += 1;
            self.stats.gen_micros_total += us as u64;
            self.finished.push(chunk);
        }
        if !self.finished.is_empty() {
            self.finished
                .sort_by_key(|c| std::cmp::Reverse(c.pos.chebyshev(center)));
            let mut n = 0;
            while n < self.insert_budget {
                let Some(c) = self.finished.pop() else { break };
                if c.pos.chebyshev(center) > gen_r + 1 || world.has_chunk(c.pos) {
                    continue;
                }
                self.on_insert(&c);
                world.insert_chunk(c);
                n += 1;
            }
            // Drop results that left the area while waiting.
            self.finished
                .retain(|c| c.pos.chebyshev(center) <= gen_r + 1);
        }

        // 3. Finished light jobs.
        while let Some(out) = self.light.try_recv() {
            self.light_pending.remove(&out.center);
            self.stats.light_micros_total += out.micros as u64;
            if self.light_result_valid(world, &out) {
                if let Some(c) = world.chunk_mut(out.center) {
                    light::apply(c, &out);
                }
                self.bump(world, out.center);
                self.unlit.remove(&out.center);
                self.stats.lit_total += 1;
            } else {
                self.stats.light_discarded += 1;
            }
        }

        // 4. Unload.
        let far: Vec<ChunkPos> = world
            .chunk_positions()
            .filter(|p| p.chebyshev(center) > gen_r + 2)
            .collect();
        for p in far {
            world.remove_chunk(p);
            self.unlit.remove(&p);
            self.light_bumps.remove(&p);
        }
        self.known_count = world.chunk_count();

        // 5. Light jobs.
        let cap = tasks::pool().threads() * 2;
        if self.light.in_flight() < cap && !self.unlit.is_empty() {
            let mut cands: Vec<ChunkPos> = self
                .unlit
                .iter()
                .copied()
                .filter(|p| !self.light_pending.contains(p) && p.chebyshev(center) <= rd)
                .filter(|p| (-1..=1).all(|dz| (-1..=1).all(|dx| world.has_chunk(p.offset(dx, dz)))))
                .collect();
            cands.sort_by_key(|p| {
                p.chebyshev(center) * 1000 + (p.x - center.x).abs() + (p.z - center.z).abs()
            });
            for p in cands {
                if self.light.in_flight() >= cap {
                    break;
                }
                let bumps = &self.light_bumps;
                let Some(input) = light::snapshot(world, p, |c| {
                    c.revision
                        .wrapping_sub(bumps.get(&c.pos).copied().unwrap_or(0))
                }) else {
                    continue;
                };
                self.light_pending.insert(p);
                self.light
                    .submit(Priority::Normal, move || light::compute(&input));
            }
        }

        // 6. Generation jobs.
        let cap = tasks::pool().threads() * 2;
        if self.gen_jobs.in_flight() < cap {
            self.refresh_order(gen_r);
            for i in 0..self.order.len() {
                if self.gen_jobs.in_flight() >= cap {
                    break;
                }
                let (dx, dz) = self.order[i];
                let p = center.offset(dx, dz);
                if world.has_chunk(p)
                    || self.gen_pending.contains(&p)
                    || self.finished.iter().any(|c| c.pos == p)
                {
                    continue;
                }
                self.gen_pending.insert(p);
                let g = self.generator.clone();
                self.gen_jobs.submit(Priority::Low, move || {
                    let t0 = light::web_time_now();
                    let c = g.generate(p);
                    (c, light::elapsed_micros(t0))
                });
            }
        }

        self.stats.gen_in_flight = self.gen_jobs.in_flight() as u32;
        self.stats.light_in_flight = self.light.in_flight() as u32;
        self.stats.waiting_insert = self.finished.len() as u32;
        self.stats.unlit = self.unlit.len() as u32;
    }

    fn light_result_valid(&self, world: &World, out: &LightOutput) -> bool {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let i = ((dz + 1) * 3 + dx + 1) as usize;
                match world.chunk(out.center.offset(dx, dz)) {
                    Some(c) if self.block_rev(c) == out.block_revs[i] => {}
                    _ => return false,
                }
            }
        }
        true
    }

    /// Offsets within the generation radius sorted by priority: distance,
    /// with chunks in front of the camera first.
    fn refresh_order(&mut self, r: i32) {
        let h = glam::Vec2::new(self.view_dir.x, self.view_dir.z);
        let sector = if h.length() > 0.1 {
            ((h.y.atan2(h.x) / std::f32::consts::TAU * 16.0).round() as i32).rem_euclid(16)
        } else {
            -1
        };
        if self.order_key == (r, sector) && !self.order.is_empty() {
            return;
        }
        self.order_key = (r, sector);
        let dir = h.normalize_or_zero();
        let mut v: Vec<(f32, (i32, i32))> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                let o = glam::Vec2::new(dx as f32, dz as f32);
                let d = o.length();
                let facing = if d > 0.0 { o.dot(dir) / d } else { 1.0 };
                // Up to ~40% closer when straight ahead; always keep the
                // immediate surroundings first.
                let score = if d <= 1.5 {
                    d
                } else {
                    d * (1.0 - 0.2 * (facing + 1.0)) + 1.5
                };
                v.push((score, (dx, dz)));
            }
        }
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.order = v.into_iter().map(|(_, o)| o).collect();
    }

    /// Block until every chunk within `radius` of `center` is loaded and lit
    /// (startup / screenshots). Generation and lighting run in parallel.
    pub fn load_blocking(&mut self, world: &mut World, center: ChunkPos, radius: i32) {
        self.sync(world);
        let mut missing = Vec::new();
        for dz in -(radius + 1)..=(radius + 1) {
            for dx in -(radius + 1)..=(radius + 1) {
                let p = center.offset(dx, dz);
                if !world.has_chunk(p) {
                    missing.push(p);
                }
            }
        }
        let g = self.generator.clone();
        let chunks = tasks::pool().par_map(missing, |p| g.generate(p));
        for c in chunks {
            self.on_insert(&c);
            world.insert_chunk(c);
            self.stats.generated_total += 1;
        }
        let mut inputs = Vec::new();
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let p = center.offset(dx, dz);
                if world.chunk(p).is_some_and(|c| c.light_ready) {
                    continue;
                }
                let bumps = &self.light_bumps;
                if let Some(i) = light::snapshot(world, p, |c| {
                    c.revision
                        .wrapping_sub(bumps.get(&c.pos).copied().unwrap_or(0))
                }) {
                    inputs.push(i);
                }
            }
        }
        let outs = tasks::pool().par_map(inputs, |i| light::compute(&i));
        for out in outs {
            if let Some(c) = world.chunk_mut(out.center) {
                light::apply(c, &out);
            }
            self.bump(world, out.center);
            self.unlit.remove(&out.center);
            self.stats.lit_total += 1;
            self.stats.light_micros_total += out.micros as u64;
        }
        self.known_count = world.chunk_count();
    }
}
