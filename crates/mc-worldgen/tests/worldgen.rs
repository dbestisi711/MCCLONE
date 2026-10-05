//! Integration tests for the terrain generator.

use mc_core::{Chunk, ChunkPos, WORLD_MAX_Y, WORLD_MIN_Y, blocks};
use mc_worldgen::WorldGenerator;

fn same_chunk(a: &Chunk, b: &Chunk) -> bool {
    if a.biomes != b.biomes || a.heightmap != b.heightmap {
        return false;
    }
    for z in 0..16 {
        for x in 0..16 {
            for y in WORLD_MIN_Y..WORLD_MAX_Y {
                if a.get(x, y, z) != b.get(x, y, z) {
                    return false;
                }
            }
        }
    }
    true
}

#[test]
fn generation_is_deterministic() {
    let g1 = WorldGenerator::new(42);
    let g2 = WorldGenerator::new(42);
    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(-3, 7),
        ChunkPos::new(120, -45),
    ] {
        let a = g1.generate(pos);
        let b = g2.generate(pos);
        assert!(same_chunk(&a, &b), "chunk {pos:?} differs between runs");
    }
    // A different seed gives a different world.
    let g3 = WorldGenerator::new(43);
    assert!(!same_chunk(
        &g1.generate(ChunkPos::new(0, 0)),
        &g3.generate(ChunkPos::new(0, 0))
    ));
}

#[test]
fn generation_is_order_and_thread_independent() {
    let g = std::sync::Arc::new(WorldGenerator::new(7));
    let positions: Vec<ChunkPos> = (0..6).map(|i| ChunkPos::new(i * 3 - 7, 5 - i)).collect();
    let serial: Vec<Chunk> = positions.iter().rev().map(|p| g.generate(*p)).collect();
    let handles: Vec<_> = positions
        .iter()
        .map(|p| {
            let g = g.clone();
            let p = *p;
            std::thread::spawn(move || g.generate(p))
        })
        .collect();
    let parallel: Vec<Chunk> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (i, c) in parallel.iter().enumerate() {
        let s = &serial[positions.len() - 1 - i];
        assert!(same_chunk(c, s), "chunk {:?} differs", positions[i]);
    }
}

fn is_log(id: mc_core::BlockId) -> bool {
    let n = id.def().name;
    n.ends_with("_log") || id == blocks::MUSHROOM_STEM
}

fn is_leaves(id: mc_core::BlockId) -> bool {
    let n = id.def().name;
    n.ends_with("_leaves") || id == blocks::BROWN_MUSHROOM_BLOCK || id == blocks::RED_MUSHROOM_BLOCK
}

