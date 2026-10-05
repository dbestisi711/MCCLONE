//! Small pixel helpers on [`Rgba8Image`]: tile resizing, tinting, alpha
//! handling and mipmap generation.
//!
//! All placeholder art generated here is deliberately generic (solid colours
//! and checkerboards).

use mc_core::Rgba8Image;

/// Magenta/black checkerboard used for missing textures.
pub fn magenta_checker(size: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(size, size);
    let half = (size / 2).max(1);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / half) + (y / half)) % 2 == 0;
            img.put(
                x,
                y,
                if on {
                    [248, 0, 248, 255]
                } else {
                    [0, 0, 0, 255]
                },
            );
        }
    }
    img
}

/// Nearest-neighbour resize.
pub fn resize_nearest(src: &Rgba8Image, w: u32, h: u32) -> Rgba8Image {
    let mut out = Rgba8Image::new(w, h);
    if src.width == 0 || src.height == 0 {
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            out.put(x, y, src.get(x * src.width / w, y * src.height / h));
        }
    }
    out
}

/// Resize an image to `size`×`size`: unchanged if it already fits, an
/// alpha-weighted box filter for integer downscales (HD packs), and nearest
/// neighbour otherwise (pixel art upscales stay crisp).
pub fn resize_tile(src: &Rgba8Image, size: u32) -> Rgba8Image {
    if src.width == size && src.height == size {
        return src.clone();
    }
    if src.width > size
        && src.height > size
        && src.width % size == 0
        && src.height % size == 0
        && src.width / size == src.height / size
    {
        let f = src.width / size;
        let mut out = Rgba8Image::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let mut px = Vec::with_capacity((f * f) as usize);
                for yy in 0..f {
                    for xx in 0..f {
                        px.push(src.get(x * f + xx, y * f + yy));
                    }
                }
                out.put(x, y, average_alpha_weighted(&px));
            }
        }
        return out;
    }
    resize_nearest(src, size, size)
}

/// Average of pixels with colour weighted by alpha (so the colour of
/// transparent texels does not bleed in) and plain-averaged alpha.
fn average_alpha_weighted(px: &[[u8; 4]]) -> [u8; 4] {
    let n = px.len().max(1) as u32;
    let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
    let (mut pr, mut pg, mut pb) = (0u32, 0u32, 0u32);
    for p in px {
        let w = p[3] as u32;
        r += p[0] as u32 * w;
        g += p[1] as u32 * w;
        b += p[2] as u32 * w;
        a += w;
        pr += p[0] as u32;
        pg += p[1] as u32;
        pb += p[2] as u32;
    }
    if a == 0 {
        return [(pr / n) as u8, (pg / n) as u8, (pb / n) as u8, 0];
    }
    [
        ((r + a / 2) / a) as u8,
        ((g + a / 2) / a) as u8,
        ((b + a / 2) / a) as u8,
        ((a + n / 2) / n) as u8,
    ]
}

/// Halve an image (each output pixel averages a 2×2 block; odd sizes clamp).
fn downsample2(src: &Rgba8Image) -> Rgba8Image {
    let w = (src.width / 2).max(1);
    let h = (src.height / 2).max(1);
    let mut out = Rgba8Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let x0 = (x * 2).min(src.width - 1);
            let y0 = (y * 2).min(src.height - 1);
            let x1 = (x * 2 + 1).min(src.width - 1);
            let y1 = (y * 2 + 1).min(src.height - 1);
            let px = [
                src.get(x0, y0),
                src.get(x1, y0),
                src.get(x0, y1),
                src.get(x1, y1),
            ];
            out.put(x, y, average_alpha_weighted(&px));
        }
    }
    out
}

/// Fraction of pixels whose alpha (times `scale`) passes a 50% alpha test.
fn coverage(img: &Rgba8Image, scale: f32) -> f32 {
    let n = (img.width * img.height).max(1) as f32;
    let pass = img
        .data
        .chunks_exact(4)
        .filter(|p| p[3] as f32 * scale >= 127.5)
        .count();
    pass as f32 / n
}

