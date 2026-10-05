//! Natural spawning and despawning around the player.
//!
//! - Hostiles: one attempt per tick in a random column 24..=96 blocks away
//!   (horizontally), at a random height up to the surface (so caves count).
//!   Needs block light 0 and effective sky light (darkened by time of day)
//!   at most a random 0..=7. Groups of 1–4.
//! - Passive: every 20 ticks, on the surface of grass 24..=96 blocks away
//!   with raw light ≥ 9. Groups of 2–4 of one kind.
//! - Caps count mobs within 128 blocks of the player.
//! - Hostiles despawn beyond 128 blocks, or randomly (1/800 per tick) when
//!   farther than 32 blocks after 30 s without anything happening. Passive
//!   mobs are only dropped when the player is very far away (> 192 blocks).

use glam::{IVec3, Vec3};
use mc_core::{World, blocks};

use crate::mob::{MobKind, light_levels};
use crate::path::{self, PathConfig};
use crate::physics;
use crate::{EntityManager, Player};

pub const HOSTILE_CAP: usize = 50;
pub const PASSIVE_CAP: usize = 14;
pub const SPAWN_MIN_DIST: f32 = 24.0;
pub const SPAWN_MAX_DIST: f32 = 96.0;
pub const CAP_RANGE: f32 = 128.0;
pub const HOSTILE_DESPAWN_DIST: f32 = 128.0;
pub const RANDOM_DESPAWN_DIST: f32 = 32.0;
pub const PASSIVE_DESPAWN_DIST: f32 = 192.0;

const HOSTILE_WEIGHTS: [(MobKind, u32); 4] = [
    (MobKind::Zombie, 100),
    (MobKind::Skeleton, 100),
    (MobKind::Creeper, 100),
    (MobKind::Spider, 100),
];
const PASSIVE_WEIGHTS: [(MobKind, u32); 4] = [
    (MobKind::Sheep, 12),
    (MobKind::Pig, 10),
    (MobKind::Chicken, 10),
    (MobKind::Cow, 8),
];

fn pick(weights: &[(MobKind, u32)], roll: u32) -> MobKind {
    let total: u32 = weights.iter().map(|w| w.1).sum();
    let mut r = roll % total.max(1);
    for &(k, w) in weights {
        if r < w {
            return k;
        }
        r -= w;
    }
    weights[0].0
}

/// Can a mob of `kind` stand with its feet in block `p`?
pub fn spawn_space_ok(world: &World, kind: MobKind, p: IVec3) -> bool {
    let spec = kind.spec();
    let below = match world.block_loaded(p - IVec3::Y) {
        Some(b) => b,
        None => return false,
    };
    if !(physics::is_solid(below) && below.def().occludes()) || below == blocks::BEDROCK {
        return false;
    }
    let cfg = PathConfig {
        height: spec.height.ceil() as i32,
        ..Default::default()
    };
    if path::standable_cost(world, p, &cfg).is_none() {
        return false;
    }
    if world.block(p).def().fluid {
        return false;
    }
    let pos = p.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
    let aabb = mc_core::Aabb::from_feet(pos, spec.width, spec.height);
    !physics::collides(world, &aabb)
}

impl EntityManager {
    pub(crate) fn spawn_tick(&mut self, world: &World, player: &Player) {
        let center = player.position;
        let (mut hostile, mut passive) = (0usize, 0usize);
        for m in &self.mobs {
            if (m.body.pos - center).length() <= CAP_RANGE {
                if m.kind.hostile() {
                    hostile += 1;
                } else {
                    passive += 1;
                }
            }
        }
        if hostile < HOSTILE_CAP {
            self.try_spawn_group(world, center, true);
        }
        if passive < PASSIVE_CAP && self.tick_count.is_multiple_of(20) {
            self.try_spawn_group(world, center, false);
        }
    }

    fn try_spawn_group(&mut self, world: &World, center: Vec3, hostile: bool) {
        let angle = self.rng.f32() * std::f32::consts::TAU;
        let dist = SPAWN_MIN_DIST + self.rng.f32() * (SPAWN_MAX_DIST - SPAWN_MIN_DIST);
        let x = (center.x + angle.cos() * dist).floor() as i32;
        let z = (center.z + angle.sin() * dist).floor() as i32;
        let Some(top) = world.height(x, z) else {
            return;
        };
        let (kind, base) = if hostile {
            let kind = pick(&HOSTILE_WEIGHTS, self.rng.below(1 << 20));
            let lo = mc_core::WORLD_MIN_Y + 1;
            let mut y = self.rng.range(lo, top + 1);
            // Rise out of solid rock to the first open space (cave or surface).
            for _ in 0..16 {
                let here = world.block(IVec3::new(x, y, z));
                let below = world.block(IVec3::new(x, y - 1, z));
                if !physics::is_solid(here) && physics::is_solid(below) {
                    break;
                }
                y += 1;
            }
            (kind, IVec3::new(x, y, z))
        } else {
            let kind = pick(&PASSIVE_WEIGHTS, self.rng.below(1 << 20));
            (kind, IVec3::new(x, top + 1, z))
        };
        let group = if hostile {
            self.rng.range(1, 4)
        } else {
            self.rng.range(2, 4)
        };
        let mut spawned = 0;
        let mut p = base;
        for _ in 0..group * 3 {
            if spawned >= group {
                break;
            }
            if self.can_spawn_at(world, kind, p, center, hostile) {
                let pos = p.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
                let yaw = self.rng.f32() * std::f32::consts::TAU;
                self.spawn_mob_with_yaw(kind, pos, yaw);
                spawned += 1;
            }
            // Next member nearby.
            p = base
                + IVec3::new(
                    self.rng.range(-4, 4),
                    self.rng.range(-1, 1),
                    self.rng.range(-4, 4),
                );
            if !hostile && let Some(h) = world.height(p.x, p.z) {
                p.y = h + 1;
            }
        }
    }

    fn can_spawn_at(
        &mut self,
        world: &World,
        kind: MobKind,
        p: IVec3,
        center: Vec3,
        hostile: bool,
    ) -> bool {
        let pos = p.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        if (pos - center).length() < SPAWN_MIN_DIST {
            return false;
        }
        if !spawn_space_ok(world, kind, p) {
            return false;
        }
        if hostile {
            let (sky, block) = light_levels(world, p, true);
            if block > 0 {
                return false;
            }
            let limit = self.rng.below(8) as u8;
            sky <= limit
        } else {
            if world.block(p - IVec3::Y) != blocks::GRASS_BLOCK {
                return false;
            }
            let (sky, block) = light_levels(world, p, false);
            sky.max(block) >= 9
        }
    }

    pub(crate) fn despawn_tick(&mut self, player: &Player) {
        let center = player.position;
        for i in 0..self.mobs.len() {
            let m = &mut self.mobs[i];
            if m.persistent || !m.alive() {
                continue;
            }
            let d = (m.body.pos - center).length();
            if d < RANDOM_DESPAWN_DIST {
                m.no_action_ticks = 0;
            } else {
                m.no_action_ticks += 1;
            }
            let remove = if m.kind.hostile() {
                d > HOSTILE_DESPAWN_DIST
                    || (d > RANDOM_DESPAWN_DIST && m.no_action_ticks > 600 && self.rng.one_in(800))
            } else {
                d > PASSIVE_DESPAWN_DIST
            };
            if remove {
                m.death_time = crate::mob::DEATH_TICKS;
            }
        }
    }
}
