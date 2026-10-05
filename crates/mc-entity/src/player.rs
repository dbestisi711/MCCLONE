//! The player: input → movement, survival mechanics (health, hunger, air,
//! fall/fire/lava/void damage, eating) and the first-person camera.

use glam::{IVec3, Vec2, Vec3};
use mc_core::input::{InputState, Key, MouseButton};
use mc_core::item::ItemKind;
use mc_core::render_types::Camera;
use mc_core::{Aabb, Inventory, ItemId, WORLD_MIN_Y, World, blocks};

use crate::EntityEvent;
use crate::physics::{self, Body};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode {
    Survival,
    Creative,
}

/// What hurt the player (or a mob). Sources with a position knock the victim
/// away from it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DamageSource {
    Generic,
    Fall,
    Drown,
    Starve,
    Lava,
    Fire,
    Cactus,
    /// Below the world. Also hurts creative players.
    Void,
    /// Head inside a solid block.
    Suffocation,
    /// Melee hit by a mob standing at `attacker`.
    Mob {
        attacker: Vec3,
    },
    /// Arrow (or other projectile) arriving from `from`.
    Projectile {
        from: Vec3,
    },
    /// Explosion centred at `center`.
    Explosion {
        center: Vec3,
    },
    /// Melee hit by the player standing at `attacker`.
    Player {
        attacker: Vec3,
    },
}

impl DamageSource {
    /// Position the victim is knocked away from.
    pub fn knockback_origin(&self) -> Option<Vec3> {
        match *self {
            DamageSource::Mob { attacker } | DamageSource::Player { attacker } => Some(attacker),
            DamageSource::Projectile { from } => Some(from),
            _ => None,
        }
    }
    /// Armour reduces this kind of damage.
    pub fn armor_applies(&self) -> bool {
        matches!(
            self,
            DamageSource::Mob { .. }
                | DamageSource::Player { .. }
                | DamageSource::Projectile { .. }
                | DamageSource::Explosion { .. }
                | DamageSource::Cactus
                | DamageSource::Generic
        )
    }
}

pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_HEIGHT: f32 = 1.8;
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
/// Box height while sneaking.
pub const PLAYER_SNEAK_HEIGHT: f32 = 1.5;
pub const PLAYER_SNEAK_EYE_HEIGHT: f32 = 1.27;
/// Steady walking speed on ordinary ground, blocks/tick (≈4.3 m/s).
pub const WALK_SPEED: f32 = 0.216;
pub const SPRINT_MULTIPLIER: f32 = 1.3;
pub const SNEAK_MULTIPLIER: f32 = 0.3;
/// Movement multiplier while eating.
pub const USE_ITEM_MULTIPLIER: f32 = 0.2;
pub const FLY_ACCEL: f32 = 0.05;
pub const FLY_SPRINT_ACCEL: f32 = 0.1;
pub const FLY_VERTICAL_ACCEL: f32 = 0.25;
pub const MAX_AIR: i32 = 300;
/// Ticks to eat a food item (1.6 s).
pub const EAT_TICKS: u32 = 32;
/// Hurt invulnerability after taking damage.
pub const INVULNERABLE_TICKS: u32 = 10;
/// Window for double-tapping forward (sprint) or jump (fly).
const DOUBLE_TAP_TICKS: u32 = 7;
/// Hunger: exhaustion that costs one saturation/food point.
const EXHAUSTION_PER_POINT: f32 = 4.0;

/// An eating action in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eating {
    pub item: ItemId,
    pub slot: usize,
    pub ticks: u32,
}

pub struct Player {
    /// Feet position.
    pub position: Vec3,
    pub prev_position: Vec3,
    /// Blocks per tick.
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
    /// Accumulated horizontal distance walked on the ground (for view bobbing / walk anim).
    pub walk_distance: f32,
    pub prev_walk_distance: f32,

