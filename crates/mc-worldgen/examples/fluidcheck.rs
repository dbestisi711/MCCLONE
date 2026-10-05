//! Count fluid problems in a generated area (dev tool): water or lava
//! directly above air ("floating"), and fluid next to air at the same height
//! (an unsupported "wall"). Neighbour checks cross chunk borders.
//!
//! ```text
//! cargo run --release -p mc-worldgen --example fluidcheck -- [--seed 12345] [--x 0] [--z 0] [--chunks 16]
//! ```

mod common;

use std::collections::HashMap;

use common::*;
use mc_core::{BlockId, Chunk, ChunkPos, WORLD_MAX_Y, WORLD_MIN_Y, blocks};
use mc_worldgen::WorldGenerator;

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let (cx, cz): (i32, i32) = (a.get("x", 0), a.get("z", 0));
    let n: i32 = a.get("chunks", 16);
    let g = WorldGenerator::new(seed);
    let c0 = ChunkPos::from_block(cx, cz);
    let mut chunks: HashMap<ChunkPos, Chunk> = HashMap::new();
    for dz in -1..=n {
        for dx in -1..=n {
            let p = c0.offset(dx, dz);
            chunks.insert(p, g.generate(p));
        }
    }
    let get = |x: i32, y: i32, z: i32| -> BlockId {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            return blocks::AIR;
        }
        chunks[&ChunkPos::from_block(x, z)].get(x & 15, y, z & 15)
    };
    let (bx, bz) = c0.min_block();
    let (mut floating, mut walls, mut wall_border, mut fluid) = (0usize, 0usize, 0usize, 0usize);
    let mut examples = Vec::new();
    for z in bz..bz + n * 16 {
        for x in bx..bx + n * 16 {
            for y in WORLD_MIN_Y + 1..WORLD_MAX_Y - 1 {
                let id = get(x, y, z);
                if id != blocks::WATER && id != blocks::LAVA {
                    continue;
                }
                fluid += 1;
                if get(x, y - 1, z).is_air() {
                    floating += 1;
                    if examples.len() < 8 {
                        examples.push((x, y, z));
                    }
                }
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if get(x + dx, y, z + dz).is_air() {
                        walls += 1;
                        if examples.is_empty() {
                            examples.push((x, y, z));
                        }
                        if (x + dx) >> 4 != x >> 4 || (z + dz) >> 4 != z >> 4 {
                            wall_border += 1;
                        }
                    }
                }
            }
        }
    }
    if let Some(&(x, y, z)) = examples.first() {
        println!("around {x} {y} {z} (top = {}):", g.probe(x, z).top);
        for yy in (y - 4..=y + 3).rev() {
            let row: Vec<String> = (-2..=2)
                .map(|dx| {
                    let n = get(x + dx, yy, z).def().name;
                    format!("{:>10}", &n[..n.len().min(10)])
                })
                .collect();
            println!("  y={yy:4} {}", row.join(" "));
        }
    }
    println!("fluid blocks {fluid}");
    println!("floating (air below) {floating}  examples {examples:?}");
    println!("side faces next to air {walls} (of which across a chunk border: {wall_border})");
}
