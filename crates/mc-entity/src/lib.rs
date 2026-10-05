//! Player controller, physics, mobs, item entities.
//!
//! OWNER: mob behaviour & physics agent. Public API used by `mc-game`
//! (keep these signatures stable; add freely):
//! - [`Player`]: `new`, `look`, `tick`, `interpolated_position`, `eye_position`,
//!   `aabb`, `camera`, `look_dir`, `damage`, `try_eat`
//! - [`EntityManager`]: `new`, `tick`, `spawn_item`, `throw_item`, `attack`,
//!   `interact`, `raycast`, `fill_frame`, `count`, `on_block_changed`,
//!   `take_events`
//!
//! Modules:
//! - [`physics`]: AABB-vs-voxel collision (swept, per axis), step-up, fluids.
//! - [`path`]: A* over the block grid.
//! - `player`: movement, survival mechanics, camera.
//! - `mob` / `ai`: mob kinds, damage, goals.
//! - [`items`] / [`projectile`]: dropped items, falling blocks, arrows.
//! - `spawn`: natural spawning and despawning.
//! - `render`: procedural poses and the hand-off to the renderer.
//!
//! Everything is deterministic for a given seed (own PRNG in [`rng`]) and uses
//! no threads, IO or OS APIs, so it also runs on `wasm32`.

mod ai;
pub mod items;
mod mob;
pub mod path;
pub mod physics;
mod player;
pub mod projectile;
mod render;
pub mod rng;
mod spawn;

use glam::{IVec3, Vec2, Vec3};
use mc_core::block::Drop;
use mc_core::{ChunkPos, ItemId, ItemStack, World, blocks};

pub use ai::{AiState, MoveIntent};
pub use items::{FallingBlock, ItemEntity};
pub use mob::{
    CREEPER_FUSE, CREEPER_POWER, DEATH_TICKS, LootDrop, Mob, MobKind, MobSpec, light_levels,
    sky_exposed,
};
pub use player::{
    DamageSource, EAT_TICKS, Eating, GameMode, PLAYER_EYE_HEIGHT, PLAYER_HEIGHT,
    PLAYER_SNEAK_EYE_HEIGHT, PLAYER_SNEAK_HEIGHT, PLAYER_WIDTH, Player, WALK_SPEED,
};
pub use projectile::Arrow;
pub use spawn::{HOSTILE_CAP, PASSIVE_CAP};

use crate::rng::Rng;

/// Something that happened during a tick, for audio / effects in `mc-game`.
/// Sound names are keys of the pack's `sounds/sound_definitions.json`.
#[derive(Clone, Debug, PartialEq)]
pub enum EntityEvent {
    Sound { name: &'static str, pos: Vec3 },
    Explosion { pos: Vec3, power: f32 },
    PlayerHurt { amount: f32, source: DamageSource },
    PlayerDied { source: DamageSource },
    MobDied { kind: MobKind, pos: Vec3 },
    ItemPickedUp { item: ItemId, count: u8 },
}

impl EntityEvent {
    pub fn sound(name: &'static str, pos: Vec3) -> Self {
        EntityEvent::Sound { name, pos }
    }
}

/// Events kept when nobody drains them.
const MAX_EVENTS: usize = 512;

/// Mutable world access handed to each mob while it ticks.
pub(crate) struct MobCtx<'a> {
    pub world: &'a mut World,
    pub player: &'a mut Player,
    pub rng: &'a mut Rng,
    pub events: &'a mut Vec<EntityEvent>,
    pub arrows: &'a mut Vec<Arrow>,
    pub explosions: &'a mut Vec<(Vec3, f32)>,
    pub drops: &'a mut Vec<(ItemStack, Vec3)>,
}

