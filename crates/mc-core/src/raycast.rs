//! Voxel raycasting (Amanatides & Woo DDA), used for block targeting.

use glam::{IVec3, Vec3};

use crate::{BlockId, Face, World};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub block: IVec3,
    pub id: BlockId,
    /// Face of `block` that was hit (place position is `block + face.normal()`).
    pub face: Face,
    pub distance: f32,
    pub point: Vec3,
}

/// Cast a ray and return the first block for which `hit(id)` is true.
pub fn raycast_with(
    world: &World,
    origin: Vec3,
    dir: Vec3,
    max_dist: f32,
    mut hit: impl FnMut(BlockId) -> bool,
) -> Option<RayHit> {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let mut cell = origin.floor().as_ivec3();
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let inv = Vec3::new(1.0 / dir.x.abs(), 1.0 / dir.y.abs(), 1.0 / dir.z.abs());
    let next_boundary = |o: f32, c: i32, s: i32| -> f32 {
        if s > 0 {
            (c as f32 + 1.0) - o
        } else {
            o - c as f32
        }
    };
    let mut t_max = Vec3::new(
        if step.x != 0 {
            next_boundary(origin.x, cell.x, step.x) * inv.x
        } else {
            f32::INFINITY
        },
        if step.y != 0 {
            next_boundary(origin.y, cell.y, step.y) * inv.y
        } else {
            f32::INFINITY
        },
        if step.z != 0 {
            next_boundary(origin.z, cell.z, step.z) * inv.z
        } else {
            f32::INFINITY
        },
    );
    let t_delta = Vec3::new(
        if step.x != 0 { inv.x } else { f32::INFINITY },
        if step.y != 0 { inv.y } else { f32::INFINITY },
        if step.z != 0 { inv.z } else { f32::INFINITY },
    );
    let mut face = Face::Up;
    let mut t = 0.0;
    while t <= max_dist {
        let id = world.block(cell);
        if hit(id) {
            return Some(RayHit {
                block: cell,
                id,
                face,
                distance: t,
                point: origin + dir * t,
            });
        }
        if t_max.x < t_max.y && t_max.x < t_max.z {
            cell.x += step.x;
            t = t_max.x;
            t_max.x += t_delta.x;
            face = if step.x > 0 { Face::West } else { Face::East };
        } else if t_max.y < t_max.z {
            cell.y += step.y;
            t = t_max.y;
            t_max.y += t_delta.y;
            face = if step.y > 0 { Face::Down } else { Face::Up };
        } else {
            cell.z += step.z;
            t = t_max.z;
            t_max.z += t_delta.z;
            face = if step.z > 0 { Face::North } else { Face::South };
        }
    }
    None
}

/// Cast for the first targetable block (anything with a shape, ignoring fluids).
pub fn raycast(world: &World, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit> {
    raycast_with(world, origin, dir, max_dist, |id| {
        let d = id.def();
        !id.is_air() && !d.fluid
    })
}

fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, ChunkPos, blocks};

    #[test]
    fn hits_block_below() {
        let mut w = World::new(0);
        w.insert_chunk(Chunk::new(ChunkPos::new(0, 0)));
        w.set_block(IVec3::new(2, 60, 2), blocks::STONE);
        let hit = raycast(&w, Vec3::new(2.5, 62.5, 2.5), Vec3::NEG_Y, 5.0).unwrap();
        assert_eq!(hit.block, IVec3::new(2, 60, 2));
        assert_eq!(hit.face, Face::Up);
        assert!((hit.distance - 1.5).abs() < 1e-4);
    }
}
