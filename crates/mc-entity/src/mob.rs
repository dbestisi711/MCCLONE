//! Mob kinds, their static properties, and per-mob state, damage and physics.
//! Behaviour (goals, pathing) lives in `ai.rs`; poses in `render.rs`.

use glam::{IVec3, Vec2, Vec3};
use mc_core::{ItemId, ItemStack, WORLD_MIN_Y, World, blocks};

use crate::ai::AiState;
use crate::physics::{self, Body};
use crate::player::DamageSource;
use crate::{EntityEvent, MobCtx};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MobKind {
    Pig,
    Cow,
    Sheep,
    Chicken,
    Zombie,
    Skeleton,
    Creeper,
    Spider,
}

/// Loot entry: `min..=max` of `item`, or of `cooked` when the mob died burning.
#[derive(Clone, Copy, Debug)]
pub struct LootDrop {
    pub item: &'static str,
    pub min: u8,
    pub max: u8,
    pub cooked: Option<&'static str>,
}

const fn loot(item: &'static str, min: u8, max: u8) -> LootDrop {
    LootDrop {
        item,
        min,
        max,
        cooked: None,
    }
}

const fn meat(item: &'static str, min: u8, max: u8, cooked: &'static str) -> LootDrop {
    LootDrop {
        item,
        min,
        max,
        cooked: Some(cooked),
    }
}

/// Static properties of a mob kind. Model ids and texture paths refer to the
/// resource pack (`models/entity/*.geo.json`, `textures/entity/...`).
#[derive(Clone, Copy, Debug)]
pub struct MobSpec {
    pub name: &'static str,
    /// Geometry identifier in the pack.
    pub model: &'static str,
    /// Pack-relative texture path without extension.
    pub texture: &'static str,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub max_health: f32,
    /// Steady wandering speed, blocks/tick.
    pub walk_speed: f32,
    /// Steady speed when chasing a target, blocks/tick.
    pub chase_speed: f32,
    pub attack_damage: f32,
    pub hostile: bool,
    /// Burns in sunlight.
    pub undead: bool,
    pub drops: &'static [LootDrop],
    pub ambient_sound: &'static str,
    pub hurt_sound: &'static str,
    pub death_sound: &'static str,
}

static PIG: MobSpec = MobSpec {
    name: "pig",
    model: "geometry.pig.v3",
    texture: "textures/entity/pig/pig_v3",
    width: 0.9,
    height: 0.9,
    eye_height: 0.75,
    max_health: 10.0,
    walk_speed: 0.09,
    chase_speed: 0.09,
    attack_damage: 0.0,
    hostile: false,
    undead: false,
    drops: &[meat("porkchop", 1, 3, "cooked_porkchop")],
    ambient_sound: "mob.pig.say",
    hurt_sound: "mob.pig.say",
    death_sound: "mob.pig.death",
};

static COW: MobSpec = MobSpec {
    name: "cow",
    model: "geometry.cow.v2",
    texture: "textures/entity/cow/cow_v2",
    width: 0.9,
    height: 1.4,
    eye_height: 1.3,
    max_health: 10.0,
    walk_speed: 0.08,
    chase_speed: 0.08,
    attack_damage: 0.0,
    hostile: false,
    undead: false,
    drops: &[meat("beef", 1, 3, "cooked_beef"), loot("leather", 0, 2)],
    ambient_sound: "mob.cow.say",
    hurt_sound: "mob.cow.hurt",
    death_sound: "mob.cow.hurt",
};

static SHEEP: MobSpec = MobSpec {
    name: "sheep",
    model: "geometry.sheep.v1.8",
    texture: "textures/entity/sheep/sheep",
    width: 0.9,
    height: 1.3,
    eye_height: 1.2,
    max_health: 8.0,
    walk_speed: 0.09,
    chase_speed: 0.09,
    attack_damage: 0.0,
    hostile: false,
    undead: false,
    // Wool is added in code unless the sheep is sheared.
    drops: &[meat("mutton", 1, 2, "cooked_mutton")],
    ambient_sound: "mob.sheep.say",
    hurt_sound: "mob.sheep.say",
    death_sound: "mob.sheep.say",
};

