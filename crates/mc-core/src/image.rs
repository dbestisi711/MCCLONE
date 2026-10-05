//! Minimal RGBA8 image type shared between crates (so they don't all need the
//! `image` crate's types in their public APIs).

#[derive(Clone, Debug, Default)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA, `width * height * 4` bytes.
    pub data: Vec<u8>,
}

impl Rgba8Image {
    pub fn new(width: u32, height: u32) -> Self {
        Rgba8Image {
            width,
            height,
            data: vec![0; (width * height * 4) as usize],
        }
    }
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut img = Self::new(width, height);
        for px in img.data.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        img
    }
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
    #[inline]
    pub fn put(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }
    /// Copy `src` into `self` at (dx, dy), clipping at the edges.
    pub fn blit(&mut self, src: &Rgba8Image, dx: u32, dy: u32) {
        for y in 0..src.height {
            if dy + y >= self.height {
                break;
            }
            for x in 0..src.width {
                if dx + x >= self.width {
                    break;
                }
                self.put(dx + x, dy + y, src.get(x, y));
            }
        }
    }
    /// Sub-rectangle copy.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Rgba8Image {
        let mut out = Rgba8Image::new(w, h);
        for yy in 0..h.min(self.height.saturating_sub(y)) {
            for xx in 0..w.min(self.width.saturating_sub(x)) {
                out.put(xx, yy, self.get(x + xx, y + yy));
            }
        }
        out
    }
}