/// Generate a 3×3 area and check that trees are not cut at chunk borders:
/// every leaf block has a trunk/branch nearby, and every trunk top near a
/// border has leaves around it, even when they lie in another chunk.
#[test]
fn features_are_seamless_across_chunk_borders() {
    let g = WorldGenerator::new(2024);
    // Search for a forested area so the test is meaningful.
    let mut center = None;
    'search: for i in 0..400 {
        let (cx, cz) = ((i % 20) * 7 - 70, (i / 20) * 7 - 70);
        let b = g.biome_at(cx * 16 + 8, cz * 16 + 8);
        if matches!(
            b,
            mc_core::biome::biomes::FOREST
                | mc_core::biome::biomes::DARK_FOREST
                | mc_core::biome::biomes::JUNGLE
                | mc_core::biome::biomes::TAIGA
                | mc_core::biome::biomes::BIRCH_FOREST
        ) {
            center = Some(ChunkPos::new(cx, cz));
            break 'search;
        }
    }
    let center = center.expect("no forest found near the origin");
    let mut chunks = std::collections::HashMap::new();
    for dz in -2..=2 {
        for dx in -2..=2 {
            let p = center.offset(dx, dz);
            chunks.insert(p, g.generate(p));
        }
    }
    let block = |x: i32, y: i32, z: i32| -> mc_core::BlockId {
        let p = ChunkPos::from_block(x, z);
        chunks
            .get(&p)
            .map(|c| c.get(x & 15, y, z & 15))
            .unwrap_or(blocks::AIR)
    };
    let (bx, bz) = center.min_block();
    let mut leaves_checked = 0;
    let mut trunks_near_border = 0;
    let mut lopsided = 0;
    for dz in -16..32 {
        for dx in -16..32 {
            let (x, z) = (bx + dx, bz + dz);
            for y in 40..260 {
                let id = block(x, y, z);
                if is_leaves(id) {
                    leaves_checked += 1;
                    let mut found = false;
                    'n: for oy in -6..=4 {
                        for oz in -6..=6 {
                            for ox in -6..=6 {
                                if is_log(block(x + ox, y + oy, z + oz)) {
                                    found = true;
                                    break 'n;
                                }
                            }
                        }
                    }
                    assert!(found, "floating leaves at {x} {y} {z} (cut tree?)");
                }
                // Trunk tops: a log with air/leaves above and no log above.
                if is_log(id) && !is_log(block(x, y + 1, z)) && !is_log(block(x, y - 1, z)) {
                    continue;
                }
                if is_log(id) && !is_log(block(x, y + 1, z)) {
                    let near_border =
                        (x & 15) <= 1 || (x & 15) >= 14 || (z & 15) <= 1 || (z & 15) >= 14;
                    if near_border {
                        trunks_near_border += 1;
                        let mut leaves = 0;
                        for oy in -3..=3 {
                            for oz in -3..=3 {
                                for ox in -3..=3 {
                                    if is_leaves(block(x + ox, y + oy, z + oz)) {
                                        leaves += 1;
                                    }
                                }
                            }
                        }
                        assert!(leaves > 0, "trunk without canopy at {x} {y} {z}");
                        // A tree cut along a border would lose a whole side.
                        let mut sides = [0; 4];
                        for oy in -4..=2 {
                            for oz in -3i32..=3 {
                                for ox in -3i32..=3 {
                                    if is_leaves(block(x + ox, y + oy, z + oz)) {
                                        if ox > 0 {
                                            sides[0] += 1;
                                        }
                                        if ox < 0 {
                                            sides[1] += 1;
                                        }
                                        if oz > 0 {
                                            sides[2] += 1;
                                        }
                                        if oz < 0 {
                                            sides[3] += 1;
                                        }
                                    }
                                }
                            }
                        }
                        if sides.iter().any(|s| *s == 0) {
                            lopsided += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        leaves_checked > 100,
        "expected trees in the test area, found {leaves_checked} leaves"
    );
    assert!(trunks_near_border > 0, "no trunk near a border to check");
    // Allow the odd tree squeezed against a cliff or another trunk.
    assert!(
        lopsided * 10 <= trunks_near_border,
        "{lopsided} of {trunks_near_border} border trees are missing a side"
    );
}

#[test]
fn spawn_point_is_on_solid_dry_ground() {
    for seed in [1u64, 12345, 999] {
        let g = WorldGenerator::new(seed);
        let s = g.spawn_point();
        let chunk = g.generate(ChunkPos::from_block(s.x, s.z));
        let (lx, lz) = (s.x & 15, s.z & 15);
        let feet = chunk.get(lx, s.y, lz);
        let head = chunk.get(lx, s.y + 1, lz);
        let below = chunk.get(lx, s.y - 1, lz);
        assert!(
            !feet.def().solid && !feet.def().fluid,
            "seed {seed}: feet in {:?}",
            feet.def().name
        );
        assert!(
            !head.def().solid && !head.def().fluid,
            "seed {seed}: head in {:?}",
            head.def().name
        );
        assert!(
            below.def().solid && !below.def().fluid,
            "seed {seed}: standing on {:?}",
            below.def().name
        );
        assert!(
            s.y > mc_worldgen::SEA_LEVEL,
            "seed {seed}: spawn below sea level"
        );
    }
}

#[test]
fn world_has_expected_layers() {
    let g = WorldGenerator::new(5);
    let c = g.generate(ChunkPos::new(3, 3));
    for z in 0..16 {
        for x in 0..16 {
            assert_eq!(c.get(x, WORLD_MIN_Y, z), blocks::BEDROCK);
        }
    }
    // Deepslate dominates deep down, stone higher up.
    let count = |y: i32, id| (0..256).filter(|i| c.get(i % 16, y, i / 16) == id).count();
    assert!(count(-30, blocks::DEEPSLATE) > count(-30, blocks::STONE));
    assert!(count(30, blocks::STONE) + count(30, blocks::WATER) > count(30, blocks::DEEPSLATE));
    // Biomes are filled in.
    let b = c.biome(0, 0);
    assert!(b.def().name.len() > 1);
    let _ = g.biome_at(0, 0);
}

/// Aquifers and carvers must never leave water or lava hanging over air or
/// standing next to air at the same height, including across chunk borders.
#[test]
fn no_floating_fluids() {
    for (seed, cx, cz) in [
        (12345u64, 70, 120),
        (12345, -12, 24),
        (1, 18, -45),
        (77, -5, -5),
    ] {
        let g = WorldGenerator::new(seed);
        let n = 6;
        let mut chunks = std::collections::HashMap::new();
        for dz in -1..=n {
            for dx in -1..=n {
                let p = ChunkPos::new(cx + dx, cz + dz);
                chunks.insert(p, g.generate(p));
            }
        }
        let get =
            |x: i32, y: i32, z: i32| chunks[&ChunkPos::from_block(x, z)].get(x & 15, y, z & 15);
        let (bx, bz) = (cx * 16, cz * 16);
        for z in bz..bz + n * 16 {
            for x in bx..bx + n * 16 {
                for y in WORLD_MIN_Y + 1..WORLD_MAX_Y - 1 {
                    let id = get(x, y, z);
                    if id != blocks::WATER && id != blocks::LAVA {
                        continue;
                    }
                    assert!(
                        !get(x, y - 1, z).is_air(),
                        "seed {seed}: fluid over air at {x} {y} {z}"
                    );
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        assert!(
                            !get(x + dx, y, z + dz).is_air(),
                            "seed {seed}: fluid next to air at {x} {y} {z}"
                        );
                    }
                }
            }
        }
    }
}
