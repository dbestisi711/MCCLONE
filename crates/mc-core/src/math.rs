//! Small geometry helpers.

use glam::{IVec3, Vec3};

/// Axis-aligned bounding box in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Aabb { min, max }
    }
    /// Box of `width` × `height` with its bottom-centre at `feet`.
    pub fn from_feet(feet: Vec3, width: f32, height: f32) -> Self {
        let h = width * 0.5;
        Aabb {
            min: Vec3::new(feet.x - h, feet.y, feet.z - h),
            max: Vec3::new(feet.x + h, feet.y + height, feet.z + h),
        }
    }
    pub fn block(p: IVec3) -> Self {
        let min = p.as_vec3();
        Aabb {
            min,
            max: min + Vec3::ONE,
        }
    }
    pub fn translate(&self, d: Vec3) -> Self {
        Aabb {
            min: self.min + d,
            max: self.max + d,
        }
    }
    pub fn expand(&self, d: Vec3) -> Self {
        Aabb {
            min: self.min.min(self.min + d),
            max: self.max.max(self.max + d),
        }
    }
    pub fn inflate(&self, r: f32) -> Self {
        Aabb {
            min: self.min - Vec3::splat(r),
            max: self.max + Vec3::splat(r),
        }
    }
    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min.x < o.max.x
            && self.max.x > o.min.x
            && self.min.y < o.max.y
            && self.max.y > o.min.y
            && self.min.z < o.max.z
            && self.max.z > o.min.z
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
    /// Integer block range overlapped by this box (inclusive).
    pub fn block_range(&self) -> (IVec3, IVec3) {
        (
            self.min.floor().as_ivec3(),
            (self.max - Vec3::splat(1e-4)).floor().as_ivec3(),
        )
    }
}