impl Mob {
    /// One 20 TPS step: timers, environment, AI, physics, animation.
    pub(crate) fn tick(&mut self, ctx: &mut MobCtx) {
        self.prev_pos = self.body.pos;
        self.prev_yaw = self.yaw;
        self.prev_head_yaw = self.head_yaw;
        self.prev_pitch = self.pitch;
        self.prev_limb_swing = self.limb_swing;
        self.prev_limb_amount = self.limb_amount;
        self.prev_fuse = self.fuse;
        self.prev_flap = self.flap;
        self.age = self.age.wrapping_add(1);
        self.hurt_time = self.hurt_time.saturating_sub(1);
        self.invulnerable = self.invulnerable.saturating_sub(1);
        self.light = ctx.world.light(self.eye().floor().as_ivec3());
        if self.just_hurt {
            self.just_hurt = false;
            if self.health > 0.0 {
                ctx.events
                    .push(EntityEvent::sound(self.spec().hurt_sound, self.body.pos));
            }
        }

        if self.health <= 0.0 || self.death_time > 0 {
            if self.death_time == 0 {
                self.begin_death(ctx);
            } else {
                self.death_time += 1;
            }
            let still = MoveIntent::default();
            self.travel(ctx.world, &still);
            self.limb_amount *= 0.8;
            return;
        }

        self.environment(ctx);
        if self.health <= 0.0 {
            return;
        }
        let intent = self.think(ctx);
        if self.exploded {
            self.begin_death(ctx);
            return;
        }
        if let Some(fall) = self.travel(ctx.world, &intent) {
            let dmg = (fall - 3.0 - 1e-3).ceil();
            if dmg > 0.0 {
                self.hurt(dmg, DamageSource::Fall);
            }
        }
        self.update_rotation(&intent);

        let moved = self.body.pos - self.prev_pos;
        let speed = Vec2::new(moved.x, moved.z).length();
        let target = (speed * 4.0).min(1.0);
        self.limb_amount += (target - self.limb_amount) * 0.4;
        self.limb_swing += self.limb_amount;
        if self.kind == MobKind::Chicken {
            let target = if self.body.on_ground || self.body.in_fluid() {
                0.0
            } else {
                1.0
            };
            self.flap += (target - self.flap) * 0.3;
        }
    }
}

/// Owns all non-player entities (mobs, dropped items, falling blocks, arrows).
pub struct EntityManager {
    rng: Rng,
    next_id: u64,
    pub(crate) mobs: Vec<Mob>,
    pub(crate) items: Vec<ItemEntity>,
    pub(crate) falling: Vec<FallingBlock>,
    pub(crate) arrows: Vec<Arrow>,
    events: Vec<EntityEvent>,
    pending_blocks: Vec<IVec3>,
    pub(crate) tick_count: u64,
    player_spilled: bool,
    /// Natural spawning and despawning of mobs.
    pub natural_spawning: bool,
}

impl Default for EntityManager {
    fn default() -> Self {
        Self::new(0)
    }
}