static CHICKEN: MobSpec = MobSpec {
    name: "chicken",
    model: "geometry.chicken.v1.12",
    texture: "textures/entity/chicken/chicken",
    width: 0.4,
    height: 0.7,
    eye_height: 0.64,
    max_health: 4.0,
    walk_speed: 0.08,
    chase_speed: 0.08,
    attack_damage: 0.0,
    hostile: false,
    undead: false,
    drops: &[
        meat("chicken", 1, 1, "cooked_chicken"),
        loot("feather", 0, 2),
    ],
    ambient_sound: "mob.chicken.say",
    hurt_sound: "mob.chicken.hurt",
    death_sound: "mob.chicken.hurt",
};

static ZOMBIE: MobSpec = MobSpec {
    name: "zombie",
    model: "geometry.zombie.v1.8",
    texture: "textures/entity/zombie/zombie",
    width: 0.6,
    height: 1.95,
    eye_height: 1.74,
    max_health: 20.0,
    walk_speed: 0.07,
    chase_speed: 0.14,
    attack_damage: 3.0,
    hostile: true,
    undead: true,
    drops: &[loot("rotten_flesh", 0, 2)],
    ambient_sound: "mob.zombie.say",
    hurt_sound: "mob.zombie.hurt",
    death_sound: "mob.zombie.death",
};

static SKELETON: MobSpec = MobSpec {
    name: "skeleton",
    model: "geometry.skeleton.v1.8",
    texture: "textures/entity/skeleton/skeleton",
    width: 0.6,
    height: 1.99,
    eye_height: 1.74,
    max_health: 20.0,
    walk_speed: 0.07,
    chase_speed: 0.13,
    attack_damage: 2.0,
    hostile: true,
    undead: true,
    drops: &[loot("bone", 0, 2), loot("arrow", 0, 2)],
    ambient_sound: "mob.skeleton.say",
    hurt_sound: "mob.skeleton.hurt",
    death_sound: "mob.skeleton.death",
};

static CREEPER: MobSpec = MobSpec {
    name: "creeper",
    model: "geometry.creeper.v1.8",
    texture: "textures/entity/creeper/creeper",
    width: 0.6,
    height: 1.7,
    eye_height: 1.45,
    max_health: 20.0,
    walk_speed: 0.07,
    chase_speed: 0.13,
    attack_damage: 0.0,
    hostile: true,
    undead: false,
    drops: &[loot("gunpowder", 0, 2)],
    ambient_sound: "",
    hurt_sound: "mob.creeper.say",
    death_sound: "mob.creeper.death",
};

static SPIDER: MobSpec = MobSpec {
    name: "spider",
    model: "geometry.spider.v1.8",
    texture: "textures/entity/spider/spider",
    width: 1.4,
    height: 0.9,
    eye_height: 0.65,
    max_health: 16.0,
    walk_speed: 0.09,
    chase_speed: 0.17,
    attack_damage: 2.0,
    hostile: true,
    undead: false,
    drops: &[loot("string", 0, 2)],
    ambient_sound: "mob.spider.say",
    hurt_sound: "mob.spider.say",
    death_sound: "mob.spider.death",
};

/// Geometry used for a sheared sheep.
pub const SHEEP_SHEARED_MODEL: &str = "geometry.sheep.sheared.v1.8";
/// Arrow projectile model and texture.
pub const ARROW_MODEL: &str = "geometry.arrow";
pub const ARROW_TEXTURE: &str = "textures/entity/arrows";

/// Creeper fuse length in ticks (1.5 s).
pub const CREEPER_FUSE: i32 = 30;
pub const CREEPER_POWER: f32 = 3.0;
/// Ticks a dying mob lies on its side before it is removed.
pub const DEATH_TICKS: u32 = 20;

impl MobKind {
    pub const ALL: [MobKind; 8] = [
        MobKind::Pig,
        MobKind::Cow,
        MobKind::Sheep,
        MobKind::Chicken,
        MobKind::Zombie,
        MobKind::Skeleton,
        MobKind::Creeper,
        MobKind::Spider,
    ];

