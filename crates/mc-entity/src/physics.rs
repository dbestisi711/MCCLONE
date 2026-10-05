//! Shared AABB-vs-voxel physics for the player, mobs, items and projectiles.
//!
//! Movement is resolved per axis (Y first, then the larger horizontal axis)
//! against every block box inside the *swept* region, so even very fast
//! bodies cannot tunnel through thin floors. Chunks that are not loaded count
//! as solid so nothing falls into ungenerated terrain.
//!
//! All velocities are in blocks per tick (20 ticks per second).

use glam::{IVec3, Vec3};
use mc_core::block::Shape;
use mc_core::{Aabb, BlockId, ChunkPos, World, blocks};

/// Downward acceleration per tick² in air.
pub const GRAVITY: f32 = 0.08;
/// Vertical velocity multiplier per tick in air.
pub const VERTICAL_DRAG: f32 = 0.98;
/// Horizontal velocity multiplier per tick in air.
pub const AIR_FRICTION: f32 = 0.91;
/// Slipperiness of ordinary ground.
pub const DEFAULT_SLIPPERINESS: f32 = 0.6;
/// Slipperiness of ice and packed ice.
pub const ICE_SLIPPERINESS: f32 = 0.98;
/// Horizontal velocity multiplier per tick on ordinary ground.
pub const NORMAL_GROUND_FRICTION: f32 = DEFAULT_SLIPPERINESS * AIR_FRICTION;
/// Height a walking body climbs without jumping.
pub const STEP_HEIGHT: f32 = 0.6;
/// Initial vertical velocity of a jump.
pub const JUMP_VELOCITY: f32 = 0.42;
/// Velocity multiplier per tick in water.
pub const WATER_DRAG: f32 = 0.8;
/// Velocity multiplier per tick in lava.
pub const LAVA_DRAG: f32 = 0.5;
/// Downward acceleration per tick² inside a fluid.
pub const FLUID_GRAVITY: f32 = 0.02;
/// Acceleration from movement input inside a fluid.
pub const FLUID_ACCEL: f32 = 0.02;
/// Acceleration from movement input while airborne.
pub const AIR_ACCEL: f32 = 0.02;
/// Height of a fluid surface inside its block (when no fluid is above it).
pub const FLUID_SURFACE: f32 = 0.89;

/// Tolerance used when testing contacts. Large enough to absorb `f32`
/// rounding at coordinates of a few thousand blocks, small enough to be
/// invisible.
const EPS: f32 = 1e-3;

const FULL: (Vec3, Vec3) = (Vec3::ZERO, Vec3::ONE);

/// Collision box of a block inside its unit cell, or `None` if entities pass
/// through it.
pub fn block_shape(id: BlockId) -> Option<(Vec3, Vec3)> {
    let d = id.def();
    if !d.solid {
        return None;
    }
    match d.shape {
        Shape::None | Shape::Cross | Shape::Torch | Shape::Liquid => None,
        Shape::Cube => Some(FULL),
        Shape::Layer(n) => Some((
            Vec3::ZERO,
            Vec3::new(1.0, (n.max(1) as f32 / 16.0).min(1.0), 1.0),
        )),
        Shape::Inset => {
            let i = 1.0 / 16.0;
            Some((Vec3::new(i, 0.0, i), Vec3::new(1.0 - i, 1.0, 1.0 - i)))
        }
    }
}

/// True if a block has a collision box.
pub fn is_solid(id: BlockId) -> bool {
    block_shape(id).is_some()
}

