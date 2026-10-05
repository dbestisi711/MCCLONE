//! Data that gameplay crates hand to the renderer each frame.
//!
//! These are deliberately renderer-agnostic: `mc-ui` builds a [`UiDrawList`],
//! `mc-entity` builds [`EntityRenderInstance`]s, `mc-game` fills a
//! [`Camera`] and [`SkyState`], and `mc-render` consumes all of it.

use std::sync::Arc;

use glam::{Mat4, Vec2, Vec3};

use crate::Rgba8Image;

/// Names a texture the renderer can bind.
///
/// - A pack-relative path *without extension*, e.g. `"textures/gui/gui"` or
///   `"textures/entity/pig/pig"`; the renderer loads it via `mc-assets`
///   (which tries `.png` then `.tga`).
/// - `"@white"`: a 1×1 white texture (solid-colour quads).
/// - `"@blocks"`: the block texture array/atlas (UVs from `mc-assets` block textures).
/// - Any other `"@name"`: a dynamic texture supplied through
///   [`UiDrawList::upload`] (e.g. `"@font"`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TextureKey(pub Arc<str>);

impl TextureKey {
    pub fn new(s: &str) -> Self {
        TextureKey(Arc::from(s))
    }
    pub fn white() -> Self {
        Self::new("@white")
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TextureKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// A textured, coloured quad in screen space (physical pixels, origin top-left).
/// Corners are in order top-left, top-right, bottom-right, bottom-left, so
/// arbitrary parallelograms (isometric block icons) are possible.
#[derive(Clone, Debug)]
pub struct UiQuad {
    pub texture: TextureKey,
    pub pos: [Vec2; 4],
    /// Normalised UVs (0..1) per corner.
    pub uv: [Vec2; 4],
    /// Texture array layer, only used with `"@blocks"` when it is an array texture.
    pub layer: u32,
    /// Linear RGBA multiplier.
    pub color: [f32; 4],
}

impl UiQuad {
    /// Axis-aligned rectangle helper. `uv` is (u0, v0, u1, v1).
    pub fn rect(
        texture: TextureKey,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        color: [f32; 4],
    ) -> Self {
        UiQuad {
            texture,
            pos: [
                Vec2::new(x, y),
                Vec2::new(x + w, y),
                Vec2::new(x + w, y + h),
                Vec2::new(x, y + h),
            ],
            uv: [
                Vec2::new(uv[0], uv[1]),
                Vec2::new(uv[2], uv[1]),
                Vec2::new(uv[2], uv[3]),
                Vec2::new(uv[0], uv[3]),
            ],
            layer: 0,
            color,
        }
    }
    pub fn solid(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self::rect(TextureKey::white(), x, y, w, h, [0.0, 0.0, 1.0, 1.0], color)
    }
}

/// Everything the UI wants drawn this frame, in painter's order.
#[derive(Clone, Debug, Default)]
pub struct UiDrawList {
    pub quads: Vec<UiQuad>,
    /// Dynamic textures to (re)upload before drawing, keyed by `"@name"`.
    /// Only send when contents change (e.g. font atlas on first frame).
    pub upload: Vec<(TextureKey, Arc<Rgba8Image>)>,
    /// Draw the world darkened behind (when a screen like the inventory is open).
    pub dim_world: bool,
}

impl UiDrawList {
    pub fn push(&mut self, q: UiQuad) {
        self.quads.push(q);
    }
    pub fn clear(&mut self) {
        self.quads.clear();
        self.upload.clear();
        self.dim_world = false;
    }
}

/// Pose override for one bone of an entity model.
#[derive(Clone, Debug, PartialEq)]
pub struct BonePose {
    /// Bone name as in the `.geo.json` (e.g. `"head"`, `"leg0"`).
    pub bone: Arc<str>,
    /// Extra rotation in degrees (x, y, z), applied around the bone pivot.
    pub rotation: Vec3,
    /// Extra translation in model pixels (1/16 block).
    pub offset: Vec3,
    /// Uniform scale (1.0 = unchanged). 0 hides the bone.
    pub scale: f32,
}

impl BonePose {
    pub fn rot(bone: &str, rotation: Vec3) -> Self {
        BonePose {
            bone: Arc::from(bone),
            rotation,
            offset: Vec3::ZERO,
            scale: 1.0,
        }
    }
    pub fn hidden(bone: &str) -> Self {
        BonePose {
            bone: Arc::from(bone),
            rotation: Vec3::ZERO,
            offset: Vec3::ZERO,
            scale: 0.0,
        }
    }
}

/// One entity to draw this frame.
#[derive(Clone, Debug)]
pub struct EntityRenderInstance {
    /// Geometry identifier, e.g. `"geometry.pig"` (as in the pack's `.geo.json`).
    pub model: Arc<str>,
    pub texture: TextureKey,
    /// Model space → world space (position, yaw, scale). Model units are blocks
    /// (the asset crate converts geo pixels to blocks).
    pub transform: Mat4,
    pub poses: Vec<BonePose>,
    /// Multiplied with the texture colour (sheep wool colour, etc.).
    pub tint: [f32; 4],
    /// 0..1 red flash when hurt.
    pub hurt: f32,
    /// Packed light at the entity position (`sky << 4 | block`).
    pub light: u8,
}

/// Simple coloured line box (block selection outline, debug hitboxes).
#[derive(Clone, Copy, Debug)]
pub struct DebugBox {
    pub min: Vec3,
    pub max: Vec3,
    pub color: [f32; 4],
}

/// A block item / falling block rendered as a small cube in the world.
#[derive(Clone, Copy, Debug)]
pub struct BlockModelInstance {
    pub block: crate::BlockId,
    pub transform: Mat4,
    pub light: u8,
}

/// A flat sprite in the world (dropped non-block items, particles).
#[derive(Clone, Debug)]
pub struct SpriteInstance {
    pub texture: TextureKey,
    pub uv: [f32; 4],
    pub center: Vec3,
    pub size: Vec2,
    pub color: [f32; 4],
    /// Always face the camera (true) or use `transform` orientation (false).
    pub billboard: bool,
    pub light: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: Vec3,
    /// Radians, 0 = looking toward -Z, positive turns toward +X... see `forward()`.
    pub yaw: f32,
    /// Radians, positive looks up.
    pub pitch: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: Vec3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            near: 0.05,
            far: 1000.0,
        }
    }
}