/// Full mip chain for a tile, **including a copy of level 0** at index 0,
/// down to 1×1 (16×16 → 5 levels: 16, 8, 4, 2, 1).
///
/// Colour is averaged weighted by alpha so transparent texels never darken
/// edges. For cutout textures (some texels below and some above 50% alpha)
/// each level's alpha is rescaled so the fraction of texels that pass a 50%
/// alpha test matches level 0; leaves and plants therefore keep their
/// density in the distance instead of fading away. Opaque and uniformly
/// translucent tiles are plain averages.
pub fn generate_mips(tile: &Rgba8Image) -> Vec<Rgba8Image> {
    let mut levels = vec![tile.clone()];
    let has_low = tile.data.chunks_exact(4).any(|p| p[3] < 128);
    let has_high = tile.data.chunks_exact(4).any(|p| p[3] >= 128);
    let cutout = has_low && has_high;
    let target = coverage(tile, 1.0);
    // Downsample from the unscaled chain so scaling doesn't compound.
    let mut prev = tile.clone();
    while prev.width > 1 || prev.height > 1 {
        let next = downsample2(&prev);
        let mut level = next.clone();
        if cutout {
            // Binary search an alpha scale that preserves alpha-test coverage.
            let (mut lo, mut hi) = (0.0f32, 64.0f32);
            for _ in 0..20 {
                let mid = 0.5 * (lo + hi);
                if coverage(&next, mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let s = hi;
            for p in level.data.chunks_exact_mut(4) {
                p[3] = (p[3] as f32 * s).round().min(255.0) as u8;
            }
        }
        levels.push(level);
        prev = next;
    }
    levels
}

/// Fill the RGB of fully transparent pixels with the average of nearby
/// non-transparent pixels (alpha stays 0). Prevents dark fringes when a
/// cutout texture is bilinearly filtered or mipmapped.
pub fn dilate_transparent(img: &mut Rgba8Image) {
    let (w, h) = (img.width as i32, img.height as i32);
    if !img.data.chunks_exact(4).any(|p| p[3] == 0) || !img.data.chunks_exact(4).any(|p| p[3] > 0) {
        return;
    }
    let mut known: Vec<bool> = img.data.chunks_exact(4).map(|p| p[3] > 0).collect();
    // A few passes grow the known region outwards; remaining pixels get the global mean.
    for _ in 0..4 {
        let snapshot = img.clone();
        let prev_known = known.clone();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if prev_known[i] {
                    continue;
                }
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for (dx, dy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    if prev_known[(ny * w + nx) as usize] {
                        let p = snapshot.get(nx as u32, ny as u32);
                        r += p[0] as u32;
                        g += p[1] as u32;
                        b += p[2] as u32;
                        n += 1;
                    }
                }
                if n > 0 {
                    img.put(
                        x as u32,
                        y as u32,
                        [(r / n) as u8, (g / n) as u8, (b / n) as u8, 0],
                    );
                    known[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for (p, k) in img.data.chunks_exact(4).zip(&known) {
        if *k {
            r += p[0] as u64;
            g += p[1] as u64;
            b += p[2] as u64;
            n += 1;
        }
    }
    let mean = if n > 0 {
        [(r / n) as u8, (g / n) as u8, (b / n) as u8]
    } else {
        [0, 0, 0]
    };
    for (p, k) in img.data.chunks_exact_mut(4).zip(&known) {
        if !*k {
            p[..3].copy_from_slice(&mean);
        }
    }
}

/// Multiply the RGB channels by a 0xRRGGBB colour.
pub fn tint(img: &mut Rgba8Image, rgb: u32) {
    let t = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF];
    for p in img.data.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = ((p[c] as u32 * t[c] + 127) / 255) as u8;
        }
    }
}

/// Force every pixel fully opaque.
pub fn set_opaque(img: &mut Rgba8Image) {
    for p in img.data.chunks_exact_mut(4) {
        p[3] = 255;
    }
}

/// Composite `src` over `dst` (same size), standard "over" alpha blending.
pub fn alpha_over(dst: &mut Rgba8Image, src: &Rgba8Image) {
    for (d, s) in dst.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        let sa = s[3] as u32;
        if sa == 0 {
            continue;
        }
        let da = d[3] as u32;
        let out_a = sa + da * (255 - sa) / 255;
        if out_a == 0 {
            continue;
        }
        for c in 0..3 {
            let v = (s[c] as u32 * sa + d[c] as u32 * da * (255 - sa) / 255) / out_a;
            d[c] = v.min(255) as u8;
        }
        d[3] = out_a.min(255) as u8;
    }
}

/// True if most visible pixels are grey, i.e. the texture is meant to be
/// multiplied by a biome tint (grass top, leaves, water...). A few coloured
/// specks (jungle leaves) are allowed; pre-coloured art (sugar cane, dirt)
/// is not grey.
pub fn is_greyscale(img: &Rgba8Image) -> bool {
    let mut visible = 0u32;
    let mut grey = 0u32;
    for p in img.data.chunks_exact(4) {
        if p[3] == 0 {
            continue;
        }
        visible += 1;
        let max = p[0].max(p[1]).max(p[2]);
        let min = p[0].min(p[1]).min(p[2]);
        if max - min <= 12 {
            grey += 1;
        }
    }
    visible > 0 && grey * 2 >= visible
}

/// Number of square frames in a vertical strip (1 for ordinary textures).
pub fn strip_frames(img: &Rgba8Image) -> u32 {
    if img.width == 0 || img.height <= img.width || img.height % img.width != 0 {
        1
    } else {
        img.height / img.width
    }
}

/// Upscale by an integer factor with nearest neighbour.
pub fn scale_integer(src: &Rgba8Image, factor: u32) -> Rgba8Image {
    resize_nearest(src, src.width * factor, src.height * factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker_cutout() -> Rgba8Image {
        // 16×16 with ~60% of texels opaque in a fine pattern (like leaves).
        let mut img = Rgba8Image::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                let opaque = (x * 7 + y * 13) % 5 < 3;
                img.put(
                    x,
                    y,
                    if opaque {
                        [40, 160, 40, 255]
                    } else {
                        [0, 0, 0, 0]
                    },
                );
            }
        }
        img
    }

    #[test]
    fn mip_chain_sizes() {
        let mips = generate_mips(&Rgba8Image::filled(16, 16, [10, 20, 30, 255]));
        let sizes: Vec<u32> = mips.iter().map(|m| m.width).collect();
        assert_eq!(sizes, vec![16, 8, 4, 2, 1]);
        assert_eq!(mips[4].get(0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn cutout_mips_keep_coverage() {
        // A blob covering ~60% of the tile keeps roughly that coverage.
        let mut blob = Rgba8Image::new(16, 16);
        for y in 0..16i32 {
            for x in 0..16i32 {
                let inside = (x - 8) * (x - 8) + (y - 8) * (y - 8) < 50;
                blob.put(
                    x as u32,
                    y as u32,
                    if inside {
                        [40, 160, 40, 255]
                    } else {
                        [0, 0, 0, 0]
                    },
                );
            }
        }
        let c0 = coverage(&blob, 1.0);
        let mips = generate_mips(&blob);
        for m in &mips[1..3] {
            let c = coverage(m, 1.0);
            assert!((c - c0).abs() < 0.2, "coverage {c} vs {c0}");
        }
        // A fine pattern averages to uniform alpha; plain averaging would drop
        // it below the alpha-test threshold, coverage scaling keeps it.
        let tile = checker_cutout();
        let mips = generate_mips(&tile);
        assert!(mips[2].get(1, 1)[3] >= 128);
        assert!(mips.last().unwrap().get(0, 0)[3] >= 128);
        // Colour must not be darkened by transparent black texels.
        let p = mips[2].get(1, 1);
        assert!(p[1] > 120, "{p:?}");
    }

    #[test]
    fn resize_box_filter() {
        let img = Rgba8Image::filled(32, 32, [100, 50, 25, 255]);
        let t = resize_tile(&img, 16);
        assert_eq!((t.width, t.height), (16, 16));
        assert_eq!(t.get(3, 3), [100, 50, 25, 255]);
    }

    #[test]
    fn dilate_fills_transparent_rgb() {
        let mut img = checker_cutout();
        dilate_transparent(&mut img);
        for p in img.data.chunks_exact(4) {
            if p[3] == 0 {
                assert!(p[1] > 100);
            }
        }
    }
}