    pub fn spec(self) -> &'static MobSpec {
        match self {
            MobKind::Pig => &PIG,
            MobKind::Cow => &COW,
            MobKind::Sheep => &SHEEP,
            MobKind::Chicken => &CHICKEN,
            MobKind::Zombie => &ZOMBIE,
            MobKind::Skeleton => &SKELETON,
            MobKind::Creeper => &CREEPER,
            MobKind::Spider => &SPIDER,
        }
    }

    pub fn hostile(self) -> bool {
        self.spec().hostile
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn by_name(name: &str) -> Option<MobKind> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A living mob.
#[derive(Clone, Debug)]
pub struct Mob {
    pub id: u64,
    pub kind: MobKind,
    pub body: Body,
    pub prev_pos: Vec3,
    /// Body yaw, radians (same convention as `Camera::yaw`).
    pub yaw: f32,
    pub prev_yaw: f32,
    /// Absolute head yaw, radians.
    pub head_yaw: f32,
    pub prev_head_yaw: f32,
    /// Head pitch, radians, positive looks up.
    pub pitch: f32,
    pub prev_pitch: f32,
    pub health: f32,
    /// Counts down from 10 after being hurt (red flash).
    pub hurt_time: u32,
    pub invulnerable: u32,
    /// > 0 while dying; removed at [`DEATH_TICKS`].
    pub death_time: u32,
    pub fire_ticks: i32,
    /// Walk animation phase and amount (0..1).
    pub limb_swing: f32,
    pub prev_limb_swing: f32,
    pub limb_amount: f32,
    pub prev_limb_amount: f32,
    pub age: u32,
    /// Packed light at the mob (`sky << 4 | block`), sampled each tick.
    pub light: u8,
    /// Creeper swell, 0..=CREEPER_FUSE.
    pub fuse: i32,
    pub prev_fuse: i32,
    pub sheared: bool,
    /// Sheep eating grass animation, counts down.
    pub graze_ticks: u32,
    pub egg_timer: u32,
    /// Arm swing animation after a melee attack, counts down from 10.
    pub attack_anim: u32,
    /// Chicken wing flap amount (0..1) and phase.
    pub flap: f32,
    pub prev_flap: f32,
    /// Never despawns naturally.
    pub persistent: bool,
    pub ai: AiState,
    pub(crate) exploded: bool,
    pub(crate) no_action_ticks: u32,
    pub(crate) just_hurt: bool,
    last_hurt_amount: f32,
}

/// Wrap an angle to `(-π, π]`.
pub fn wrap_angle(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let mut a = a % t;
    if a > std::f32::consts::PI {
        a -= t;
    } else if a <= -std::f32::consts::PI {
        a += t;
    }
    a
}

/// Rotate `cur` toward `target` by at most `max_step` radians.
pub fn approach_angle(cur: f32, target: f32, max_step: f32) -> f32 {
    let d = wrap_angle(target - cur);
    cur + d.clamp(-max_step, max_step)
}

/// Yaw (radians, `Camera::yaw` convention) that looks along `dir`.
pub fn yaw_of(dir: Vec3) -> f32 {
    dir.x.atan2(-dir.z)
}

/// Pitch (radians, positive up) that looks along `dir`.
pub fn pitch_of(dir: Vec3) -> f32 {
    dir.y.atan2(Vec2::new(dir.x, dir.z).length())
}

impl Mob {
    pub fn new(id: u64, kind: MobKind, pos: Vec3, yaw: f32) -> Self {
        let spec = kind.spec();
        Mob {
            id,
            kind,
            body: Body::new(pos, spec.width, spec.height),
            prev_pos: pos,
            yaw,
            prev_yaw: yaw,
            head_yaw: yaw,
            prev_head_yaw: yaw,
            pitch: 0.0,
            prev_pitch: 0.0,
            health: spec.max_health,
            hurt_time: 0,
            invulnerable: 0,
            death_time: 0,
            fire_ticks: 0,
            limb_swing: 0.0,
            prev_limb_swing: 0.0,
            limb_amount: 0.0,
            prev_limb_amount: 0.0,
            age: 0,
            light: 0xF0,
            fuse: 0,
            prev_fuse: 0,
            sheared: false,
            graze_ticks: 0,
            egg_timer: 6000 + (id.wrapping_mul(2654435761) % 6000) as u32,
            attack_anim: 0,
            flap: 0.0,
            prev_flap: 0.0,
            persistent: false,
            ai: AiState::default(),
            exploded: false,
            no_action_ticks: 0,
            just_hurt: false,
            last_hurt_amount: 0.0,
        }
    }

    pub fn spec(&self) -> &'static MobSpec {
        self.kind.spec()
    }

    pub fn pos(&self) -> Vec3 {
        self.body.pos
    }

    pub fn aabb(&self) -> mc_core::Aabb {
        self.body.aabb()
    }

    pub fn eye(&self) -> Vec3 {
        self.body.pos + Vec3::Y * self.spec().eye_height
    }

    pub fn center(&self) -> Vec3 {
        self.body.pos + Vec3::Y * (self.body.height * 0.5)
    }

    pub fn alive(&self) -> bool {
        self.health > 0.0 && self.death_time == 0
    }

    /// Fully gone (death animation over, or exploded).
    pub fn removed(&self) -> bool {
        self.death_time >= DEATH_TICKS
    }

    /// Hurt the mob. Returns true if damage was applied.
    pub fn hurt(&mut self, amount: f32, source: DamageSource) -> bool {
        if !self.alive() || amount <= 0.0 || !amount.is_finite() {
            return false;
        }
        let applied = if self.invulnerable > 0 {
            if amount <= self.last_hurt_amount {
                return false;
            }
            let extra = amount - self.last_hurt_amount;
            self.last_hurt_amount = amount;
            extra
        } else {
            self.last_hurt_amount = amount;
            self.invulnerable = 10;
            self.hurt_time = 10;
            if let Some(from) = source.knockback_origin() {
                self.knockback(from, 0.4);
            }
            amount
        };
        self.health -= applied;
        self.no_action_ticks = 0;
        self.just_hurt = true;
        let from = source.knockback_origin();
        if self.spec().hostile {
            if matches!(source, DamageSource::Player { .. }) {
                self.ai.target_player = true;
                self.ai.provoked = true;
                self.ai.path = None;
                self.ai.path_cooldown = 0;
            }
        } else {
            self.ai.panic_ticks = 60 + (self.age % 40);
            self.ai.panic_from = from;
            self.ai.path = None;
            self.ai.path_cooldown = 0;
        }
        true
    }

    /// Push away from `from` horizontally (and up when grounded).
    pub fn knockback(&mut self, from: Vec3, strength: f32) {
        let mut d = self.body.pos - from;
        d.y = 0.0;
        if d.length_squared() < 1e-6 {
            return;
        }
        let d = d.normalize();
        let v = &mut self.body.vel;
        v.x = v.x * 0.5 + d.x * strength;
        v.z = v.z * 0.5 + d.z * strength;
        if self.body.on_ground {
            v.y = (v.y * 0.5 + strength).min(0.4);
        }
    }

    /// Movement physics for one tick. Returns the fall distance on landing.
    pub(crate) fn travel(&mut self, world: &World, intent: &crate::ai::MoveIntent) -> Option<f32> {
        let b = &mut self.body;
        let dir = intent.dir;
        if b.in_water || b.in_lava {
            let drag = if b.in_water {
                physics::WATER_DRAG
            } else {
                physics::LAVA_DRAG
            };
            let scale = (intent.speed / 0.1).clamp(0.5, 1.5);
            b.vel += dir * physics::FLUID_ACCEL * scale;
            if intent.swim_up {
                b.vel.y += 0.04;
            }
            let vel = b.vel;
            physics::move_body(world, b, vel, false);
            b.vel *= drag;
            b.vel.y -= physics::FLUID_GRAVITY;
            if b.collided_h && (intent.swim_up || intent.jump) {
                let raised = b.aabb().translate(Vec3::new(b.vel.x, 0.6, b.vel.z));
                if !physics::collides(world, &raised) {
                    b.vel.y = 0.3;
                }
            }
            return None;
        }
        let slip = if b.on_ground {
            physics::slipperiness_below(world, b.pos)
        } else {
            1.0
        };
        let friction = slip * physics::AIR_FRICTION;
        let accel = if b.on_ground {
            physics::ground_accel(intent.speed, friction)
        } else {
            (intent.speed * 0.2).min(physics::AIR_ACCEL * 1.5)
        };
        b.vel += dir * accel;
        if intent.jump && b.on_ground {
            b.vel.y = physics::JUMP_VELOCITY;
        }
        let vel = b.vel;
        let out = physics::move_body(world, b, vel, false);
        let mut landed = out.landed_fall;
        if self.kind == MobKind::Spider && b.collided_h && dir.length_squared() > 0.01 {
            // Climb walls.
            b.vel.y = 0.2;
            b.fall_distance = 0.0;
        }
        b.vel.y = (b.vel.y - physics::GRAVITY) * physics::VERTICAL_DRAG;
        if self.kind == MobKind::Chicken && !b.on_ground && b.vel.y < 0.0 {
            b.vel.y *= 0.6;
            b.fall_distance = 0.0;
            landed = None;
        }
        b.vel.x *= friction;
        b.vel.z *= friction;
        landed
    }

    /// Sunlight, fire, lava, cactus and void.
    pub(crate) fn environment(&mut self, ctx: &mut MobCtx) {
        let world = &*ctx.world;
        if self.body.pos.y < (WORLD_MIN_Y - 64) as f32 {
            self.hurt(4.0, DamageSource::Void);
            self.invulnerable = 0;
        }
        if self.body.in_lava {
            self.hurt(4.0, DamageSource::Lava);
            self.fire_ticks = self.fire_ticks.max(300);
        }
        if self.body.in_water {
            self.fire_ticks = 0;
        }
        if self.spec().undead
            && self.age % 20 == 0
            && !self.body.in_water
            && world.daylight() > 0.65
            && sky_exposed(world, self.eye())
        {
            self.fire_ticks = self.fire_ticks.max(160);
        }
        if self.fire_ticks > 0 {
            self.fire_ticks -= 1;
            if self.fire_ticks % 20 == 0 {
                self.hurt(1.0, DamageSource::Fire);
            }
        }
        if physics::touches_block(world, &self.aabb(), 0.01, blocks::CACTUS) {
            self.hurt(1.0, DamageSource::Cactus);
        }
    }

    /// Start the death animation: sounds and loot.
    pub(crate) fn begin_death(&mut self, ctx: &mut MobCtx) {
        self.death_time = 1;
        self.fuse = 0;
        let spec = self.spec();
        if self.exploded {
            self.death_time = DEATH_TICKS;
            return;
        }
        ctx.events
            .push(EntityEvent::sound(spec.death_sound, self.body.pos));
        ctx.events.push(EntityEvent::MobDied {
            kind: self.kind,
            pos: self.body.pos,
        });
        let at = self.center();
        let burning = self.fire_ticks > 0;
        for d in spec.drops {
            let n = ctx.rng.range(d.min as i32, d.max as i32);
            let name = if burning {
                d.cooked.unwrap_or(d.item)
            } else {
                d.item
            };
            if n > 0
                && let Some(item) = ItemId::by_name(name)
            {
                ctx.drops.push((ItemStack::new(item, n as u8), at));
            }
        }
        if self.kind == MobKind::Sheep
            && !self.sheared
            && let Some(item) = ItemId::by_name("white_wool")
        {
            ctx.drops.push((ItemStack::new(item, 1), at));
        }
    }
}