/// Collect the world-space collision boxes of every block overlapping
/// `region` into `out` (cleared first). Unloaded chunks yield full cubes.
pub fn collect_boxes(world: &World, region: &Aabb, out: &mut Vec<Aabb>) {
    out.clear();
    let min = region.min.floor().as_ivec3();
    let max = (region.max - Vec3::splat(1e-6)).floor().as_ivec3();
    // Absurd regions (teleports, NaN guards) are not worth scanning.
    if (max - min).max_element() > 128 || max.cmplt(min).any() {
        return;
    }
    for x in min.x..=max.x {
        for z in min.z..=max.z {
            let chunk = world.chunk(ChunkPos::from_block(x, z));
            for y in min.y..=max.y {
                let shape = match chunk {
                    Some(c) => block_shape(c.get(x & 15, y, z & 15)),
                    None => Some(FULL),
                };
                if let Some((lo, hi)) = shape {
                    let o = Vec3::new(x as f32, y as f32, z as f32);
                    out.push(Aabb::new(o + lo, o + hi));
                }
            }
        }
    }
}

/// True if `aabb` overlaps any collision box (contacts within the tolerance
/// do not count).
pub fn collides(world: &World, aabb: &Aabb) -> bool {
    let shrunk = aabb.inflate(-EPS);
    let mut boxes = Vec::new();
    collect_boxes(world, aabb, &mut boxes);
    boxes.iter().any(|b| b.intersects(&shrunk))
}

fn overlaps_other_axes(b: &Aabb, a: &Aabb, axis: usize) -> bool {
    (0..3).filter(|&o| o != axis).all(|o| {
        let lo = b.min[o].max(a.min[o]);
        let hi = b.max[o].min(a.max[o]);
        hi - lo > EPS
    })
}

/// Clip a movement `d` along `axis` so that `a` stops at the first box in the
/// way. Boxes `a` already overlaps are ignored so stuck bodies can escape.
fn clip_axis(boxes: &[Aabb], a: &Aabb, axis: usize, mut d: f32) -> f32 {
    if d == 0.0 {
        return 0.0;
    }
    for b in boxes {
        if !overlaps_other_axes(b, a, axis) {
            continue;
        }
        if d > 0.0 && a.max[axis] <= b.min[axis] + EPS {
            d = d.min(b.min[axis] - a.max[axis]);
        } else if d < 0.0 && a.min[axis] >= b.max[axis] - EPS {
            d = d.max(b.max[axis] - a.min[axis]);
        }
    }
    d
}

fn sweep_boxes(boxes: &[Aabb], aabb: &Aabb, delta: Vec3) -> Vec3 {
    let mut a = *aabb;
    let mut out = Vec3::ZERO;
    if delta.y != 0.0 {
        out.y = clip_axis(boxes, &a, 1, delta.y);
        a = a.translate(Vec3::new(0.0, out.y, 0.0));
    }
    let order = if delta.x.abs() >= delta.z.abs() {
        [0, 2]
    } else {
        [2, 0]
    };
    for axis in order {
        if delta[axis] != 0.0 {
            let d = clip_axis(boxes, &a, axis, delta[axis]);
            let mut t = Vec3::ZERO;
            t[axis] = d;
            a = a.translate(t);
            out[axis] = d;
        }
    }
    out
}

/// Move a box through the world, returning the movement actually possible.
pub fn sweep(world: &World, aabb: &Aabb, delta: Vec3) -> Vec3 {
    let mut boxes = Vec::new();
    collect_boxes(world, &aabb.expand(delta), &mut boxes);
    sweep_boxes(&boxes, aabb, delta)
}

/// Physical state shared by everything that moves.
#[derive(Clone, Debug)]
pub struct Body {
    /// Bottom centre of the box.
    pub pos: Vec3,
    /// Blocks per tick.
    pub vel: Vec3,
    pub width: f32,
    pub height: f32,
    /// Height climbed automatically when walking into a ledge (0 = none).
    pub step_height: f32,
    pub on_ground: bool,
    /// Blocked horizontally during the last move.
    pub collided_h: bool,
    /// Blocked vertically during the last move.
    pub collided_v: bool,
    pub in_water: bool,
    pub in_lava: bool,
    /// Fraction (0..1) of the box height below a fluid surface.
    pub submerged: f32,
    /// Distance fallen since last touching the ground (or a fluid).
    pub fall_distance: f32,
}

