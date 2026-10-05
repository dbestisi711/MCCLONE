//! Player controller, physics, mobs, item entities.
//!
//! OWNER: mob behaviour & physics agent. Public API used by `mc-game`
//! (keep these signatures stable; add freely):
//! - [`Player`]: `new`, `look`, `tick`, `eye_position`, `camera`
//! - [`EntityManager`]: `new`, `tick`, `spawn_item`, `attack`, `render_instances`
//!
//! Stub: a free-flying camera player and no mobs.

use glam::{IVec3, Vec3};
use mc_core::input::{InputState, Key};
use mc_core::render_types::{Camera, EntityRenderInstance};
use mc_core::{Aabb, Inventory, ItemStack, World};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode {
    Survival,
    Creative,
}

pub struct Player {
    /// Feet position.
    pub position: Vec3,
    pub prev_position: Vec3,
    pub velocity: Vec3,
    /// Radians (see `Camera::forward`).
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
    pub game_mode: GameMode,
    pub health: f32,
    pub max_health: f32,
    pub food: f32,
    pub saturation: f32,
    /// Remaining air ticks (300 = full).
    pub air: i32,
    pub sneaking: bool,
    pub sprinting: bool,
    pub in_water: bool,
    pub eyes_in_water: bool,
    pub inventory: Inventory,
    pub dead: bool,
    /// Accumulated horizontal distance walked (for view bobbing / walk anim).
    pub walk_distance: f32,
}

pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_HEIGHT: f32 = 1.8;
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;

impl Player {
    pub fn new(spawn: IVec3) -> Self {
        let p = spawn.as_vec3() + Vec3::new(0.5, 0.0, 0.5);
        Player {
            position: p,
            prev_position: p,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: true,
            game_mode: GameMode::Creative,
            health: 20.0,
            max_health: 20.0,
            food: 20.0,
            saturation: 5.0,
            air: 300,
            sneaking: false,
            sprinting: false,
            in_water: false,
            eyes_in_water: false,
            inventory: Inventory::default(),
            dead: false,
            walk_distance: 0.0,
        }
    }

    /// Apply mouse look (called every frame with raw mouse delta in pixels).
    pub fn look(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw += dx * sensitivity;
        self.pitch = (self.pitch - dy * sensitivity).clamp(-1.55, 1.55);
    }

    /// One fixed 20 TPS step of movement + physics.
    pub fn tick(&mut self, input: &InputState, _world: &World) {
        self.prev_position = self.position;
        let (s, c) = self.yaw.sin_cos();
        let fwd = Vec3::new(s, 0.0, -c);
        let right = Vec3::new(c, 0.0, s);
        let mut wish = Vec3::ZERO;
        if input.held(Key::Forward) {
            wish += fwd;
        }
        if input.held(Key::Back) {
            wish -= fwd;
        }
        if input.held(Key::Right) {
            wish += right;
        }
        if input.held(Key::Left) {
            wish -= right;
        }
        if input.held(Key::Jump) {
            wish += Vec3::Y;
        }
        if input.held(Key::Sneak) {
            wish -= Vec3::Y;
        }
        let speed = if input.held(Key::Sprint) { 2.0 } else { 0.6 };
        self.position += wish.normalize_or_zero() * speed;
    }

    pub fn interpolated_position(&self, partial: f32) -> Vec3 {
        self.prev_position.lerp(self.position, partial)
    }

    pub fn eye_position(&self, partial: f32) -> Vec3 {
        self.interpolated_position(partial) + Vec3::Y * PLAYER_EYE_HEIGHT
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::from_feet(self.position, PLAYER_WIDTH, PLAYER_HEIGHT)
    }

    pub fn camera(&self, partial: f32) -> Camera {
        Camera {
            position: self.eye_position(partial),
            yaw: self.yaw,
            pitch: self.pitch,
            ..Default::default()
        }
    }

    pub fn look_dir(&self) -> Vec3 {
        Camera {
            yaw: self.yaw,
            pitch: self.pitch,
            ..Default::default()
        }
        .forward()
    }
}

/// Owns all non-player entities (mobs, dropped items, falling blocks, arrows).
#[derive(Default)]
pub struct EntityManager {}

impl EntityManager {
    pub fn new(_seed: u64) -> Self {
        EntityManager {}
    }

    /// One fixed 20 TPS step: AI, physics, spawning/despawning, item pickup.
    pub fn tick(&mut self, _world: &mut World, _player: &mut Player) {}

    /// Spawn a dropped item at a position with a small random velocity.
    pub fn spawn_item(&mut self, _stack: ItemStack, _pos: Vec3) {}

    /// Player melee attack along a ray. Returns true if an entity was hit.
    pub fn attack(&mut self, _origin: Vec3, _dir: Vec3, _reach: f32, _damage: f32) -> bool {
        false
    }

    /// Distance to the closest entity hit by a ray (so block targeting can be blocked by mobs).
    pub fn raycast(&self, _origin: Vec3, _dir: Vec3, _reach: f32) -> Option<f32> {
        None
    }

    pub fn render_instances(&self, _partial: f32) -> Vec<EntityRenderInstance> {
        Vec::new()
    }

    pub fn count(&self) -> usize {
        0
    }
}