impl EntityManager {
    pub fn new(seed: u64) -> Self {
        EntityManager {
            rng: Rng::new(seed ^ 0x00E7_7171_E5EE_D5ED),
            next_id: 1,
            mobs: Vec::new(),
            items: Vec::new(),
            falling: Vec::new(),
            arrows: Vec::new(),
            events: Vec::new(),
            pending_blocks: Vec::new(),
            tick_count: 0,
            player_spilled: false,
            natural_spawning: true,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// One fixed 20 TPS step: AI, physics, spawning/despawning, item pickup.
    pub fn tick(&mut self, world: &mut World, player: &mut Player) {
        self.tick_count += 1;
        self.events.append(&mut player.events);
        self.process_block_checks(world);

        // A player who died in survival drops everything.
        if player.dead && !self.player_spilled {
            self.player_spilled = true;
            if !player.creative() {
                self.spill_inventory(player);
            }
        } else if !player.dead {
            self.player_spilled = false;
        }

        // Mobs.
        let mut new_arrows = Vec::new();
        let mut explosions = Vec::new();
        let mut drops = Vec::new();
        {
            let mut ctx = MobCtx {
                world: &mut *world,
                player: &mut *player,
                rng: &mut self.rng,
                events: &mut self.events,
                arrows: &mut new_arrows,
                explosions: &mut explosions,
                drops: &mut drops,
            };
            for m in self.mobs.iter_mut() {
                if m.removed() || !ctx.world.has_chunk(ChunkPos::from_world(m.body.pos)) {
                    continue;
                }
                m.tick(&mut ctx);
            }
        }
        self.separate(player);

        // Arrows.
        self.arrows.append(&mut new_arrows);
        for a in self.arrows.iter_mut() {
            a.tick(world, player, &mut self.mobs, &mut self.events);
        }

        for (center, power) in explosions {
            self.explode(world, player, center, power);
        }
        for (stack, pos) in drops {
            self.spawn_item(stack, pos);
        }

        // Items.
        for it in self.items.iter_mut() {
            if world.has_chunk(ChunkPos::from_world(it.pos)) {
                it.tick(world);
            }
        }
        if self.items.len() < 256 || self.tick_count.is_multiple_of(4) {
            self.merge_items();
        }
        self.pickup(player);

        // Falling blocks.
        let mut landed = Vec::new();
        for f in self.falling.iter_mut() {
            if let Some(cell) = f.tick(world) {
                landed.push((f.block, cell));
            }
        }
        for (block, cell) in landed {
            let here = world.block_loaded(cell);
            let placed = match here {
                Some(id) if id.is_air() || id.def().replaceable => {
                    world.set_block(cell, block).is_some()
                }
                _ => false,
            };
            if placed {
                self.pending_blocks.push(cell);
                // A block falling right behind may already overlap the new
                // block (only the lower one was clipped): rest it on top.
                let cell_box = mc_core::Aabb::block(cell);
                let top = (cell.y + 1) as f32;
                for f in self.falling.iter_mut() {
                    let fb = mc_core::Aabb::from_feet(f.pos, items::FALLING_BLOCK_SIZE, 0.98);
                    if !f.dead && fb.intersects(&cell_box) && f.pos.y > cell.y as f32 {
                        f.pos.y = top;
                        f.vel.y = 0.0;
                    }
                }
            } else {
                self.spawn_item(
                    ItemStack::of_block(block, 1),
                    cell.as_vec3() + Vec3::splat(0.5),
                );
            }
        }

        if self.natural_spawning {
            self.spawn_tick(world, player);
            self.despawn_tick(player);
        }

        self.mobs.retain(|m| !m.removed());
        self.items.retain(|i| !i.dead && i.stack.count > 0);
        self.arrows.retain(|a| !a.dead);
        self.falling.retain(|f| !f.dead);
        self.events.append(&mut player.events);
        if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
        }
    }

    /// Drain sounds/explosions/deaths that happened since the last call.
    pub fn take_events(&mut self) -> Vec<EntityEvent> {
        std::mem::take(&mut self.events)
    }

    /// Spawn a mob (feet position). Returns its id.
    pub fn spawn_mob(&mut self, kind: MobKind, pos: Vec3) -> u64 {
        let yaw = self.rng.f32() * std::f32::consts::TAU;
        self.spawn_mob_with_yaw(kind, pos, yaw)
    }

    pub fn spawn_mob_with_yaw(&mut self, kind: MobKind, pos: Vec3, yaw: f32) -> u64 {
        let id = self.alloc_id();
        let mut m = Mob::new(id, kind, pos, yaw);
        m.egg_timer = 6000 + self.rng.below(6000);
        self.mobs.push(m);
        id
    }

    pub fn mobs(&self) -> &[Mob] {
        &self.mobs
    }

    pub fn mobs_mut(&mut self) -> &mut [Mob] {
        &mut self.mobs
    }

    pub fn mob(&self, id: u64) -> Option<&Mob> {
        self.mobs.iter().find(|m| m.id == id)
    }

    pub fn mob_mut(&mut self, id: u64) -> Option<&mut Mob> {
        self.mobs.iter_mut().find(|m| m.id == id)
    }

    pub fn items(&self) -> &[ItemEntity] {
        &self.items
    }

    pub fn falling_blocks(&self) -> &[FallingBlock] {
        &self.falling
    }

    pub fn arrows(&self) -> &[Arrow] {
        &self.arrows
    }

    /// Spawn a dropped item at a position with a small random velocity.
    pub fn spawn_item(&mut self, stack: ItemStack, pos: Vec3) {
        if stack.count == 0 {
            return;
        }
        let vel = Vec3::new(self.rng.signed() * 0.1, 0.2, self.rng.signed() * 0.1);
        self.spawn_item_with_velocity(stack, pos, vel);
    }

    /// Spawn a dropped item with an explicit velocity (blocks/tick).
    pub fn spawn_item_with_velocity(&mut self, stack: ItemStack, pos: Vec3, vel: Vec3) -> u64 {
        let id = self.alloc_id();
        let phase = self.rng.f32() * std::f32::consts::TAU;
        // `pos` is the item's centre; the entity stores its bottom.
        let feet = pos - Vec3::Y * (items::ITEM_SIZE * 0.5);
        self.items
            .push(ItemEntity::new(id, stack, feet, vel, phase));
        id
    }

    /// Throw an item from the player's eye along `dir` (the Q key).
    pub fn throw_item(&mut self, stack: ItemStack, pos: Vec3, dir: Vec3) {
        if stack.count == 0 {
            return;
        }
        let jitter = Vec3::new(self.rng.signed(), self.rng.signed(), self.rng.signed()) * 0.02;
        let vel = dir.normalize_or_zero() * 0.3 + Vec3::Y * 0.1 + jitter;
        let id = self.spawn_item_with_velocity(stack, pos - Vec3::Y * 0.3, vel);
        if let Some(it) = self.items.iter_mut().find(|i| i.id == id) {
            it.pickup_delay = items::THROW_PICKUP_DELAY;
        }
    }

    fn spill_inventory(&mut self, player: &mut Player) {
        let at = player.position + Vec3::Y * 0.5;
        let inv = &mut player.inventory;
        let mut stacks: Vec<ItemStack> = inv.slots.iter_mut().filter_map(|s| s.take()).collect();
        stacks.extend(inv.armor.iter_mut().filter_map(|s| s.take()));
        stacks.extend(inv.offhand.take());
        for s in stacks {
            let a = self.rng.f32() * std::f32::consts::TAU;
            let r = self.rng.f32() * 0.25;
            let vel = Vec3::new(a.cos() * r, 0.2, a.sin() * r);
            self.spawn_item_with_velocity(s, at, vel);
        }
    }

    fn nearest_mob_on_ray(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<(usize, f32)> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let mut best: Option<(usize, f32)> = None;
        for (i, m) in self.mobs.iter().enumerate() {
            if !m.alive() {
                continue;
            }
            if let Some(t) = physics::ray_aabb(origin, dir, &m.aabb().inflate(0.1))
                && t <= reach
                && best.is_none_or(|(_, bt)| t < bt)
            {
                best = Some((i, t));
            }
        }
        best
    }

    /// Player melee attack along a ray. Returns true if an entity was hit.
    pub fn attack(&mut self, origin: Vec3, dir: Vec3, reach: f32, damage: f32) -> bool {
        let Some((i, _)) = self.nearest_mob_on_ray(origin, dir, reach) else {
            return false;
        };
        let m = &mut self.mobs[i];
        // Knock back along the swing direction.
        let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        let attacker = m.body.pos - flat;
        m.hurt(damage, DamageSource::Player { attacker });
        true
    }

    /// Distance to the closest entity hit by a ray (so block targeting can be blocked by mobs).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<f32> {
        self.nearest_mob_on_ray(origin, dir, reach).map(|(_, t)| t)
    }