impl Body {
    pub fn new(pos: Vec3, width: f32, height: f32) -> Self {
        Body {
            pos,
            vel: Vec3::ZERO,
            width,
            height,
            step_height: STEP_HEIGHT,
            on_ground: false,
            collided_h: false,
            collided_v: false,
            in_water: false,
            in_lava: false,
            submerged: 0.0,
            fall_distance: 0.0,
        }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::from_feet(self.pos, self.width, self.height)
    }

    pub fn in_fluid(&self) -> bool {
        self.in_water || self.in_lava
    }
}

/// Result of [`move_body`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveOutcome {
    /// Movement actually applied.
    pub moved: Vec3,
    /// Set when the body touched down this move: the distance it fell.
    pub landed_fall: Option<f32>,
    /// The body stepped up onto a ledge.
    pub stepped: bool,
}

fn toward_zero(v: f32, step: f32) -> f32 {
    if v.abs() <= step {
        0.0
    } else {
        v - step * v.signum()
    }
}

/// Reduce a horizontal movement so the box keeps something below it within
/// `probe` blocks (sneaking never walks off an edge).
fn edge_guard(world: &World, aabb: &Aabb, mut d: Vec3, probe: f32) -> Vec3 {
    const STEP: f32 = 0.05;
    let supported = |dx: f32, dz: f32| collides(world, &aabb.translate(Vec3::new(dx, -probe, dz)));
    while d.x != 0.0 && !supported(d.x, 0.0) {
        d.x = toward_zero(d.x, STEP);
    }
    while d.z != 0.0 && !supported(0.0, d.z) {
        d.z = toward_zero(d.z, STEP);
    }
    while d.x != 0.0 && d.z != 0.0 && !supported(d.x, d.z) {
        d.x = toward_zero(d.x, STEP);
        d.z = toward_zero(d.z, STEP);
    }
    d
}

/// Move `body` by `delta` with collision, step-up, optional edge guard
/// (sneaking), fluid detection and fall-distance tracking. Velocity
/// components that were blocked are zeroed.
pub fn move_body(world: &World, body: &mut Body, delta: Vec3, edge_guarded: bool) -> MoveOutcome {
    let aabb = body.aabb();
    let mut delta = delta;
    if !delta.is_finite() {
        delta = Vec3::ZERO;
        body.vel = Vec3::ZERO;
    }
    if edge_guarded && body.on_ground && delta.y <= 0.0 {
        let guarded = edge_guard(world, &aabb, delta, body.step_height.max(0.5));
        if guarded.x != delta.x {
            body.vel.x = 0.0;
        }
        if guarded.z != delta.z {
            body.vel.z = 0.0;
        }
        delta = guarded;
    }

    let mut boxes = Vec::new();
    collect_boxes(world, &aabb.expand(delta), &mut boxes);
    let mut moved = sweep_boxes(&boxes, &aabb, delta);
    let mut hit_y = moved.y != delta.y;
    let mut on_ground = hit_y && delta.y < 0.0;
    let mut stepped = false;

    let blocked_h = moved.x != delta.x || moved.z != delta.z;
    if body.step_height > 0.0 && blocked_h && (body.on_ground || on_ground) {
        let horizontal = Vec3::new(delta.x, 0.0, delta.z);
        let region = aabb
            .expand(horizontal)
            .expand(Vec3::Y * body.step_height)
            .expand(Vec3::Y * delta.y.min(0.0));
        collect_boxes(world, &region, &mut boxes);
        let up = clip_axis(&boxes, &aabb, 1, body.step_height);
        let raised = aabb.translate(Vec3::Y * up);
        let h = sweep_boxes(&boxes, &raised, horizontal);
        let shifted = raised.translate(Vec3::new(h.x, 0.0, h.z));
        let down_wanted = -up + delta.y.min(0.0);
        let down = clip_axis(&boxes, &shifted, 1, down_wanted);
        if h.x * h.x + h.z * h.z > moved.x * moved.x + moved.z * moved.z + 1e-7 {
            moved = Vec3::new(h.x, up + down, h.z);
            stepped = true;
            hit_y = down != down_wanted;
            on_ground = hit_y;
        }
    }

    body.pos += moved;
    let hit_x = moved.x != delta.x;
    let hit_z = moved.z != delta.z;
    if hit_x {
        body.vel.x = 0.0;
    }
    if hit_z {
        body.vel.z = 0.0;
    }
    if hit_y || stepped {
        body.vel.y = 0.0;
    }
    body.collided_h = hit_x || hit_z;
    body.collided_v = hit_y;
    body.on_ground = on_ground;

    update_fluids(world, body);
    let mut outcome = MoveOutcome {
        moved,
        landed_fall: None,
        stepped,
    };
    if moved.y < 0.0 {
        body.fall_distance -= moved.y;
    }
    if body.in_water {
        body.fall_distance = 0.0;
    } else if body.on_ground {
        if body.fall_distance > 0.0 {
            outcome.landed_fall = Some(body.fall_distance);
        }
        body.fall_distance = 0.0;
    }
    outcome
}