    /// Step up 1-block ledges automatically when walking into them (touch controls).
    pub auto_jump: bool,
    /// Apply view bobbing to [`Player::camera`].
    pub view_bobbing: bool,
    pub in_lava: bool,
    pub eyes_in_lava: bool,
    pub exhaustion: f32,
    pub fall_distance: f32,
    /// Counts down from 10 after being hurt (camera shake / red flash).
    pub hurt_time: u32,
    pub invulnerable_ticks: u32,
    /// Ticks left burning.
    pub fire_ticks: i32,
    pub eye_height: f32,
    pub prev_eye_height: f32,
    /// View-bob amplitude 0..1.
    pub bob: f32,
    pub prev_bob: f32,
    /// Field of view multiplier (sprinting widens the view).
    pub fov_scale: f32,
    pub prev_fov_scale: f32,
    pub last_damage: Option<DamageSource>,
    pub eating: Option<Eating>,
    /// Ticks lived.
    pub age: u64,
    pub collided_h: bool,
    /// Box is in the low (sneaking) pose.
    pub crouched: bool,

    pub(crate) events: Vec<EntityEvent>,
    last_hurt_amount: f32,
    food_timer: u32,
    drown_timer: u32,
    forward_tap: u32,
    jump_tap: u32,
    jump_held_last: bool,
    forward_held_last: bool,
    fly_key_held_last: bool,
}

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
            air: MAX_AIR,
            sneaking: false,
            sprinting: false,
            in_water: false,
            eyes_in_water: false,
            inventory: Inventory::default(),
            dead: false,
            walk_distance: 0.0,
            prev_walk_distance: 0.0,
            auto_jump: false,
            view_bobbing: true,
            in_lava: false,
            eyes_in_lava: false,
            exhaustion: 0.0,
            fall_distance: 0.0,
            hurt_time: 0,
            invulnerable_ticks: 0,
            fire_ticks: 0,
            eye_height: PLAYER_EYE_HEIGHT,
            prev_eye_height: PLAYER_EYE_HEIGHT,
            bob: 0.0,
            prev_bob: 0.0,
            fov_scale: 1.0,
            prev_fov_scale: 1.0,
            last_damage: None,
            eating: None,
            age: 0,
            collided_h: false,
            crouched: false,
            events: Vec::new(),
            last_hurt_amount: 0.0,
            food_timer: 0,
            drown_timer: 0,
            forward_tap: 0,
            jump_tap: 0,
            jump_held_last: false,
            forward_held_last: false,
            fly_key_held_last: false,
        }
    }

    pub fn creative(&self) -> bool {
        self.game_mode == GameMode::Creative
    }

    /// Apply mouse look (called every frame with raw mouse delta in pixels).
    pub fn look(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw += dx * sensitivity;
        self.pitch = (self.pitch - dy * sensitivity).clamp(-1.55, 1.55);
    }

    /// Current box height (lower while sneaking).
    pub fn height(&self) -> f32 {
        if self.crouched {
            PLAYER_SNEAK_HEIGHT
        } else {
            PLAYER_HEIGHT
        }
    }

    fn body(&self) -> Body {
        Body {
            pos: self.position,
            vel: self.velocity,
            width: PLAYER_WIDTH,
            height: self.height(),
            step_height: physics::STEP_HEIGHT,
            on_ground: self.on_ground,
            collided_h: self.collided_h,
            collided_v: false,
            in_water: self.in_water,
            in_lava: self.in_lava,
            submerged: 0.0,
            fall_distance: self.fall_distance,
        }
    }

    fn apply_body(&mut self, b: &Body) {
        self.position = b.pos;
        self.velocity = b.vel;
        self.on_ground = b.on_ground;
        self.collided_h = b.collided_h;
        self.in_water = b.in_water;
        self.in_lava = b.in_lava;
        self.fall_distance = b.fall_distance;
    }

    /// Horizontal movement input in the player's frame: x = strafe right,
    /// y = forward, length ≤ 1. Keys and the analog `move_axis` are summed.
    pub fn move_input(input: &InputState) -> Vec2 {
        let mut v = Vec2::new(input.move_axis.0, input.move_axis.1);
        if input.held(Key::Forward) {
            v.y += 1.0;
        }
        if input.held(Key::Back) {
            v.y -= 1.0;
        }
        if input.held(Key::Right) {
            v.x += 1.0;
        }
        if input.held(Key::Left) {
            v.x -= 1.0;
        }
        if !v.is_finite() {
            return Vec2::ZERO;
        }
        if v.length_squared() > 1.0 {
            v = v.normalize();
        }
        v
    }

    /// Unit forward and right vectors on the horizontal plane.
    pub fn horizontal_axes(&self) -> (Vec3, Vec3) {
        let (s, c) = self.yaw.sin_cos();
        (Vec3::new(s, 0.0, -c), Vec3::new(c, 0.0, s))
    }

    /// One fixed 20 TPS step of movement + physics + survival mechanics.
    pub fn tick(&mut self, input: &InputState, world: &World) {
        self.prev_position = self.position;
        self.prev_eye_height = self.eye_height;
        self.prev_walk_distance = self.walk_distance;
        self.prev_bob = self.bob;
        self.prev_fov_scale = self.fov_scale;
        if self.dead {
            self.velocity = Vec3::ZERO;
            self.eating = None;
            self.sprinting = false;
            self.bob = 0.0;
            return;
        }
        self.age += 1;
        self.hurt_time = self.hurt_time.saturating_sub(1);
        self.invulnerable_ticks = self.invulnerable_ticks.saturating_sub(1);
        self.forward_tap = self.forward_tap.saturating_sub(1);
        self.jump_tap = self.jump_tap.saturating_sub(1);
        let creative = self.creative();

        // Key edges. `pressed` only reaches the first tick of a frame and is
        // lost on frames without a tick, so also detect held transitions.
        let fly_key = input.pressed(Key::ToggleFly)
            || (input.held(Key::ToggleFly) && !self.fly_key_held_last);
        self.fly_key_held_last = input.held(Key::ToggleFly);
        let jump_pressed =
            input.pressed(Key::Jump) || (input.held(Key::Jump) && !self.jump_held_last);
        self.jump_held_last = input.held(Key::Jump);
        let forward_pressed =
            input.pressed(Key::Forward) || (input.held(Key::Forward) && !self.forward_held_last);
        self.forward_held_last = input.held(Key::Forward);

        // Flight toggles: F key or double-tap jump (creative only).
        if fly_key && creative {
            self.flying = !self.flying;
            self.velocity.y = 0.0;
        }
        if jump_pressed {
            if self.jump_tap > 0 && creative {
                self.flying = !self.flying;
                self.velocity.y = 0.0;
                self.jump_tap = 0;
            } else {
                self.jump_tap = DOUBLE_TAP_TICKS;
            }
        }
        if !creative && self.flying && self.on_ground {
            self.flying = false;
        }

        let mv = Self::move_input(input);
        let mut sprint_request = input.held(Key::Sprint);
        if forward_pressed {
            if self.forward_tap > 0 {
                sprint_request = true;
            }
            self.forward_tap = DOUBLE_TAP_TICKS;
        }

        // Sneaking / crouch pose.
        self.sneaking = input.held(Key::Sneak) && !self.flying;
        if self.sneaking {
            self.crouched = true;
        } else if self.crouched {
            let standing = Aabb::from_feet(self.position, PLAYER_WIDTH, PLAYER_HEIGHT);
            if !physics::collides(world, &standing) {
                self.crouched = false;
            }
        }

        // Eating (right mouse held with food selected).
        let using = self.try_eat(input.mouse_held(MouseButton::Right));

        // Sprinting.
        let can_sprint = mv.y > 0.7
            && !self.sneaking
            && !using
            && (creative || self.food > 6.0)
            && !self.in_lava;
        if sprint_request && can_sprint && !self.collided_h {
            self.sprinting = true;
        }
        if !can_sprint || (self.collided_h && !self.flying) {
            self.sprinting = false;
        }

        let mut mult = 1.0;
        if self.sneaking {
            mult *= SNEAK_MULTIPLIER;
        }
        if using {
            mult *= USE_ITEM_MULTIPLIER;
        }
        let (fwd, right) = self.horizontal_axes();
        let wish = (right * mv.x + fwd * mv.y) * mult;

        let before = self.position;
        if self.flying {
            self.fly(input, world, wish);
        } else if self.in_water || self.in_lava {
            self.swim(input, world, wish);
        } else {
            self.walk(input, world, wish, fwd);
        }
        let moved = self.position - before;
        let horiz = Vec2::new(moved.x, moved.z).length();

        // Fluids at the eyes.
        let eye = self.position + Vec3::Y * self.eye_height;
        let eye_fluid = physics::fluid_at_point(world, eye);
        self.eyes_in_water = eye_fluid == Some(blocks::WATER);
        self.eyes_in_lava = eye_fluid == Some(blocks::LAVA);

        // Exhaustion from movement.
        if !creative {
            if self.in_water {
                self.exhaustion += 0.01 * moved.length();
            } else if self.sprinting && self.on_ground {
                self.exhaustion += 0.1 * horiz;
            }
        }

        // View bobbing and walk distance.
        let grounded = self.on_ground && !self.flying;
        if grounded {
            self.walk_distance += horiz;
        }
        let target_bob = if grounded {
            (horiz / WALK_SPEED).min(1.0)
        } else {
            0.0
        };
        self.bob += (target_bob - self.bob) * 0.4;
        let target_eye = if self.crouched {
            PLAYER_SNEAK_EYE_HEIGHT
        } else {
            PLAYER_EYE_HEIGHT
        };
        self.eye_height += (target_eye - self.eye_height) * 0.5;
        let target_fov = if self.sprinting { 1.1 } else { 1.0 };
        self.fov_scale += (target_fov - self.fov_scale) * 0.5;

        self.environment_damage(world);
        if !creative {
            self.hunger_tick();
        } else {
            self.air = MAX_AIR;
            self.fire_ticks = 0;
        }
    }

    fn fly(&mut self, input: &InputState, world: &World, wish: Vec3) {
        let accel = if self.sprinting {
            FLY_SPRINT_ACCEL
        } else {
            FLY_ACCEL
        };
        self.velocity += wish * accel;
        let mut vertical = 0.0;
        if input.held(Key::Jump) {
            vertical += 1.0;
        }
        if input.held(Key::Sneak) {
            vertical -= 1.0;
        }
        self.velocity.y += vertical * FLY_VERTICAL_ACCEL;
        let mut body = self.body();
        physics::move_body(world, &mut body, self.velocity, false);
        body.fall_distance = 0.0;
        self.apply_body(&body);
        self.velocity.x *= physics::AIR_FRICTION;
        self.velocity.z *= physics::AIR_FRICTION;
        self.velocity.y *= 0.6;
        if self.on_ground && !self.creative() {
            self.flying = false;
        }
    }

    fn swim(&mut self, input: &InputState, world: &World, wish: Vec3) {
        let drag = if self.in_water {
            physics::WATER_DRAG
        } else {
            physics::LAVA_DRAG
        };
        let accel = if self.sprinting && self.in_water {
            physics::FLUID_ACCEL * 2.0
        } else {
            physics::FLUID_ACCEL
        };
        self.velocity += wish * accel;
        if input.held(Key::Jump) {
            self.velocity.y += 0.04;
        } else if input.held(Key::Sneak) {
            self.velocity.y -= 0.04;
        }
        let mut body = self.body();
        let was_lava = body.in_lava;
        physics::move_body(world, &mut body, self.velocity, false);
        if was_lava && !body.in_water {
            body.fall_distance *= 0.5;
        }
        self.apply_body(&body);
        self.velocity *= drag;
        self.velocity.y -= physics::FLUID_GRAVITY;
        if self.collided_h && self.can_leave_fluid(world) {
            self.velocity.y = 0.3;
        }
    }

    /// Is there room to hop up onto the ledge we are pushing against?
    fn can_leave_fluid(&self, world: &World) -> bool {
        let rise = 0.6 - (self.position.y - self.prev_position.y);
        let a = self.aabb().translate(Vec3::new(
            self.velocity.x,
            self.velocity.y + rise,
            self.velocity.z,
        ));
        !physics::collides(world, &a)
    }

    fn walk(&mut self, input: &InputState, world: &World, wish: Vec3, fwd: Vec3) {
        let climbing = physics::is_climbable_at(world, self.position);
        let slip = if self.on_ground {
            physics::slipperiness_below(world, self.position)
        } else {
            1.0
        };
        let friction = slip * physics::AIR_FRICTION;
        let speed = WALK_SPEED
            * if self.sprinting {
                SPRINT_MULTIPLIER
            } else {
                1.0
            };
        let accel = if self.on_ground {
            physics::ground_accel(speed, friction)
        } else if self.sprinting {
            physics::AIR_ACCEL * SPRINT_MULTIPLIER
        } else {
            physics::AIR_ACCEL
        };
        self.velocity += wish * accel;

        let wants_jump = input.held(Key::Jump)
            || (self.auto_jump
                && !self.sneaking
                && self.on_ground
                && self.auto_jump_wanted(world, wish));
        if wants_jump && self.on_ground {
            self.velocity.y = physics::JUMP_VELOCITY;
            if self.sprinting {
                self.velocity += fwd * 0.2;
            }
            if !self.creative() {
                self.exhaustion += if self.sprinting { 0.2 } else { 0.05 };
            }
        }

        if climbing {
            self.velocity.x = self.velocity.x.clamp(-0.15, 0.15);
            self.velocity.z = self.velocity.z.clamp(-0.15, 0.15);
            self.velocity.y = self.velocity.y.max(-0.15);
            if self.sneaking && self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
        }

        let mut body = self.body();
        let out = physics::move_body(world, &mut body, self.velocity, self.sneaking);
        if climbing {
            body.fall_distance = 0.0;
        }
        self.apply_body(&body);
        if climbing && (self.collided_h || input.held(Key::Jump)) {
            self.velocity.y = 0.2;
        }
        if let Some(fall) = out.landed_fall {
            self.on_landed(fall);
        }
        self.velocity.y = (self.velocity.y - physics::GRAVITY) * physics::VERTICAL_DRAG;
        self.velocity.x *= friction;
        self.velocity.z *= friction;
    }

    /// A 1-block ledge right in front that a jump would clear.
    fn auto_jump_wanted(&self, world: &World, wish: Vec3) -> bool {
        if wish.length_squared() < 1e-4 {
            return false;
        }
        let dir = wish.normalize();
        let a = self.aabb();
        let probe = a.translate(dir * 0.3);
        if !physics::collides(world, &probe) {
            return false;
        }
        // Low obstacles are handled by step-up.
        if !physics::collides(world, &probe.translate(Vec3::Y * physics::STEP_HEIGHT)) {
            return false;
        }
        let raised = Vec3::Y * 1.05;
        !physics::collides(world, &probe.translate(raised))
            && !physics::collides(world, &a.translate(raised))
    }

    fn on_landed(&mut self, fall: f32) {
        if self.creative() || self.in_water {
            return;
        }
        let dmg = (fall - 3.0 - 1e-3).ceil();
        if dmg > 0.0 {
            self.damage(dmg, DamageSource::Fall);
        }
    }

    /// Damage from the environment: lava, fire, drowning, cactus, void.
    fn environment_damage(&mut self, world: &World) {
        if self.position.y < (WORLD_MIN_Y - 64) as f32 && self.age.is_multiple_of(10) {
            self.damage(4.0, DamageSource::Void);
        }
        if self.creative() {
            return;
        }
        if self.in_lava {
            self.damage(4.0, DamageSource::Lava);
            self.fire_ticks = self.fire_ticks.max(300);
        }
        if self.fire_ticks > 0 {
            if self.in_water {
                self.fire_ticks = 0;
            } else {
                self.fire_ticks -= 1;
                if self.fire_ticks % 20 == 0 {
                    self.damage(1.0, DamageSource::Fire);
                }
            }
        }
        if self.eyes_in_water {
            if self.air > 0 {
                self.air -= 1;
                self.drown_timer = 0;
            } else {
                self.drown_timer += 1;
                if self.drown_timer >= 20 {
                    self.drown_timer = 0;
                    self.damage(2.0, DamageSource::Drown);
                }
            }
        } else {
            self.air = (self.air + 4).min(MAX_AIR);
            self.drown_timer = 0;
        }
        if physics::touches_block(world, &self.aabb(), 0.01, blocks::CACTUS) {
            self.damage(1.0, DamageSource::Cactus);
        }
        if physics::in_opaque_block(world, self.position + Vec3::Y * self.eye_height) {
            self.damage(1.0, DamageSource::Suffocation);
        }
    }

    fn hunger_tick(&mut self) {
        while self.exhaustion >= EXHAUSTION_PER_POINT {
            self.exhaustion -= EXHAUSTION_PER_POINT;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else {
                self.food = (self.food - 1.0).max(0.0);
            }
        }
        self.food_timer += 1;
        let hurt = self.health < self.max_health;
        if self.saturation > 0.0 && self.food >= 20.0 && hurt {
            if self.food_timer >= 10 {
                let s = self.saturation.min(6.0);
                self.heal(s / 6.0);
                self.exhaustion += s;
                self.food_timer = 0;
            }
        } else if self.food >= 18.0 && hurt {
            if self.food_timer >= 80 {
                self.heal(1.0);
                self.exhaustion += 6.0;
                self.food_timer = 0;
            }
        } else if self.food <= 0.0 {
            if self.food_timer >= 80 {
                // Normal difficulty: starvation stops at half a heart.
                if self.health > 1.0 {
                    self.damage(1.0, DamageSource::Starve);
                }
                self.food_timer = 0;
            }
        } else {
            self.food_timer = 0;
        }
    }

    pub fn heal(&mut self, amount: f32) {
        if !self.dead {
            self.health = (self.health + amount).min(self.max_health);
        }
    }

    /// Total armour points from worn armour items.
    pub fn armor_points(&self) -> u32 {
        self.inventory
            .armor
            .iter()
            .flatten()
            .map(|s| match s.item.kind() {
                ItemKind::Armor { defense, .. } => defense as u32,
                _ => 0,
            })
            .sum()
    }

    /// Hurt the player. Returns true if any damage was applied. Creative
    /// players only take void damage. After a hit the player is briefly
    /// invulnerable (only a stronger hit applies its difference).
    pub fn damage(&mut self, amount: f32, source: DamageSource) -> bool {
        if self.dead || amount <= 0.0 || !amount.is_finite() {
            return false;
        }
        if self.creative() && source != DamageSource::Void {
            return false;
        }
        let mut amount = amount;
        if source.armor_applies() {
            amount *= 1.0 - self.armor_points().min(20) as f32 / 25.0;
        }
        let applied = if self.invulnerable_ticks > 0 {
            if amount <= self.last_hurt_amount {
                return false;
            }
            let extra = amount - self.last_hurt_amount;
            self.last_hurt_amount = amount;
            extra
        } else {
            self.last_hurt_amount = amount;
            self.invulnerable_ticks = INVULNERABLE_TICKS;
            self.hurt_time = 10;
            if let Some(from) = source.knockback_origin() {
                self.knockback(from, 0.4);
            }
            amount
        };
        self.health -= applied;
        self.exhaustion += 0.1;
        self.last_damage = Some(source);
        self.events.push(EntityEvent::PlayerHurt {
            amount: applied,
            source,
        });
        self.events
            .push(EntityEvent::sound("game.player.hurt", self.position));
        if self.health <= 0.0 {
            self.health = 0.0;
            self.dead = true;
            self.flying = false;
            self.eating = None;
            self.events.push(EntityEvent::PlayerDied { source });
            self.events
                .push(EntityEvent::sound("game.player.die", self.position));
        }
        true
    }

    /// Push the player horizontally away from `from` (and up if on the ground).
    pub fn knockback(&mut self, from: Vec3, strength: f32) {
        let mut d = self.position - from;
        d.y = 0.0;
        let d = if d.length_squared() < 1e-6 {
            -self.horizontal_axes().0
        } else {
            d.normalize()
        };
        self.velocity.x = self.velocity.x * 0.5 + d.x * strength;
        self.velocity.z = self.velocity.z * 0.5 + d.z * strength;
        if self.on_ground {
            self.velocity.y = (self.velocity.y * 0.5 + strength).min(0.4);
        }
    }

    /// Advance eating by one tick while `use_held` (right mouse) is down with
    /// a food item selected. Returns true while eating. Finishing restores
    /// hunger/saturation and consumes the item (survival).
    pub fn try_eat(&mut self, use_held: bool) -> bool {
        let slot = self.inventory.selected;
        let food = self.inventory.selected_item().and_then(|i| match i.kind() {
            ItemKind::Food { hunger, saturation } => Some((i, hunger, saturation)),
            _ => None,
        });
        let Some((item, hunger, saturation)) = food else {
            self.eating = None;
            return false;
        };
        if !use_held || self.dead || self.food >= 20.0 {
            self.eating = None;
            return false;
        }
        let mut e = match self.eating {
            Some(e) if e.item == item && e.slot == slot => e,
            _ => Eating {
                item,
                slot,
                ticks: 0,
            },
        };
        e.ticks += 1;
        if e.ticks % 4 == 0 && e.ticks < EAT_TICKS {
            self.events
                .push(EntityEvent::sound("random.eat", self.position));
        }
        if e.ticks >= EAT_TICKS {
            self.food = (self.food + hunger as f32).min(20.0);
            self.saturation = (self.saturation + saturation).min(self.food);
            if !self.creative() {
                self.inventory.consume_selected(1);
            }
            self.events
                .push(EntityEvent::sound("random.burp", self.position));
            self.eating = None;
            return false;
        }
        self.eating = Some(e);
        true
    }

    /// Eating progress 0..1 (for hand animation / HUD), if eating.
    pub fn eating_progress(&self) -> Option<f32> {
        self.eating
            .map(|e| (e.ticks as f32 / EAT_TICKS as f32).min(1.0))
    }

    pub fn interpolated_position(&self, partial: f32) -> Vec3 {
        self.prev_position.lerp(self.position, partial)
    }

    pub fn eye_position(&self, partial: f32) -> Vec3 {
        let eye = self.prev_eye_height + (self.eye_height - self.prev_eye_height) * partial;
        self.interpolated_position(partial) + Vec3::Y * eye
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::from_feet(self.position, PLAYER_WIDTH, self.height())
    }

    /// Camera offset from view bobbing (already included in [`Player::camera`]).
    pub fn view_bob_offset(&self, partial: f32) -> Vec3 {
        let dist =
            self.prev_walk_distance + (self.walk_distance - self.prev_walk_distance) * partial;
        let amount = self.prev_bob + (self.bob - self.prev_bob) * partial;
        if amount <= 1e-3 {
            return Vec3::ZERO;
        }
        let phase = dist * std::f32::consts::PI / 1.5;
        let (_, right) = self.horizontal_axes();
        right * (phase.sin() * amount * 0.05) - Vec3::Y * ((phase.cos().abs()) * amount * 0.07)
    }

    pub fn camera(&self, partial: f32) -> Camera {
        let mut position = self.eye_position(partial);
        if self.view_bobbing {
            position += self.view_bob_offset(partial);
        }
        let fov = self.prev_fov_scale + (self.fov_scale - self.prev_fov_scale) * partial;
        let base = Camera::default();
        Camera {
            position,
            yaw: self.yaw,
            pitch: self.pitch,
            fov_y: base.fov_y * fov,
            ..base
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

    /// Player can be targeted by hostile mobs.
    pub fn targetable(&self) -> bool {
        !self.dead && self.game_mode == GameMode::Survival
    }
}
