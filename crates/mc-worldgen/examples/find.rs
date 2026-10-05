//! Print the nearest location of every biome (dev tool).
//!
//! ```text
//! cargo run --release -p mc-worldgen --example find -- [--seed 12345] [--x 0] [--z 0] [--range 6000] [--step 32]
//! ```

mod common;

use common::*;
use mc_core::biome::BIOME_DEFS;
use mc_worldgen::WorldGenerator;

fn main() {
    let a = Args::new();
    let seed: u64 = a.get("seed", 12345);
    let (cx, cz): (i32, i32) = (a.get("x", 0), a.get("z", 0));
    let range: i32 = a.get("range", 6000);
    let step: i32 = a.get("step", 32);
    let g = WorldGenerator::new(seed);
    if a.flag("spawn") {
        let t = std::time::Instant::now();
        let s = g.spawn_point();
        println!(
            "spawn {} {} {} biome {} ({:.1?})",
            s.x,
            s.y,
            s.z,
            g.biome_at(s.x, s.z).def().name,
            t.elapsed()
        );
        return;
    }
    if a.flag("probe") {
        let p = g.column_params(cx, cz);
        let info = g.probe(cx, cz);
        let raw = g.raw_params(cx, cz);
        println!(
            "raw: h {:.1} biome {}",
            raw.shape.height,
            mc_worldgen::biomes::pick(&raw.climate, &raw.shape)
                .def()
                .name
        );
        println!(
            "{:?}\n{:?}\nbiome {} top {} slope {:.2}",
            p.climate,
            p.shape,
            info.biome.def().name,
            info.top,
            info.slope
        );
        return;
    }
    let mut best: Vec<Option<(i64, i32, i32)>> = vec![None; BIOME_DEFS.len()];
    let mut counts = vec![0usize; BIOME_DEFS.len()];
    let mut z = cz - range;
    while z <= cz + range {
        let mut x = cx - range;
        while x <= cx + range {
            let b = g.biome_at(x, z).0 as usize;
            counts[b] += 1;
            let d = ((x - cx) as i64).pow(2) + ((z - cz) as i64).pow(2);
            if best[b].is_none_or(|(bd, _, _)| d < bd) {
                best[b] = Some((d, x, z));
            }
            x += step;
        }
        z += step;
    }
    let total: usize = counts.iter().sum();
    for (i, def) in BIOME_DEFS.iter().enumerate() {
        match best[i] {
            Some((d, x, z)) => println!(
                "{:>18}: nearest ({x:6}, {z:6})  dist {:6.0}  share {:5.2}%",
                def.name,
                (d as f64).sqrt(),
                counts[i] as f64 * 100.0 / total as f64
            ),
            None => println!("{:>18}: not found", def.name),
        }
    }
}
