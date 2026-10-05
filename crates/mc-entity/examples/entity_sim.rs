//! Headless entity simulation for tuning and debugging.
//!
//! Builds a small procedural world (hills, a pond, caves, some walls), runs
//! the player + entity simulation for a number of ticks with natural
//! spawning, prints statistics and timing, checks invariants (no NaNs, no
//! mob stuck inside blocks) and writes a top-down PNG of mob trajectories.
//!
//! cargo run -p mc-entity --example entity_sim --release -- [ticks] [out.png]

use std::time::Instant;

use glam::{IVec3, Vec3};
use mc_core::input::InputState;
use mc_core::{Chunk, ChunkPos, World, blocks};
use mc_entity::{EntityManager, GameMode, MobKind, Player};

const R: i32 = 6; // chunks around the origin

fn height(x: i32, z: i32) -> i32 {
    let (fx, fz) = (x as f32, z as f32);
    (64.0 + 5.0 * (fx * 0.07).sin() + 4.0 * (fz * 0.05).cos() + 2.0 * ((fx + fz) * 0.13).sin())
        as i32
}

fn build_world() -> World {
    let mut w = World::new(1);
    for cx in -R..=R {
        for cz in -R..=R {
            let mut c = Chunk::new(ChunkPos::new(cx, cz));
            for lx in 0..16 {
                for lz in 0..16 {
                    let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                    let h = height(x, z);
                    c.set_raw(lx, mc_core::WORLD_MIN_Y, lz, blocks::BEDROCK);
                    for y in 30..=h {
                        let id = if y == h {
                            if h <= 62 {
                                blocks::SAND
                            } else {
                                blocks::GRASS_BLOCK
                            }
                        } else if y > h - 3 {
                            blocks::DIRT
                        } else {
                            blocks::STONE
                        };
                        c.set_raw(lx, y, lz, id);
                    }
                    for y in h + 1..=62 {
                        c.set_raw(lx, y, lz, blocks::WATER);
                    }
                    // A long cave tunnel.
                    let cave_y = 50 + ((x as f32 * 0.1).sin() * 3.0) as i32;
                    if (z - 20).abs() <= 1 {
                        for y in cave_y..cave_y + 3 {
                            c.set_raw(lx, y, lz, blocks::AIR);
                        }
                    }
                }
            }
            c.recompute_heightmap();
            w.insert_chunk(c);
        }
    }
    // A wall with a gap, near the player.
    for z in -12i32..=12 {
        if z.abs() > 1 {
            for dy in 1..=3 {
                let x = 8;
                let y = height(x, z) + dy;
                w.set_block(IVec3::new(x, y, z), blocks::COBBLESTONE);
            }
        }
    }
    w
}

/// Minimal PNG writer (uncompressed deflate blocks), RGB.
fn write_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..h as usize {
        raw.push(0);
        raw.extend(&rgb[y * w as usize * 3..(y + 1) * w as usize * 3]);
    }
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

fn kind_color(k: MobKind) -> [u8; 3] {
    match k {
        MobKind::Pig => [255, 150, 180],
        MobKind::Cow => [120, 70, 30],
        MobKind::Sheep => [255, 255, 255],
        MobKind::Chicken => [255, 230, 0],
        MobKind::Zombie => [0, 160, 60],
        MobKind::Skeleton => [200, 200, 200],
        MobKind::Creeper => [0, 255, 0],
        MobKind::Spider => [120, 0, 0],
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ticks: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(6000);
    let out = args.get(2).cloned();
    let mut world = build_world();
    world.time_of_day = 12000;
    let mut player = Player::new(IVec3::new(0, height(0, 0) + 1, 0));
    player.game_mode = GameMode::Survival;
    player.flying = false;
    let mut mgr = EntityManager::new(42);
    let idle = InputState::default();

    let size = (R * 2 + 1) * 16;
    let half = size / 2 + 8;
    let mut img = vec![0u8; (size * size * 3) as usize];
    for z in 0..size {
        for x in 0..size {
            let (wx, wz) = (x - half + 8, z - half + 8);
            let id = world.block(IVec3::new(wx, world.height(wx, wz).unwrap_or(0), wz));
            let c = if id == blocks::WATER {
                [30, 60, 140]
            } else if id == blocks::COBBLESTONE {
                [90, 90, 90]
            } else {
                let h = world.height(wx, wz).unwrap_or(64) - 56;
                let v = (h * 6).clamp(0, 120) as u8;
                [v / 2, 40 + v / 2, v / 3]
            };
            let i = ((z * size + x) * 3) as usize;
            img[i..i + 3].copy_from_slice(&c);
        }
    }

    let mut worst = 0f64;
    let mut total = 0f64;
    let mut max_mobs = 0;
    let mut player_hits = 0;
    let t0 = Instant::now();
    for t in 0..ticks {
        world.tick += 1;
        world.time_of_day = (world.time_of_day + 1) % mc_core::DAY_LENGTH_TICKS;
        player.tick(&idle, &world);
        if player.dead {
            player = Player::new(IVec3::new(0, height(0, 0) + 1, 0));
            player.game_mode = GameMode::Survival;
            player.flying = false;
        }
        let s = Instant::now();
        mgr.tick(&mut world, &mut player);
        let dt = s.elapsed().as_secs_f64() * 1000.0;
        worst = worst.max(dt);
        total += dt;
        for e in mgr.take_events() {
            if matches!(e, mc_entity::EntityEvent::PlayerHurt { .. }) {
                player_hits += 1;
            }
        }
        max_mobs = max_mobs.max(mgr.mobs().len());
        for m in mgr.mobs() {
            let p = m.pos();
            assert!(p.is_finite() && m.body.vel.is_finite(), "NaN mob {m:?}");
            let (x, z) = (p.x as i32 + half - 8, p.z as i32 + half - 8);
            if (0..size).contains(&x) && (0..size).contains(&z) {
                let i = ((z * size + x) * 3) as usize;
                img[i..i + 3].copy_from_slice(&kind_color(m.kind));
            }
            if m.alive() && t % 100 == 0 {
                let inside = mc_entity::physics::collides(&world, &m.aabb().inflate(-0.05));
                assert!(!inside, "{:?} stuck inside blocks at {p}", m.kind);
            }
        }
        if t % 1200 == 0 {
            let mut counts = std::collections::BTreeMap::new();
            for m in mgr.mobs() {
                *counts.entry(m.kind.name()).or_insert(0) += 1;
            }
            println!(
                "t={t:>6} time={:>5} mobs={:>3} items={:>3} arrows={:>2} player hp={:>4.1} {counts:?}",
                world.time_of_day,
                mgr.mobs().len(),
                mgr.items().len(),
                mgr.arrows().len(),
                player.health,
            );
        }
    }
    let p = player.position;
    let (px, pz) = (p.x as i32 + half - 8, p.z as i32 + half - 8);
    for dz in -2..=2 {
        for dx in -2..=2 {
            let (x, z) = (px + dx, pz + dz);
            if (0..size).contains(&x) && (0..size).contains(&z) {
                let i = ((z * size + x) * 3) as usize;
                img[i..i + 3].copy_from_slice(&[255, 0, 255]);
            }
        }
    }
    println!(
        "{ticks} ticks in {:.2?}: entity tick avg {:.3} ms, worst {:.3} ms, max mobs {max_mobs}, player hurt {player_hits}x",
        t0.elapsed(),
        total / ticks as f64,
        worst
    );
    if let Some(path) = out {
        write_png(&path, size as u32, size as u32, &img).expect("write png");
        println!("wrote {path}");
    }
    let _ = Vec3::ZERO;
}
