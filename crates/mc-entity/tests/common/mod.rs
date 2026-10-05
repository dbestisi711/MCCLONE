//! Helpers for deterministic simulation tests.
#![allow(dead_code)]

use glam::{IVec3, Vec3};
use mc_core::input::{InputState, Key};
use mc_core::{BlockId, Chunk, ChunkPos, World, blocks};
use mc_entity::{EntityManager, GameMode, Player};

/// A world of `(2r+1)²` loaded chunks with a solid floor whose top block is at
/// `floor_y` (so feet rest at `floor_y + 1`).
pub fn flat_world(r: i32, floor_y: i32, top: BlockId) -> World {
    let mut w = World::new(7);
    for cx in -r..=r {
        for cz in -r..=r {
            let mut c = Chunk::new(ChunkPos::new(cx, cz));
            for x in 0..16 {
                for z in 0..16 {
                    c.set_raw(x, mc_core::WORLD_MIN_Y, z, blocks::BEDROCK);
                    for y in floor_y - 3..floor_y {
                        c.set_raw(x, y, z, blocks::STONE);
                    }
                    c.set_raw(x, floor_y, z, top);
                }
            }
            c.recompute_heightmap();
            w.insert_chunk(c);
        }
    }
    w
}

/// Empty loaded chunks (all air).
pub fn empty_world(r: i32) -> World {
    let mut w = World::new(7);
    for cx in -r..=r {
        for cz in -r..=r {
            w.insert_chunk(Chunk::new(ChunkPos::new(cx, cz)));
        }
    }
    w
}

pub fn fill(w: &mut World, a: IVec3, b: IVec3, id: BlockId) {
    for x in a.x.min(b.x)..=a.x.max(b.x) {
        for y in a.y.min(b.y)..=a.y.max(b.y) {
            for z in a.z.min(b.z)..=a.z.max(b.z) {
                w.set_block(IVec3::new(x, y, z), id);
            }
        }
    }
}

pub fn survival_player(pos: Vec3) -> Player {
    let mut p = Player::new(IVec3::ZERO);
    p.position = pos;
    p.prev_position = pos;
    p.game_mode = GameMode::Survival;
    p.flying = false;
    p
}

pub fn input(keys: &[Key]) -> InputState {
    let mut i = InputState::default();
    for k in keys {
        i.held.insert(*k);
    }
    i
}

pub fn quiet_manager(seed: u64) -> EntityManager {
    let mut m = EntityManager::new(seed);
    m.natural_spawning = false;
    m
}

/// Yaw that looks along +X.
pub const YAW_EAST: f32 = std::f32::consts::FRAC_PI_2;

pub fn count_blocks(w: &World, a: IVec3, b: IVec3, id: BlockId) -> usize {
    let mut n = 0;
    for x in a.x..=b.x {
        for y in a.y..=b.y {
            for z in a.z..=b.z {
                if w.block(IVec3::new(x, y, z)) == id {
                    n += 1;
                }
            }
        }
    }
    n
}