    /// Right-click an entity with `held`. Returns true if something happened
    /// (e.g. a sheep was sheared — the caller should wear the tool).
    pub fn interact(&mut self, origin: Vec3, dir: Vec3, reach: f32, held: Option<ItemId>) -> bool {
        let Some((i, _)) = self.nearest_mob_on_ray(origin, dir, reach) else {
            return false;
        };
        let shears = ItemId::by_name("shears");
        let m = &mut self.mobs[i];
        if m.kind == MobKind::Sheep && !m.sheared && held.is_some() && held == shears {
            m.sheared = true;
            let at = m.center();
            let n = self.rng.range(1, 3) as u8;
            self.events.push(EntityEvent::sound("mob.sheep.shear", at));
            if let Some(wool) = ItemId::by_name("white_wool") {
                self.spawn_item(ItemStack::new(wool, n), at);
            }
            return true;
        }
        false
    }

    /// Tell the entity system a block changed (broken/placed) so unsupported
    /// sand/gravel at or above it starts falling on the next tick.
    pub fn on_block_changed(&mut self, world: &World, pos: IVec3) {
        for p in [pos, pos + IVec3::Y] {
            if world.block(p).def().gravity {
                self.pending_blocks.push(p);
            }
        }
    }

    fn process_block_checks(&mut self, world: &mut World) {
        let pending = std::mem::take(&mut self.pending_blocks);
        for p in pending {
            let mut q = p;
            loop {
                let id = world.block(q);
                if !id.def().gravity {
                    break;
                }
                match world.block_loaded(q - IVec3::Y) {
                    Some(below) if items::can_fall_through(below) => {}
                    _ => break,
                }
                if world.set_block(q, blocks::AIR).is_none() {
                    break;
                }
                self.falling.push(FallingBlock::new(id, q));
                q += IVec3::Y;
            }
        }
    }

