//! Top-down terrain map renderer (dev tool).
//!
//! ```text
//! cargo run --release -p mc-worldgen --example map -- \
//!     [--seed 12345] [--x 0] [--z 0] [--size 2048] [--scale 1] [--fast] \
//!     [--threads 4] [--out target/worldgen/map]
//! ```
//!
//! Writes `<out>_biomes.png` (biome colours with hillshading) and, unless
//! `--fast` is given, `<out>_surface.png` (actual top blocks, water tinted by
//! depth). `--fast` only evaluates the climate splines (no 3D terrain), which
//! is good for iterating on the biome layout over very large areas.

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use common::*;
use mc_core::{BiomeId, BlockId, ChunkPos, blocks};
use mc_worldgen::{SEA_LEVEL, WorldGenerator};

#[derive(Clone, Copy, Default)]
struct Px {
    /// Terrain (floor) height.
    floor: f32,
    /// Water depth above the floor (0 = dry).
    water: f32,
    top: BlockId,
    biome: BiomeId,
    /// Block at the `--ylevel` slice, if requested.
    slice: BlockId,
    /// Number of cave air blocks below the surface.
    cave_air: u16,
}

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let cx: i32 = a.get("x", 0);
    let cz: i32 = a.get("z", 0);
    let size: i32 = a.get("size", 2048);
    let scale: i32 = a.get("scale", 1).max(1);
    let fast = a.flag("fast");
    let threads: usize = a.get("threads", 4);
    let out: String = a.get("out", "target/worldgen/map".to_string());
    let ylevel: Option<i32> =
        a.0.iter()
            .position(|s| s == "--ylevel")
            .map(|_| a.get("ylevel", 0));

    let generator = WorldGenerator::new(seed);
    let w = (size / scale) as usize;
    let x0 = cx - size / 2;
    let z0 = cz - size / 2;
    let pixels = Mutex::new(vec![Px::default(); w * w]);
    let t0 = Instant::now();

    if fast {
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let row = next.fetch_add(1, Ordering::Relaxed);
                        if row >= w {
                            break;
                        }
                        let z = z0 + row as i32 * scale;
                        let mut line = Vec::with_capacity(w);
                        for col in 0..w {
                            let x = x0 + col as i32 * scale;
                            let p = generator.raw_params(x, z);
                            let biome = mc_worldgen::biomes::pick(&p.climate, &p.shape);
                            let h = p.shape.height;
                            line.push(Px {
                                floor: h,
                                water: (SEA_LEVEL as f32 + 1.0 - h).max(0.0),
                                top: blocks::AIR,
                                biome,
                                slice: blocks::AIR,
                                cave_air: 0,
                            });
                        }
                        let mut px = pixels.lock().unwrap();
                        px[row * w..(row + 1) * w].copy_from_slice(&line);
                    }
                });
            }
        });
    } else {
        let cx0 = x0.div_euclid(16);
        let cz0 = z0.div_euclid(16);
        // Enough chunks to cover the window even when it is not aligned.
        let chunks = (x0 - cx0 * 16 + size + 15) / 16;
        let total = (chunks * chunks) as usize;
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let pos =
                            ChunkPos::new(cx0 + (i as i32 % chunks), cz0 + (i as i32 / chunks));
                        // With scale > 1 skip chunks that contain no sample.
                        let (bx, bz) = pos.min_block();
                        let has_sample = (0..16).any(|d| (bx + d - x0).rem_euclid(scale) == 0)
                            && (0..16).any(|d| (bz + d - z0).rem_euclid(scale) == 0);
                        if !has_sample {
                            continue;
                        }
                        let chunk = generator.generate(pos);
                        let mut samples = Vec::new();
                        for lz in 0..16 {
                            for lx in 0..16 {
                                let (x, z) = (bx + lx, bz + lz);
                                if (x - x0).rem_euclid(scale) != 0
                                    || (z - z0).rem_euclid(scale) != 0
                                {
                                    continue;
                                }
                                let col = ((x - x0) / scale) as usize;
                                let row = ((z - z0) / scale) as usize;
                                if x < x0 || z < z0 || col >= w || row >= w {
                                    continue;
                                }
                                let slice =
                                    ylevel.map(|y| chunk.get(lx, y, lz)).unwrap_or(blocks::AIR);
                                let mut y = chunk.height(lx, lz);
                                let mut cave_air = 0u16;
                                for yy in mc_core::WORLD_MIN_Y..y - 6 {
                                    if chunk.get(lx, yy, lz).is_air() {
                                        cave_air += 1;
                                    }
                                }
                                let top = chunk.get(lx, y, lz);
                                let mut water = 0.0;
                                while y > mc_core::WORLD_MIN_Y {
                                    let b = chunk.get(lx, y, lz);
                                    if b == blocks::WATER || b == blocks::ICE {
                                        water += 1.0;
                                    } else if !b.def().solid && !b.def().fluid {
                                        // plants, snow layers
                                    } else if b.def().layer == mc_core::block::Layer::Cutout
                                        && water == 0.0
                                    {
                                        // leaves: keep looking for the ground for height
                                    } else {
                                        break;
                                    }
                                    y -= 1;
                                }
                                samples.push((
                                    row * w + col,
                                    Px {
                                        floor: y as f32,
                                        water,
                                        top,
                                        biome: chunk.biome(lx, lz),
                                        slice,
                                        cave_air,
                                    },
                                ));
                            }
                        }
                        let mut px = pixels.lock().unwrap();
                        for (i, p) in samples {
                            px[i] = p;
                        }
                    }
                });
            }
        });
        let n = (chunks * chunks) as f64 / (scale * scale) as f64;
        println!(
            "generated ~{:.0} chunks in {:.2?} ({:.2} ms/chunk/thread)",
            n,
            t0.elapsed(),
            t0.elapsed().as_secs_f64() * 1000.0 * threads as f64 / n.max(1.0)
        );
    }
    let px = pixels.into_inner().unwrap();

    // Hillshading from the floor heights (lit from the north-west).
    let h = |c: usize, r: usize| {
        let p = &px[r.min(w - 1) * w + c.min(w - 1)];
        if p.water > 0.0 {
            p.floor * 0.3
        } else {
            p.floor
        }
    };
    let mut shade_map = vec![1.0f32; w * w];
    for r in 0..w {
        for c in 0..w {
            let dx = h(c.saturating_sub(1), r) - h(c + 1, r);
            let dz = h(c, r.saturating_sub(1)) - h(c, r + 1);
            shade_map[r * w + c] = (1.0 + (dx + dz) * 0.09 / scale as f32).clamp(0.55, 1.45);
        }
    }

    if let Some(y) = ylevel {
        let mut img = vec![0u8; w * w * 3];
        let mut air = 0usize;
        for i in 0..w * w {
            let b = px[i].slice;
            let c = if b == blocks::AIR {
                air += 1;
                [20, 18, 24]
            } else {
                block_color(b)
            };
            img[i * 3..i * 3 + 3].copy_from_slice(&c);
        }
        println!("y={y}: {:.1}% air", air as f32 * 100.0 / (w * w) as f32);
        save_png(&format!("{out}_y{y}.png"), w as u32, w as u32, &img);
    }
    if !fast {
        // Cave projection: darker = more cave air in the column.
        let mut img = vec![0u8; w * w * 3];
        let mut sum = 0u64;
        for i in 0..w * w {
            let n = px[i].cave_air as f32;
            sum += px[i].cave_air as u64;
            let v = (235.0 - n * 4.0).clamp(0.0, 255.0) as u8;
            img[i * 3..i * 3 + 3].copy_from_slice(&[v, v, (v as f32 * 0.95) as u8]);
        }
        println!(
            "mean cave air per column: {:.1}",
            sum as f64 / (w * w) as f64
        );
        save_png(&format!("{out}_caves.png"), w as u32, w as u32, &img);
    }
    let mut biome_img = vec![0u8; w * w * 3];
    let mut surf_img = vec![0u8; w * w * 3];
    let mut counts = std::collections::BTreeMap::<&'static str, usize>::new();
    for i in 0..w * w {
        let p = px[i];
        *counts.entry(p.biome.def().name).or_default() += 1;
        let s = shade_map[i];
        let mut bc = shade(biome_color(p.biome), s);
        if p.water > 0.0 {
            let t = (p.water / 30.0).clamp(0.0, 1.0);
            bc = mix(bc, [20, 40, 120], 0.25 + 0.5 * t);
        }
        // Light altitude tint so mountains read well.
        let alt = ((p.floor - 64.0) / 160.0).clamp(0.0, 1.0);
        bc = mix(bc, [255, 255, 255], alt * 0.25);
        biome_img[i * 3..i * 3 + 3].copy_from_slice(&bc);

        let sc = if p.water > 0.0 {
            let t = (p.water / 24.0).clamp(0.0, 1.0);
            if p.top == blocks::ICE {
                [170, 200, 250]
            } else {
                mix([60, 120, 210], [15, 30, 90], t)
            }
        } else {
            shade(block_color(p.top), s)
        };
        surf_img[i * 3..i * 3 + 3].copy_from_slice(&sc);
    }
    save_png(&format!("{out}_biomes.png"), w as u32, w as u32, &biome_img);
    if !fast {
        save_png(&format!("{out}_surface.png"), w as u32, w as u32, &surf_img);
    }
    let total = (w * w) as f32;
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in v {
        println!("{:>18} {:5.1}%", name, n as f32 * 100.0 / total);
    }
    let water = px.iter().filter(|p| p.water > 0.0).count() as f32 / total;
    println!("water {:.1}%  time {:.2?}", water * 100.0, t0.elapsed());
}
