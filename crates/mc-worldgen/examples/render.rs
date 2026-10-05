//! Perspective voxel ray-caster for eyeballing generated terrain (dev tool).
//!
//! ```text
//! cargo run --release -p mc-worldgen --example render -- \
//!     [--seed 12345] [--x 0] [--z 0] [--y 140] [--yaw 30] [--pitch -25] \
//!     [--fov 70] [--w 1280] [--h 720] [--radius 20] [--threads 4] \
//!     [--out target/worldgen/render.png]
//! ```
//!
//! Generates all chunks within `radius` chunks of the camera, then casts one
//! ray per pixel through the voxel grid (3D DDA). Shading: per-face light,
//! a shadow ray towards the sun, see-through water and distance fog. Yaw 0
//! looks towards -Z, 90 towards +X (same convention as the game).

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use mc_core::block::{Layer, Shape};
use mc_core::{BlockId, Chunk, ChunkPos, WORLD_MAX_Y, WORLD_MIN_Y, blocks};
use mc_worldgen::WorldGenerator;

struct World {
    cx0: i32,
    cz0: i32,
    n: i32,
    chunks: Vec<Chunk>,
}

impl World {
    #[inline]
    fn get(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            return if y >= WORLD_MAX_Y {
                Some(blocks::AIR)
            } else {
                None
            };
        }
        let (cx, cz) = ((x >> 4) - self.cx0, (z >> 4) - self.cz0);
        if cx < 0 || cz < 0 || cx >= self.n || cz >= self.n {
            return None;
        }
        Some(self.chunks[(cz * self.n + cx) as usize].get(x & 15, y, z & 15))
    }
}

/// How a ray interacts with a block.
#[derive(PartialEq)]
enum Hit {
    Pass,
    Water,
    Solid,
}

fn classify(id: BlockId) -> Hit {
    if id.is_air() {
        return Hit::Pass;
    }
    if id == blocks::WATER {
        return Hit::Water;
    }
    let d = id.def();
    match d.shape {
        Shape::Cube | Shape::Inset => Hit::Solid,
        Shape::Layer(_) => Hit::Solid,
        Shape::Liquid => Hit::Solid,
        _ => {
            // Plants: thin, render as solid only sometimes (gives texture).
            if d.layer == Layer::Cutout {
                Hit::Solid
            } else {
                Hit::Pass
            }
        }
    }
}

struct RayHit {
    id: BlockId,
    pos: [i32; 3],
    /// Face normal axis (0 = x, 1 = y, 2 = z) and sign.
    axis: usize,
    sign: i32,
    t: f32,
    water_t: Option<f32>,
}