    /// Explosion: removes blocks in a rough sphere (not bedrock, obsidian or
    /// fluids), drops some of them, hurts and pushes entities with falloff.
    pub fn explode(&mut self, world: &mut World, player: &mut Player, center: Vec3, power: f32) {
        self.events
            .push(EntityEvent::Explosion { pos: center, power });
        self.events
            .push(EntityEvent::sound("random.explode", center));
        let r = power.ceil() as i32 + 1;
        let c = center.floor().as_ivec3();
        let mut removed = Vec::new();
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    let p = c + IVec3::new(dx, dy, dz);
                    let d = (p.as_vec3() + Vec3::splat(0.5) - center).length();
                    let reach = power * (0.6 + 0.5 * self.rng.f32());
                    if d > reach {
                        continue;
                    }
                    let Some(id) = world.block_loaded(p) else {
                        continue;
                    };
                    let def = id.def();
                    if id.is_air() || def.fluid || def.hardness < 0.0 || def.hardness >= 10.0 {
                        continue;
                    }
                    removed.push((p, id));
                }
            }
        }
        let drop_chance = 1.0 / power.max(1.0);
        for &(p, id) in &removed {
            if world.set_block(p, blocks::AIR).is_none() {
                continue;
            }
            if self.rng.chance(drop_chance) {
                let stack = match id.def().drop {
                    Drop::Itself => Some(ItemStack::of_block(id, 1)),
                    Drop::Nothing => None,
                    Drop::Other(name, n) => ItemId::by_name(name).map(|i| ItemStack::new(i, n)),
                };
                if let Some(s) = stack {
                    self.spawn_item(s, p.as_vec3() + Vec3::splat(0.5));
                }
            }
        }
        for &(p, _) in &removed {
            let above = p + IVec3::Y;
            let up = world.block(above);
            if up.def().needs_support && world.block(p).is_air() {
                world.set_block(above, blocks::AIR);
            }
            self.pending_blocks.push(above);
        }

        let radius = power * 2.0;
        let impact = |world: &World, target: mc_core::Aabb| -> (f32, Vec3) {
            let tc = target.center();
            let d = (tc - center).length();
            if d >= radius {
                return (0.0, Vec3::ZERO);
            }
            let e = exposure(world, center, &target);
            let dir = (tc - center).normalize_or_zero();
            ((1.0 - d / radius) * e, dir)
        };
        let damage_for = |i: f32| ((i * i + i) / 2.0 * 7.0 * radius + 1.0).floor();

        let (pi, pdir) = impact(world, player.aabb());
        if pi > 0.0 {
            player.damage(damage_for(pi), DamageSource::Explosion { center });
            if !player.creative() || !player.flying {
                player.velocity += pdir * pi;
            }
        }
        for m in self.mobs.iter_mut() {
            if !m.alive() {
                continue;
            }
            let (mi, mdir) = impact(world, m.aabb());
            if mi > 0.0 {
                m.hurt(damage_for(mi), DamageSource::Explosion { center });
                m.body.vel += mdir * mi;
            }
        }
        for it in self.items.iter_mut() {
            let (ii, idir) = impact(world, it.aabb());
            if ii > 0.0 {
                it.vel += idir * ii;
            }
        }
    }

    /// Soft pushing between overlapping mobs, and between mobs and the player.
    fn separate(&mut self, player: &mut Player) {
        const PUSH: f32 = 0.05;
        let n = self.mobs.len();
        for i in 0..n {
            if !self.mobs[i].alive() {
                continue;
            }
            for j in (i + 1)..n {
                let (left, right) = self.mobs.split_at_mut(j);
                let (a, b) = (&mut left[i], &mut right[0]);
                if !b.alive() {
                    continue;
                }
                if let Some(push) = overlap_push(&a.aabb(), &b.aabb()) {
                    a.body.vel -= push * PUSH;
                    b.body.vel += push * PUSH;
                }
            }
            if !player.dead && !player.flying {
                let m = &mut self.mobs[i];
                if let Some(push) = overlap_push(&m.aabb(), &player.aabb()) {
                    m.body.vel -= push * PUSH;
                    player.velocity += push * PUSH * 0.5;
                }
            }
        }
    }

    fn merge_items(&mut self) {
        let n = self.items.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let (left, right) = self.items.split_at_mut(j);
                let (a, b) = (&mut left[i], &mut right[0]);
                if a.dead || b.dead || !a.stack.can_stack_with(&b.stack) {
                    continue;
                }
                let max = a.stack.item.max_stack();
                let total = a.stack.count as u32 + b.stack.count as u32;
                if total > max as u32 {
                    continue;
                }
                let d = b.pos - a.pos;
                if d.x.abs() > 1.0 || d.z.abs() > 1.0 || d.y.abs() > 0.5 {
                    continue;
                }
                let (keep, gone) = if b.stack.count > a.stack.count {
                    (b, a)
                } else {
                    (a, b)
                };
                keep.stack.count = total as u8;
                keep.age = keep.age.min(gone.age);
                keep.pickup_delay = keep.pickup_delay.max(gone.pickup_delay);
                gone.stack.count = 0;
                gone.dead = true;
            }
        }
    }

    fn pickup(&mut self, player: &mut Player) {
        if player.dead {
            return;
        }
        let pa = player.aabb();
        let reach = mc_core::Aabb::new(
            pa.min - Vec3::new(1.0, 0.5, 1.0),
            pa.max + Vec3::new(1.0, 0.5, 1.0),
        );
        for it in self.items.iter_mut() {
            if it.dead || it.pickup_delay > 0 || !reach.intersects(&it.aabb()) {
                continue;
            }
            let before = it.stack.count;
            let taken = match player.inventory.add(it.stack) {
                None => {
                    it.dead = true;
                    it.stack.count = 0;
                    before
                }
                Some(rest) => {
                    let t = before - rest.count;
                    it.stack = rest;
                    t
                }
            };
            if taken > 0 {
                self.events.push(EntityEvent::ItemPickedUp {
                    item: it.stack.item,
                    count: taken,
                });
                self.events.push(EntityEvent::sound("random.pop", it.pos));
            }
        }
    }

    /// Number of entities (mobs, items, arrows, falling blocks).
    pub fn count(&self) -> usize {
        self.mobs.len() + self.items.len() + self.arrows.len() + self.falling.len()
    }
}