/// Height of the fluid surface in the block at `p` (relative to `p.y`).
fn fluid_top(world: &World, p: IVec3, id: BlockId) -> f32 {
    if world.block(p + IVec3::Y) == id {
        1.0
    } else {
        FLUID_SURFACE
    }
}

/// Refresh `in_water`, `in_lava` and `submerged` for the body's box.
pub fn update_fluids(world: &World, body: &mut Body) {
    let a = body.aabb().inflate(-0.001);
    let (min, max) = a.block_range();
    body.in_water = false;
    body.in_lava = false;
    let mut top = f32::NEG_INFINITY;
    for x in min.x..=max.x {
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                let p = IVec3::new(x, y, z);
                let id = world.block(p);
                if id != blocks::WATER && id != blocks::LAVA {
                    continue;
                }
                let surface = y as f32 + fluid_top(world, p, id);
                if a.min.y >= surface {
                    continue;
                }
                if id == blocks::WATER {
                    body.in_water = true;
                } else {
                    body.in_lava = true;
                }
                top = top.max(surface);
            }
        }
    }
    body.submerged = if top.is_finite() {
        ((top - a.min.y) / body.height.max(0.01)).clamp(0.0, 1.0)
    } else {
        0.0
    };
}

/// Fluid (water or lava) the point is inside of, honouring the surface height.
pub fn fluid_at_point(world: &World, p: Vec3) -> Option<BlockId> {
    let b = p.floor().as_ivec3();
    let id = world.block(b);
    if (id == blocks::WATER || id == blocks::LAVA) && p.y < b.y as f32 + fluid_top(world, b, id) {
        Some(id)
    } else {
        None
    }
}

/// Slipperiness of the block under the body's feet.
pub fn slipperiness_below(world: &World, pos: Vec3) -> f32 {
    let p = Vec3::new(pos.x, pos.y - 0.5, pos.z).floor().as_ivec3();
    let id = world.block(p);
    if id == blocks::ICE || id == blocks::PACKED_ICE {
        ICE_SLIPPERINESS
    } else {
        DEFAULT_SLIPPERINESS
    }
}

/// Ground acceleration that gives a steady-state speed of `speed` blocks/tick
/// on ordinary ground. Slippery ground (higher `friction`) accelerates slower
/// but glides further.
pub fn ground_accel(speed: f32, friction: f32) -> f32 {
    let n = NORMAL_GROUND_FRICTION;
    speed * (1.0 - n) * (n / friction).powi(3)
}

/// Any block of `id` overlapping the box grown by `grow`.
pub fn touches_block(world: &World, aabb: &Aabb, grow: f32, id: BlockId) -> bool {
    let (min, max) = aabb.inflate(grow).block_range();
    for x in min.x..=max.x {
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                if world.block(IVec3::new(x, y, z)) == id {
                    return true;
                }
            }
        }
    }
    false
}

