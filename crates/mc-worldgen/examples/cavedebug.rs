//! Point-sampled cave term on a vertical plane (dev tool): shows the cave
//! noise before grid interpolation, to tune cave shapes.
//!
//! ```text
//! cargo run --release -p mc-worldgen --example cavedebug -- [--seed 12345] [--z 0] [--x 0] [--len 300] [--px 3]
//! ```

mod common;

use common::*;
use mc_worldgen::climate::Shape;
use mc_worldgen::terrain::TerrainNoise;

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let z: i32 = a.get("z", 0);
    let center: i32 = a.get("x", 0);
    let len: i32 = a.get("len", 300);
    let px: i32 = a.get("px", 3);
    let (ymin, ymax) = (-64, 100);
    let t = TerrainNoise::new(seed);
    let shape = Shape {
        height: 200.0,
        factor: 4.0,
        river: 0.0,
    };
    let w = (len * px) as u32;
    let h = ((ymax - ymin) * px) as u32;
    let mut img = vec![0u8; (w * h * 3) as usize];
    for i in 0..len {
        let x = center - len / 2 + i;
        for y in ymin..ymax {
            let v = t.caves(x, y, z, &shape);
            let c = if v < 0.0 {
                [20, 20, 25]
            } else {
                [128, 128, 128]
            };
            for dy in 0..px {
                for dx in 0..px {
                    let o = ((((ymax - 1 - y) * px + dy) as u32 * w + (i * px + dx) as u32) * 3)
                        as usize;
                    img[o..o + 3].copy_from_slice(&c);
                }
            }
        }
    }
    save_png(
        &a.get("out", "target/worldgen/cavedebug.png".to_string()),
        w,
        h,
        &img,
    );
}
