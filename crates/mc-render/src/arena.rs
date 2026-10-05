//! Pooled GPU vertex storage for chunk meshes.
//!
//! Section meshes live in a few large vertex buffers ("pages") instead of
//! one buffer per section: allocating is a free-list operation, uploads go
//! through `Queue::write_buffer` (wgpu reuses its staging memory), and draws
//! only rebind the vertex buffer when the page changes. Units are quads.

use std::collections::BTreeMap;

use crate::mesher::BYTES_PER_QUAD;

/// Allocation granularity in quads (limits fragmentation churn).
const GRAIN: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alloc {
    pub page: u32,
    /// First quad.
    pub start: u32,
    /// Reserved quads (≥ the mesh size).
    pub len: u32,
}

/// CPU-side free list (separate from the GPU buffer so it can be tested).
#[derive(Debug)]
pub struct FreeList {
    capacity: u32,
    free: BTreeMap<u32, u32>,
    used: u32,
}

impl FreeList {
    pub fn new(capacity: u32) -> Self {
        let mut free = BTreeMap::new();
        free.insert(0, capacity);
        FreeList {
            capacity,
            free,
            used: 0,
        }
    }

    /// First fit.
    pub fn alloc(&mut self, quads: u32) -> Option<(u32, u32)> {
        let need = quads.max(1).div_ceil(GRAIN) * GRAIN;
        let (&start, &len) = self.free.iter().find(|&(_, &len)| len >= need)?;
        self.free.remove(&start);
        if len > need {
            self.free.insert(start + need, len - need);
        }
        self.used += need;
        Some((start, need))
    }

    pub fn free(&mut self, start: u32, len: u32) {
        self.used -= len;
        let mut start = start;
        let mut len = len;
        // Merge with the following block.
        if let Some(&next_len) = self.free.get(&(start + len)) {
            self.free.remove(&(start + len));
            len += next_len;
        }
        // Merge with the preceding block.
        if let Some((&ps, &pl)) = self.free.range(..start).next_back() {
            if ps + pl == start {
                self.free.remove(&ps);
                start = ps;
                len += pl;
            }
        }
        self.free.insert(start, len);
    }

    pub fn used(&self) -> u32 {
        self.used
    }
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

pub struct Page {
    pub buffer: wgpu::Buffer,
    list: FreeList,
}

pub struct Arena {
    pub pages: Vec<Page>,
    page_quads: u32,
}

impl Arena {
    pub fn new(max_buffer_size: u64) -> Self {
        let page_bytes = max_buffer_size.min(64 << 20);
        Arena {
            pages: Vec::new(),
            page_quads: (page_bytes / BYTES_PER_QUAD) as u32,
        }
    }

    pub fn alloc(&mut self, device: &wgpu::Device, quads: u32) -> Alloc {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some((start, len)) = p.list.alloc(quads) {
                return Alloc {
                    page: i as u32,
                    start,
                    len,
                };
            }
        }
        let cap = self.page_quads.max(quads.div_ceil(GRAIN) * GRAIN);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk vertex page"),
            size: cap as u64 * BYTES_PER_QUAD,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut list = FreeList::new(cap);
        let (start, len) = list.alloc(quads).expect("fresh page fits");
        self.pages.push(Page { buffer, list });
        Alloc {
            page: (self.pages.len() - 1) as u32,
            start,
            len,
        }
    }

    pub fn free(&mut self, a: Alloc) {
        if let Some(p) = self.pages.get_mut(a.page as usize) {
            p.list.free(a.start, a.len);
        }
    }

    pub fn write(&self, queue: &wgpu::Queue, a: Alloc, words: &[u32]) {
        let p = &self.pages[a.page as usize];
        queue.write_buffer(
            &p.buffer,
            a.start as u64 * BYTES_PER_QUAD,
            bytemuck::cast_slice(words),
        );
    }

    pub fn used_bytes(&self) -> u64 {
        self.pages
            .iter()
            .map(|p| p.list.used() as u64 * BYTES_PER_QUAD)
            .sum()
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.pages
            .iter()
            .map(|p| p.list.capacity() as u64 * BYTES_PER_QUAD)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_coalesce() {
        let mut f = FreeList::new(1024);
        let a = f.alloc(10).unwrap();
        let b = f.alloc(100).unwrap();
        let c = f.alloc(16).unwrap();
        assert_eq!(a, (0, 16));
        assert_eq!(b.0, 16);
        f.free(b.0, b.1);
        // A smaller request reuses the hole.
        let d = f.alloc(20).unwrap();
        assert_eq!(d.0, 16);
        f.free(a.0, a.1);
        f.free(d.0, d.1);
        f.free(c.0, c.1);
        assert_eq!(f.used(), 0);
        // Everything merged back into one block.
        assert_eq!(f.alloc(1024), Some((0, 1024)));
        assert_eq!(f.alloc(1), None);
    }
}