impl Camera {
    /// Unit look direction. yaw = 0 looks toward -Z; yaw = +90° looks toward +X.
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, -cy * cp)
    }
    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Y)
    }
    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far)
    }
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }
}

/// Global lighting/atmosphere state for the frame.
#[derive(Clone, Copy, Debug)]
pub struct SkyState {
    /// 0..DAY_LENGTH_TICKS.
    pub time_of_day: u64,
    /// 0..1 sun brightness.
    pub daylight: f32,
    /// Biome sky/fog colours at the camera, 0xRRGGBB.
    pub sky_color: u32,
    pub fog_color: u32,
    /// Camera is underwater / in lava.
    pub underwater: bool,
    pub in_lava: bool,
    /// 0..1 rain strength.
    pub rain: f32,
}

impl Default for SkyState {
    fn default() -> Self {
        SkyState {
            time_of_day: 6000,
            daylight: 1.0,
            sky_color: 0x78A7FF,
            fog_color: 0xC0D8FF,
            underwater: false,
            in_lava: false,
            rain: 0.0,
        }
    }
}

/// Everything the renderer needs for one frame besides the world itself.
#[derive(Clone, Debug, Default)]
pub struct FrameData {
    pub camera: Camera,
    pub sky: SkyState,
    pub entities: Vec<EntityRenderInstance>,
    pub block_models: Vec<BlockModelInstance>,
    pub sprites: Vec<SpriteInstance>,
    pub boxes: Vec<DebugBox>,
    /// Block currently being broken and progress 0..1 (crack overlay).
    pub breaking: Option<(glam::IVec3, f32)>,
    /// First-person held item (None = empty hand).
    pub held_item: Option<crate::ItemId>,
    /// 0..1 arm swing animation progress.
    pub swing: f32,
    pub third_person: bool,
    pub ui: UiDrawList,
}
