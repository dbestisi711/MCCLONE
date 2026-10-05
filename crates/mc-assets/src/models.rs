//! Entity models parsed from the pack's Bedrock `.geo.json` files.
//!
//! OWNER: models & textures agent. Stub: the API shape is fixed, the parser is
//! not implemented yet.

use std::sync::Arc;

use glam::{Vec2, Vec3};
use mc_core::render_types::BonePose;
use rustc_hash::FxHashMap as HashMap;

use crate::Pack;

/// Vertex produced by posing a model, in model space (units: blocks).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelVertex {
    pub pos: Vec3,
    pub normal: Vec3,
    /// Normalised texture coordinates.
    pub uv: Vec2,
}

#[derive(Clone, Debug, Default)]
pub struct EntityModel {
    pub identifier: Arc<str>,
    pub texture_width: u32,
    pub texture_height: u32,
}

impl EntityModel {
    /// Triangles (3 vertices each) for the model with the given bone poses applied.
    pub fn mesh(&self, _poses: &[BonePose]) -> Vec<ModelVertex> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Default)]
pub struct EntityModels {
    pub by_id: HashMap<Arc<str>, Arc<EntityModel>>,
}

impl EntityModels {
    pub fn load(_pack: &Pack) -> Self {
        EntityModels::default()
    }
    pub fn get(&self, id: &str) -> Option<&Arc<EntityModel>> {
        self.by_id.get(id)
    }
}
