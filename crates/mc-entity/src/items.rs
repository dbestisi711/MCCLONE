//! Dropped item entities and falling blocks (sand, gravel).

use glam::{IVec3, Vec3};
use mc_core::{BlockId, ItemStack, World};

use crate::physics::{self, Body};

/// Edge length of a dropped item's box.
pub const ITEM_SIZE: f32 = 0.25;
pub const ITEM_GRAVITY: f32 = 0.04;
/// Dropped items vanish after 5 minutes.
pub const ITEM_DESPAWN_TICKS: u32 = 6000;
/// Ticks before a freshly dropped item can be picked up.
pub const PICKUP_DELAY: u32 = 10;
/// Pickup delay for items the player threw.
pub const THROW_PICKUP_DELAY: u32 = 40;

#[derive(Clone, Debug)]
pub struct ItemEntity {
    pub id: u64,
    pub stack: ItemStack,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub in_water: bool,
    pub age: u32,
    pub pickup_delay: u32,
    /// Random phase for spin/bob.
    pub phase: f32,
    pub light: u8,
    pub(crate) dead: bool,
}

impl ItemEntity {
    pub fn new(id: u64, stack: ItemStack, pos: Vec3, vel: Vec3, phase: f32) -> Self {
        ItemEntity {
            id,
            stack,
            pos,
            prev_pos: pos,
            vel,
            on_ground: false,
            in_water: false,
            age: 0,
            pickup_delay: PICKUP_DELAY,
            phase,
            light: 0xF0,
            dead: false,
        }
    }

    pub fn aabb(&self) -> mc_core::Aabb {
        mc_core::Aabb::from_feet(self.pos, ITEM_SIZE, ITEM_SIZE)
    }

    pub(crate) fn tick(&mut self, world: &World) {
        self.prev_pos = self.pos;
        self.age += 1;
        self.pickup_delay = self.pickup_delay.saturating_sub(1);
        if self.age >= ITEM_DESPAWN_TICKS || self.pos.y < (mc_core::WORLD_MIN_Y - 64) as f32 {
            self.dead = true;
            return;
        }
        if physics::collides(world, &self.aabb().inflate(-0.01)) {
            // Inside a block (placed on top of it, or spawned in one): pop up.
            let cell = (self.pos + Vec3::Y * (ITEM_SIZE * 0.5)).floor();
            self.pos.y = cell.y + 1.0;
            self.vel = Vec3::new(self.vel.x * 0.5, 0.05, self.vel.z * 0.5);
            self.on_ground = false;
            return;
        }
        let mut body = Body::new(self.pos, ITEM_SIZE, ITEM_SIZE);
        body.vel = self.vel;
        body.on_ground = self.on_ground;
        body.step_height = 0.0;
        physics::update_fluids(world, &mut body);
        if body.in_water || body.in_lava {
            // Float up to the surface.
            body.vel.x *= 0.95;
            body.vel.z *= 0.95;
            body.vel.y = body.vel.y * 0.9 + if body.submerged > 0.5 { 0.006 } else { -0.01 };
        } else {
            body.vel.y -= ITEM_GRAVITY;
        }
        // Items resting on the ground do not need a full sweep every tick.
        let vel = body.vel;
        if !(self.on_ground && vel.length_squared() < 1e-8) {
            physics::move_body(world, &mut body, vel, false);
        }
        let friction = if body.on_ground {
            physics::slipperiness_below(world, body.pos) * 0.98
        } else {
            0.98
        };
        body.vel.x *= friction;
        body.vel.z *= friction;
        body.vel.y *= 0.98;
        if body.vel.length_squared() < 1e-8 {
            body.vel = Vec3::ZERO;
        }
        self.pos = body.pos;
        self.vel = body.vel;
        self.on_ground = body.on_ground;
        self.in_water = body.in_water;
        if body.in_lava {
            self.dead = true;
        }
        if self.age % 10 == 1 {
            self.light = world.light((self.pos + Vec3::Y * 0.1).floor().as_ivec3());
        }
    }
}

/// A gravity block (sand, gravel) falling as an entity.
#[derive(Clone, Debug)]
pub struct FallingBlock {
    pub block: BlockId,
    /// Bottom centre.
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub age: u32,
    pub light: u8,
    pub(crate) dead: bool,
}

pub const FALLING_BLOCK_SIZE: f32 = 0.98;

impl FallingBlock {
    pub fn new(block: BlockId, cell: IVec3) -> Self {
        let pos = cell.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        FallingBlock {
            block,
            pos,
            prev_pos: pos,
            vel: Vec3::ZERO,
            age: 0,
            light: 0xF0,
            dead: false,
        }
    }

    /// Advance one tick. Returns `Some(cell)` when it landed and should
    /// become a block at `cell` (or drop as an item if that is impossible).
    pub(crate) fn tick(&mut self, world: &World) -> Option<IVec3> {
        self.prev_pos = self.pos;
        self.age += 1;
        let mut body = Body::new(self.pos, FALLING_BLOCK_SIZE, FALLING_BLOCK_SIZE);
        body.vel = self.vel;
        body.step_height = 0.0;
        body.vel.y -= ITEM_GRAVITY;
        let vel = body.vel;
        physics::move_body(world, &mut body, vel, false);
        body.vel *= 0.98;
        self.pos = body.pos;
        self.vel = body.vel;
        self.light = world.light((self.pos + Vec3::Y * 0.5).floor().as_ivec3());
        if self.pos.y < (mc_core::WORLD_MIN_Y - 64) as f32 {
            self.dead = true;
            return None;
        }
        if body.on_ground || self.age > 600 {
            self.dead = true;
            return Some((self.pos + Vec3::Y * 0.5).floor().as_ivec3());
        }
        None
    }
}

/// A falling block can drop through this block.
pub fn can_fall_through(id: BlockId) -> bool {
    let d = id.def();
    id.is_air() || d.fluid || (d.replaceable && !d.solid)
}