fn cast(w: &World, o: [f32; 3], d: [f32; 3], max_t: f32, stop_at_water: bool) -> Option<RayHit> {
    let mut p = [
        o[0].floor() as i32,
        o[1].floor() as i32,
        o[2].floor() as i32,
    ];
    let step = [
        d[0].signum() as i32,
        d[1].signum() as i32,
        d[2].signum() as i32,
    ];
    let inv = [
        1.0 / d[0].abs().max(1e-9),
        1.0 / d[1].abs().max(1e-9),
        1.0 / d[2].abs().max(1e-9),
    ];
    let mut tmax = [0f32; 3];
    for i in 0..3 {
        let edge = if d[i] > 0.0 {
            (p[i] + 1) as f32 - o[i]
        } else {
            o[i] - p[i] as f32
        };
        tmax[i] = edge * inv[i];
    }
    let mut t = 0.0f32;
    let mut axis = 1usize;
    let mut water_t: Option<f32> = None;
    while t < max_t {
        let id = w.get(p[0], p[1], p[2])?;
        match classify(id) {
            Hit::Solid if t > 0.0 || !stop_at_water => {
                if t > 0.0 {
                    return Some(RayHit {
                        id,
                        pos: p,
                        axis,
                        sign: -step[axis],
                        t,
                        water_t,
                    });
                }
            }
            Hit::Water => {
                if water_t.is_none() {
                    water_t = Some(t);
                    if stop_at_water {
                        return Some(RayHit {
                            id,
                            pos: p,
                            axis,
                            sign: -step[axis],
                            t,
                            water_t,
                        });
                    }
                }
            }
            _ => {}
        }
        axis = if tmax[0] < tmax[1] {
            if tmax[0] < tmax[2] { 0 } else { 2 }
        } else if tmax[1] < tmax[2] {
            1
        } else {
            2
        };
        t = tmax[axis];
        tmax[axis] += inv[axis];
        p[axis] += step[axis];
    }
    None
}

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let cam = [a.get("x", 0.0f32), a.get("y", 140.0f32), a.get("z", 0.0f32)];
    let yaw: f32 = a.get::<f32>("yaw", 30.0).to_radians();
    let pitch: f32 = a.get::<f32>("pitch", -25.0).to_radians();
    let fov: f32 = a.get::<f32>("fov", 70.0).to_radians();
    let (width, height): (u32, u32) = (a.get("w", 1280), a.get("h", 720));
    let radius: i32 = a.get("radius", 20);
    let threads: usize = a.get("threads", 4);
    let out: String = a.get("out", "target/worldgen/render.png".to_string());

    let generator = WorldGenerator::new(seed);
    let center = ChunkPos::from_block(cam[0] as i32, cam[2] as i32);
    let n = radius * 2 + 1;
    let (cx0, cz0) = (center.x - radius, center.z - radius);
    let t0 = std::time::Instant::now();
    let slots: Vec<Mutex<Option<Chunk>>> = (0..n * n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= (n * n) as usize {
                        break;
                    }
                    let p = ChunkPos::new(cx0 + i as i32 % n, cz0 + i as i32 / n);
                    *slots[i].lock().unwrap() = Some(generator.generate(p));
                }
            });
        }
    });
    let world = World {
        cx0,
        cz0,
        n,
        chunks: slots
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .collect(),
    };
    println!("generated {} chunks in {:.2?}", n * n, t0.elapsed());

    // Camera basis. Yaw 0 looks to -Z, +90° to +X.
    let fwd = [
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    ];
    let right = [yaw.cos(), 0.0, yaw.sin()];
    let up = [
        right[1] * fwd[2] - right[2] * fwd[1],
        right[2] * fwd[0] - right[0] * fwd[2],
        right[0] * fwd[1] - right[1] * fwd[0],
    ];
    let sun = {
        let v = [0.45f32, 0.8, 0.3];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let max_t = (radius * 16) as f32;
    let sky = [150u8, 190, 245];
    let tan = (fov * 0.5).tan();
    let aspect = width as f32 / height as f32;

    let rows = AtomicUsize::new(0);
    let img = Mutex::new(vec![0u8; (width * height * 3) as usize]);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let row = rows.fetch_add(1, Ordering::Relaxed) as u32;
                    if row >= height {
                        break;
                    }
                    let mut line = vec![0u8; (width * 3) as usize];
                    for col in 0..width {
                        let u = ((col as f32 + 0.5) / width as f32 * 2.0 - 1.0) * tan * aspect;
                        let v = (1.0 - (row as f32 + 0.5) / height as f32 * 2.0) * tan;
                        let mut d = [0f32; 3];
                        for i in 0..3 {
                            d[i] = fwd[i] + right[i] * u + up[i] * v;
                        }
                        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        for x in d.iter_mut() {
                            *x /= l;
                        }
                        let sky_c = mix(sky, [215, 230, 255], (1.0 - d[1].max(0.0)).powi(3) * 0.6);
                        let c = match cast(&world, cam, d, max_t, false) {
                            None => sky_c,
                            Some(h) => {
                                let mut base = block_color(h.id);
                                // Per-face light.
                                let mut n = [0f32; 3];
                                n[h.axis] = h.sign as f32;
                                let lambert =
                                    (n[0] * sun[0] + n[1] * sun[1] + n[2] * sun[2]).max(0.0);
                                let face = match (h.axis, h.sign) {
                                    (1, 1) => 1.0,
                                    (1, _) => 0.5,
                                    (0, _) => 0.8,
                                    _ => 0.7,
                                };
                                // Shadow ray from the hit face.
                                let hp = [
                                    h.pos[0] as f32 + 0.5 + n[0] * 0.51,
                                    h.pos[1] as f32 + 0.5 + n[1] * 0.51,
                                    h.pos[2] as f32 + 0.5 + n[2] * 0.51,
                                ];
                                let lit = lambert > 0.0
                                    && cast(&world, hp, sun, 160.0, true)
                                        .is_none_or(|s| s.id == blocks::WATER);
                                let light = 0.45 * face + if lit { 0.6 * lambert } else { 0.0 };
                                // Small per-block brightness jitter for texture.
                                let j = ((h.pos[0].wrapping_mul(73856093)
                                    ^ h.pos[1].wrapping_mul(19349663)
                                    ^ h.pos[2].wrapping_mul(83492791))
                                    & 15) as f32
                                    / 15.0;
                                base = shade(base, light * (0.92 + 0.12 * j));
                                if let Some(wt) = h.water_t {
                                    let depth = (h.t - wt).max(0.0);
                                    let k = 1.0 - (-depth * 0.18).exp();
                                    base = mix(base, [40, 80, 170], 0.35 + 0.6 * k);
                                }
                                let fog = ((h.t / max_t).powi(3) * 1.2).min(1.0);
                                mix(base, sky_c, fog)
                            }
                        };
                        line[(col * 3) as usize..(col * 3 + 3) as usize].copy_from_slice(&c);
                    }
                    let mut im = img.lock().unwrap();
                    let o = (row * width * 3) as usize;
                    im[o..o + line.len()].copy_from_slice(&line);
                }
            });
        }
    });
    save_png(&out, width, height, &img.into_inner().unwrap());
    println!("total {:.2?}", t0.elapsed());
}