/// Is the point inside a full opaque block (suffocation)? Unloaded chunks
/// do not count.
pub fn in_opaque_block(world: &World, p: Vec3) -> bool {
    world
        .block_loaded(p.floor().as_ivec3())
        .is_some_and(|id| id.def().occludes() && is_solid(id))
}

/// Is the body inside a climbable block (vines)?
pub fn is_climbable_at(world: &World, pos: Vec3) -> bool {
    world.block(pos.floor().as_ivec3()) == blocks::VINE
}

/// Line of sight between two points (only full opaque blocks block it).
pub fn line_of_sight(world: &World, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    if len < 1e-4 {
        return true;
    }
    mc_core::raycast::raycast_with(world, from, d / len, len, |id| {
        id.def().occludes() && is_solid(id)
    })
    .is_none()
}

/// Ray vs box slab test. Returns the entry distance along `dir` (normalised).
pub fn ray_aabb(origin: Vec3, dir: Vec3, b: &Aabb) -> Option<f32> {
    let mut t0 = 0.0f32;
    let mut t1 = f32::INFINITY;
    for a in 0..3 {
        if dir[a].abs() < 1e-9 {
            if origin[a] < b.min[a] || origin[a] > b.max[a] {
                return None;
            }
        } else {
            let inv = 1.0 / dir[a];
            let mut ta = (b.min[a] - origin[a]) * inv;
            let mut tb = (b.max[a] - origin[a]) * inv;
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::{Chunk, ChunkPos};

    fn flat_world() -> World {
        let mut w = World::new(1);
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.insert_chunk(Chunk::new(ChunkPos::new(cx, cz)));
            }
        }
        for x in -16..32 {
            for z in -16..32 {
                w.set_block(IVec3::new(x, 63, z), blocks::STONE);
            }
        }
        w
    }

    #[test]
    fn shapes() {
        assert_eq!(block_shape(blocks::STONE), Some(FULL));
        assert_eq!(block_shape(blocks::AIR), None);
        assert_eq!(block_shape(blocks::WATER), None);
        assert_eq!(block_shape(blocks::TORCH), None);
        assert_eq!(block_shape(blocks::SHORT_GRASS), None);
        let (lo, hi) = block_shape(blocks::CACTUS).unwrap();
        assert!(lo.x > 0.0 && hi.x < 1.0 && hi.y == 1.0);
        let (_, hi) = block_shape(blocks::LILY_PAD).unwrap();
        assert!((hi.y - 1.0 / 16.0).abs() < 1e-6);
    }

    #[test]
    fn fast_fall_does_not_tunnel() {
        let w = flat_world();
        let mut b = Body::new(Vec3::new(0.5, 80.0, 0.5), 0.6, 1.8);
        // Far more than a block per tick.
        let out = move_body(&w, &mut b, Vec3::new(0.0, -30.0, 0.0), false);
        assert!(b.on_ground);
        assert!((b.pos.y - 64.0).abs() < 1e-3, "{}", b.pos.y);
        assert!((out.landed_fall.unwrap() - 16.0).abs() < 1e-3);
    }

    #[test]
    fn unloaded_is_solid() {
        let w = World::new(1);
        let mut b = Body::new(Vec3::new(0.5, 80.0, 0.5), 0.6, 1.8);
        // Starts inside "solid" unloaded space: overlapping boxes are ignored,
        // so it may move, but a body outside cannot enter.
        let a = Aabb::from_feet(Vec3::new(0.5, 80.0, 0.5), 0.6, 1.8);
        assert!(collides(&w, &a));
        let _ = move_body(&w, &mut b, Vec3::new(0.0, -1.0, 0.0), false);
    }

    #[test]
    fn ray_box() {
        let b = Aabb::new(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
        let t = ray_aabb(Vec3::new(0.0, 0.5, 0.5), Vec3::X, &b).unwrap();
        assert!((t - 1.0).abs() < 1e-6);
        assert!(ray_aabb(Vec3::new(0.0, 0.5, 0.5), Vec3::NEG_X, &b).is_none());
    }
}
