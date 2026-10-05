//! Vertical cross-section renderer (dev tool).
//!
//! ```text
//! cargo run --release -p mc-worldgen --example section -- \
//!     [--seed 12345] [--z 0] [--x 0] [--len 512] [--ymin -64] [--ymax 320] \
//!     [--px 2] [--along z] [--out target/worldgen/section.png]
//! ```
//!
//! Renders the blocks of the plane `z = const` (or `x = const` with
//! `--along z`), centred on `--x`, `len` blocks wide. Cave air is drawn dark,
//! sky air light, so caves, aquifers, lava, ores and the deepslate transition
//! are easy to see.

mod common;

use common::*;
use mc_core::{ChunkPos, blocks};
use mc_worldgen::WorldGenerator;

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let fixed: i32 = a.get("z", 0);
    let center: i32 = a.get("x", 0);
    let len: i32 = a.get("len", 512);
    let ymin: i32 = a.get("ymin", -64);
    let ymax: i32 = a.get("ymax", 320);
    let px: i32 = a.get("px", 2).max(1);
    let along_z = a.get("along", "x".to_string()) == "z";
    let out: String = a.get("out", "target/worldgen/section.png".to_string());

    let generator = WorldGenerator::new(seed);
    let start = center - len / 2;
    let w = (len * px) as u32;
    let h = ((ymax - ymin) * px) as u32;
    let mut img = vec![0u8; (w * h * 3) as usize];
    let t0 = std::time::Instant::now();
    let mut chunks = 0;
    let mut cache: Option<(ChunkPos, mc_core::Chunk)> = None;
    for i in 0..len {
        let (x, z) = if along_z {
            (fixed, start + i)
        } else {
            (start + i, fixed)
        };
        let cp = ChunkPos::from_block(x, z);
        if cache.as_ref().map(|c| c.0) != Some(cp) {
            cache = Some((cp, generator.generate(cp)));
            chunks += 1;
        }
        let chunk = &cache.as_ref().unwrap().1;
        let (lx, lz) = (x & 15, z & 15);
        let top = chunk.height(lx, lz);
        for y in ymin..ymax {
            let id = chunk.get(lx, y, lz);
            let c = if id == blocks::AIR {
                if y > top {
                    [170, 200, 240]
                } else {
                    [25, 22, 28]
                }
            } else {
                let mut c = block_color(id);
                if id == blocks::STONE || id == blocks::DEEPSLATE {
                    // Subtle depth cue.
                    c = shade(c, 0.85 + 0.15 * ((y - ymin) as f32 / (ymax - ymin) as f32));
                }
                c
            };
            for dy in 0..px {
                for dx in 0..px {
                    let ix = (i * px + dx) as u32;
                    let iy = ((ymax - 1 - y) * px + dy) as u32;
                    let o = ((iy * w + ix) * 3) as usize;
                    img[o..o + 3].copy_from_slice(&c);
                }
            }
        }
    }
    println!("{chunks} chunks in {:.2?}", t0.elapsed());
    save_png(&out, w, h, &img);
}