/// Direct sky above `p` (nothing but air up to the top of the column).
pub fn sky_exposed(world: &World, p: Vec3) -> bool {
    let b = p.floor().as_ivec3();
    match world.height(b.x, b.z) {
        Some(h) => b.y > h,
        None => false,
    }
}

/// Block light / effective sky light at a position. Uses computed light when
/// the chunk has it, otherwise the heightmap (sky light 15 above the surface,
/// 0 below). Sky light is darkened by the time of day when `time_of_day` is set.
pub fn light_levels(world: &World, p: IVec3, time_of_day: bool) -> (u8, u8) {
    let cp = mc_core::ChunkPos::from_block(p.x, p.z);
    let (sky, block) = match world.chunk(cp) {
        Some(c) if c.light_ready => {
            let l = world.light(p);
            (l >> 4, l & 15)
        }
        Some(c) => {
            let h = c.height(p.x & 15, p.z & 15);
            (if p.y > h { 15 } else { 0 }, 0)
        }
        None => (15, 0),
    };
    if time_of_day {
        // daylight(): 1 at noon, 0.2 at midnight → darken sky by up to 11.
        let f = ((world.daylight() - 0.2) / 0.8).clamp(0.0, 1.0);
        let darken = ((1.0 - f) * 11.0).round() as u8;
        (sky.saturating_sub(darken), block)
    } else {
        (sky, block)
    }
}