/// Horizontal push direction from `a` toward `b` when the boxes overlap.
fn overlap_push(a: &mc_core::Aabb, b: &mc_core::Aabb) -> Option<Vec3> {
    if !a.intersects(b) {
        return None;
    }
    let d = b.center() - a.center();
    let flat = Vec3::new(d.x, 0.0, d.z);
    if flat.length_squared() < 1e-6 {
        return Some(Vec3::X);
    }
    Some(flat.normalize())
}

/// Fraction (0..1) of sample points of `target` visible from `center`.
fn exposure(world: &World, center: Vec3, target: &mc_core::Aabb) -> f32 {
    let t = target.inflate(-0.05);
    let mut seen = 0;
    let mut total = 0;
    for i in 0..9 {
        let p = if i == 8 {
            t.center()
        } else {
            Vec3::new(
                if i & 1 == 0 { t.min.x } else { t.max.x },
                if i & 2 == 0 { t.min.y } else { t.max.y },
                if i & 4 == 0 { t.min.z } else { t.max.z },
            )
        };
        total += 1;
        let d = p - center;
        let len = d.length();
        let blocked = len > 1e-4
            && mc_core::raycast::raycast_with(world, center, d / len, len, physics::is_solid)
                .is_some();
        if !blocked {
            seen += 1;
        }
    }
    seen as f32 / total as f32
}
