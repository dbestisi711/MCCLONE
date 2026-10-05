//! Arrows: ballistic projectiles that stick in blocks and hurt what they hit.

use glam::{IVec3, Vec2, Vec3};
use mc_core::World;

use crate::EntityEvent;
use crate::mob::{Mob, pitch_of, yaw_of};
use crate::physics::{self, ray_aabb};
use crate::player::{DamageSource, Player};

pub const ARROW_GRAVITY: f32 = 0.05;
pub const ARROW_DRAG: f32 = 0.99;
pub const ARROW_WATER_DRAG: f32 = 0.6;
/// Stuck arrows disappear after a minute.
pub const ARROW_LIFETIME: u32 = 1200;

#[derive(Clone, Debug)]
pub struct Arrow {
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub prev_yaw: f32,
    pub prev_pitch: f32,
    /// Block the arrow is stuck in.
    pub stuck_in: Option<IVec3>,
    pub age: u32,
    /// Damage per block/tick of speed.
    pub base_damage: f32,
    /// Mob that shot it (not hit by its own arrow).
    pub shooter: Option<u64>,
    pub light: u8,
    /// Wobble after hitting a block, counts down.
    pub shake: u32,
    pub(crate) dead: bool,
}

impl Arrow {
    pub fn new(pos: Vec3, vel: Vec3, shooter: Option<u64>, base_damage: f32) -> Self {
        let yaw = yaw_of(vel);
        let pitch = pitch_of(vel);
        Arrow {
            pos,
            prev_pos: pos,
            vel,
            yaw,
            pitch,
            prev_yaw: yaw,
            prev_pitch: pitch,
            stuck_in: None,
            age: 0,
            base_damage,
            shooter,
            light: 0xF0,
            shake: 0,
            dead: false,
        }
    }

    pub(crate) fn tick(
        &mut self,
        world: &World,
        player: &mut Player,
        mobs: &mut [Mob],
        events: &mut Vec<EntityEvent>,
    ) {
        self.prev_pos = self.pos;
        self.prev_yaw = self.yaw;
        self.prev_pitch = self.pitch;
        self.age += 1;
        self.shake = self.shake.saturating_sub(1);
        self.light = world.light(self.pos.floor().as_ivec3());
        if let Some(b) = self.stuck_in {
            if physics::is_solid(world.block(b)) {
                if self.age > ARROW_LIFETIME {
                    self.dead = true;
                }
                return;
            }
            self.stuck_in = None;
            self.vel = Vec3::ZERO;
        }
        if self.pos.y < (mc_core::WORLD_MIN_Y - 64) as f32 || self.age > ARROW_LIFETIME * 2 {
            self.dead = true;
            return;
        }

        let len = self.vel.length();
        if len > 1e-6 {
            let dir = self.vel / len;
            let block_hit = mc_core::raycast::raycast_with(world, self.pos, dir, len, |id| {
                physics::is_solid(id)
            });
            let mut nearest = block_hit.map(|h| h.distance).unwrap_or(len);
            enum Hit {
                Player,
                Mob(usize),
            }
            let mut hit = None;
            if !player.dead
                && let Some(t) = ray_aabb(self.pos, dir, &player.aabb().inflate(0.3))
                && t <= nearest
            {
                nearest = t;
                hit = Some(Hit::Player);
            }
            for (i, m) in mobs.iter().enumerate() {
                if !m.alive() || (Some(m.id) == self.shooter && self.age < 20) {
                    continue;
                }
                if let Some(t) = ray_aabb(self.pos, dir, &m.aabb().inflate(0.3))
                    && t <= nearest
                {
                    nearest = t;
                    hit = Some(Hit::Mob(i));
                }
            }
            let damage = (self.base_damage * len).ceil().clamp(1.0, 10.0);
            let from = self.pos - dir * 0.5;
            match hit {
                Some(Hit::Player) => {
                    player.damage(damage, DamageSource::Projectile { from });
                    events.push(EntityEvent::sound("random.bowhit", self.pos));
                    self.dead = true;
                    return;
                }
                Some(Hit::Mob(i)) => {
                    mobs[i].hurt(damage, DamageSource::Projectile { from });
                    events.push(EntityEvent::sound("random.bowhit", self.pos));
                    self.dead = true;
                    return;
                }
                None => {}
            }
            if let Some(h) = block_hit {
                self.pos += dir * (h.distance - 0.05).max(0.0);
                self.stuck_in = Some(h.block);
                self.vel = Vec3::ZERO;
                self.shake = 7;
                self.age = 0;
                events.push(EntityEvent::sound("random.bowhit", self.pos));
                return;
            }
            self.pos += self.vel;
            self.yaw = yaw_of(self.vel);
            self.pitch = self.vel.y.atan2(Vec2::new(self.vel.x, self.vel.z).length());
        }
        let drag = if physics::fluid_at_point(world, self.pos).is_some() {
            ARROW_WATER_DRAG
        } else {
            ARROW_DRAG
        };
        self.vel *= drag;
        self.vel.y -= ARROW_GRAVITY;
    }
}
