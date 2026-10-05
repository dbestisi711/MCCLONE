//! Mob behaviour: a small priority list of goals evaluated every tick.
//!
//! Passive mobs: swim → panic (when hurt) → wander → look at player / idle look.
//! Hostile mobs: swim → attack the player (acquired within 16 blocks with line
//! of sight) → wander → look at player. Zombies and spiders melee (spiders
//! leap and climb, and are neutral in daylight unless provoked), skeletons keep
//! their distance, strafe and shoot arrows, creepers approach and ignite.
//! Paths come from [`crate::path`] and are recomputed at most every 10–20
//! ticks per mob.

use glam::{IVec3, Vec2, Vec3};
use mc_core::{ItemId, ItemStack, blocks};

use crate::mob::{
    CREEPER_FUSE, CREEPER_POWER, Mob, MobKind, approach_angle, pitch_of, wrap_angle, yaw_of,
};
use crate::path::{self, Path, PathConfig};
use crate::physics;
use crate::player::DamageSource;
use crate::projectile::Arrow;
use crate::{EntityEvent, MobCtx};

/// Hostiles notice the player within this distance (with line of sight).
pub const FOLLOW_RANGE: f32 = 16.0;
/// Hostiles give up beyond this distance.
pub const FORGET_RANGE: f32 = 24.0;
/// Panic speed multiplier for passive mobs.
pub const PANIC_SPEED: f32 = 2.0;

/// What the AI wants the body to do this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveIntent {
    /// Horizontal direction, length ≤ 1.
    pub dir: Vec3,
    /// Steady speed, blocks/tick.
    pub speed: f32,
    pub jump: bool,
    /// Float upward in fluids.
    pub swim_up: bool,
    /// Body yaw to turn toward (otherwise the body follows `dir`).
    pub face: Option<f32>,
}

/// Per-mob AI memory.
#[derive(Clone, Debug, Default)]
pub struct AiState {
    pub target_player: bool,
    /// Attacked by the player (spiders stay hostile in daylight).
    pub provoked: bool,
    pub sees_target: bool,
    pub lost_sight: u32,
    pub panic_ticks: u32,
    pub panic_from: Option<Vec3>,
    pub path: Option<Path>,
    pub path_index: usize,
    pub path_cooldown: u32,
    pub path_speed: f32,
    /// Goal the current chase path was computed for.
    pub path_goal: Option<IVec3>,
    pub wander_cooldown: u32,
    pub look_target: Option<Vec3>,
    pub look_ticks: u32,
    pub idle_yaw: Option<f32>,
    pub attack_cooldown: u32,
    pub strafe: f32,
    pub strafe_timer: u32,
    pub shoot_cooldown: u32,
    pub aim_ticks: u32,
    pub stuck_ticks: u32,
    pub last_node_dist: f32,
}

fn feet_block(pos: Vec3) -> IVec3 {
    Vec3::new(pos.x, pos.y + 0.1, pos.z).floor().as_ivec3()
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

impl Mob {
    fn path_config(&self, max_nodes: usize, avoid_water: bool, tolerance: i32) -> PathConfig {
        PathConfig {
            height: self.body.height.ceil().max(1.0) as i32,
            max_drop: 3,
            max_nodes,
            avoid_water,
            tolerance,
        }
    }

    pub(crate) fn set_path(&mut self, path: Option<Path>, speed: f32) {
        self.ai.path = path.filter(|p| !p.nodes.is_empty());
        self.ai.path_index = 0;
        self.ai.path_speed = speed;
        self.ai.stuck_ticks = 0;
        self.ai.last_node_dist = f32::INFINITY;
    }

    /// Steer along the current path. Returns false when there is none (left).
    fn follow_path(&mut self, intent: &mut MoveIntent) -> bool {
        let pos = self.body.pos;
        let reach = (self.body.width * 0.5).max(0.35);
        let Some(path) = &self.ai.path else {
            return false;
        };
        let mut idx = self.ai.path_index;
        while idx < path.nodes.len() {
            let n = path.nodes[idx];
            let c = Vec2::new(n.x as f32 + 0.5, n.z as f32 + 0.5);
            let d = (c - Vec2::new(pos.x, pos.z)).length();
            if d < reach && (pos.y - n.y as f32).abs() < 1.2 {
                idx += 1;
                self.ai.last_node_dist = f32::INFINITY;
            } else {
                break;
            }
        }
        if idx >= path.nodes.len() {
            self.ai.path = None;
            return false;
        }
        let n = path.nodes[idx];
        self.ai.path_index = idx;
        let target = Vec3::new(n.x as f32 + 0.5, n.y as f32, n.z as f32 + 0.5);
        let to = flat(target - pos);
        let d = to.length();
        intent.dir = if d > 1e-3 { to / d } else { Vec3::ZERO };
        intent.speed = self.ai.path_speed;
        if (n.y as f32 > pos.y + 0.5 && d < 1.6) || self.body.collided_h {
            intent.jump = true;
        }
        if d >= self.ai.last_node_dist - 0.005 {
            self.ai.stuck_ticks += 1;
        } else {
            self.ai.stuck_ticks = 0;
        }
        self.ai.last_node_dist = self.ai.last_node_dist.min(d);
        if self.ai.stuck_ticks > 60 {
            self.set_path(None, 0.0);
            self.ai.path_cooldown = 20;
            return false;
        }
        true
    }

    /// One AI tick: decide movement, attack, shoot, explode.
    pub(crate) fn think(&mut self, ctx: &mut MobCtx) -> MoveIntent {
        let ai = &mut self.ai;
        ai.path_cooldown = ai.path_cooldown.saturating_sub(1);
        ai.wander_cooldown = ai.wander_cooldown.saturating_sub(1);
        ai.attack_cooldown = ai.attack_cooldown.saturating_sub(1);
        ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
        ai.look_ticks = ai.look_ticks.saturating_sub(1);
        ai.panic_ticks = ai.panic_ticks.saturating_sub(1);
        self.attack_anim = self.attack_anim.saturating_sub(1);
        self.graze_ticks = self.graze_ticks.saturating_sub(1);

        let mut intent = MoveIntent::default();
        let eye_fluid = physics::fluid_at_point(ctx.world, self.eye()).is_some();
        if self.body.in_fluid() && (self.body.submerged > 0.4 || eye_fluid) {
            intent.swim_up = true;
        }

        let spec = self.spec();
        if !spec.ambient_sound.is_empty() && ctx.rng.one_in(480) {
            ctx.events
                .push(EntityEvent::sound(spec.ambient_sound, self.body.pos));
        }

        if spec.hostile {
            self.hostile_ai(ctx, &mut intent);
        } else {
            self.passive_ai(ctx, &mut intent);
        }

        // Head look target (players nearby, or idle glances).
        if self.ai.look_ticks == 0 {
            self.ai.look_target = None;
        }
        intent
    }

    fn look_behaviour(&mut self, ctx: &mut MobCtx) {
        let player_eye = ctx.player.eye_position(1.0);
        let dist = (player_eye - self.eye()).length();
        if self.ai.look_ticks > 0 && self.ai.look_target.is_some() {
            if ctx.player.dead {
                self.ai.look_ticks = 0;
            } else if self.ai.idle_yaw.is_none() {
                self.ai.look_target = Some(player_eye);
            }
            return;
        }
        if dist < 8.0 && !ctx.player.dead && ctx.rng.one_in(50) {
            self.ai.look_ticks = 40 + ctx.rng.below(40);
            self.ai.look_target = Some(player_eye);
            self.ai.idle_yaw = None;
        } else if ctx.rng.one_in(100) {
            let yaw = self.yaw + ctx.rng.signed() * 1.2;
            let (s, c) = yaw.sin_cos();
            self.ai.idle_yaw = Some(yaw);
            self.ai.look_ticks = 20 + ctx.rng.below(20);
            self.ai.look_target = Some(self.eye() + Vec3::new(s, 0.0, -c) * 4.0);
        }
    }

    fn passive_ai(&mut self, ctx: &mut MobCtx, intent: &mut MoveIntent) {
        let spec = self.spec();
        if self.kind == MobKind::Chicken {
            self.egg_timer = self.egg_timer.saturating_sub(1);
            if self.egg_timer == 0 {
                self.egg_timer = 6000 + ctx.rng.below(6000);
                if let Some(egg) = ItemId::by_name("egg") {
                    let at = self.body.pos + Vec3::Y * 0.3;
                    ctx.drops.push((ItemStack::new(egg, 1), at));
                    ctx.events
                        .push(EntityEvent::sound("mob.chicken.plop", self.body.pos));
                }
            }
        }
        if self.kind == MobKind::Sheep {
            self.graze(ctx);
            if self.graze_ticks > 0 {
                return;
            }
        }

        if self.ai.panic_ticks > 0 {
            let need = self.ai.path.is_none() || self.ai.path_cooldown == 0;
            if need && ctx.take_path_budget() {
                let target = self.random_target(ctx, 5, 3, self.ai.panic_from, false);
                if let Some(t) = target {
                    let cfg = self.path_config(120, false, 0);
                    let p = path::find_path(ctx.world, feet_block(self.body.pos), t, &cfg);
                    self.set_path(p, spec.walk_speed * PANIC_SPEED);
                }
                self.ai.path_cooldown = 20;
            }
            self.follow_path(intent);
            self.ai.look_ticks = 0;
            return;
        }

        if !self.follow_path(intent)
            && self.ai.wander_cooldown == 0
            && ctx.rng.one_in(120)
            && ctx.take_path_budget()
        {
            if let Some(t) = self.random_target(ctx, 10, 7, None, true) {
                let cfg = self.path_config(200, true, 0);
                let p = path::find_path(ctx.world, feet_block(self.body.pos), t, &cfg);
                self.set_path(p, spec.walk_speed);
            }
            self.ai.wander_cooldown = 40;
        }
        self.look_behaviour(ctx);
    }

    /// Sheep regrow wool by eating grass.
    fn graze(&mut self, ctx: &mut MobCtx) {
        if self.graze_ticks == 4 {
            let below = feet_block(self.body.pos) - IVec3::Y;
            let at = feet_block(self.body.pos);
            if ctx.world.block(at) == blocks::SHORT_GRASS {
                ctx.world.set_block(at, blocks::AIR);
                self.sheared = false;
            } else if ctx.world.block(below) == blocks::GRASS_BLOCK {
                ctx.world.set_block(below, blocks::DIRT);
                self.sheared = false;
            }
        }
        if self.graze_ticks == 0
            && self.body.on_ground
            && self.ai.path.is_none()
            && ctx.rng.one_in(if self.sheared { 300 } else { 1000 })
        {
            let below = feet_block(self.body.pos) - IVec3::Y;
            if ctx.world.block(below) == blocks::GRASS_BLOCK {
                self.graze_ticks = 40;
            }
        }
    }

    /// Pick a random standable spot within `xz`/`y` blocks; biased away from
    /// `away` if given; `prefer_land` scores grass and light higher.
    fn random_target(
        &self,
        ctx: &mut MobCtx,
        xz: i32,
        y: i32,
        away: Option<Vec3>,
        prefer_land: bool,
    ) -> Option<IVec3> {
        let start = feet_block(self.body.pos);
        let cfg = self.path_config(0, prefer_land, 0);
        let mut best: Option<(f32, IVec3)> = None;
        for _ in 0..10 {
            let mut off = IVec3::new(
                ctx.rng.range(-xz, xz),
                ctx.rng.range(-y, y),
                ctx.rng.range(-xz, xz),
            );
            if let Some(from) = away {
                let flee = flat(self.body.pos - from);
                if flee.length_squared() > 1e-4 {
                    let f = flee.normalize();
                    if f.x * off.x as f32 + f.z * off.z as f32 <= 0.0 {
                        off.x = -off.x;
                        off.z = -off.z;
                    }
                }
            }
            let mut p = start + off;
            // Settle onto the ground below the candidate.
            let mut ok = None;
            for _ in 0..(y * 2 + 1) {
                if let Some(c) = path::standable_cost(ctx.world, p, &cfg) {
                    ok = Some(c);
                    break;
                }
                p.y -= 1;
            }
            let Some(cost) = ok else { continue };
            let mut score = -cost;
            if prefer_land {
                if ctx.world.block(p - IVec3::Y) == blocks::GRASS_BLOCK {
                    score += 10.0;
                }
                let (sky, block) = crate::mob::light_levels(ctx.world, p, false);
                score += sky.max(block) as f32 * 0.5;
            }
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, p));
            }
        }
        best.map(|(_, p)| p)
    }

    fn hostile_ai(&mut self, ctx: &mut MobCtx, intent: &mut MoveIntent) {
        let spec = self.spec();
        let player = &*ctx.player;
        let target_center = player.position + Vec3::Y * (player.height() * 0.5);
        let dist = (target_center - self.center()).length();
        let bright = {
            let (sky, block) = crate::mob::light_levels(ctx.world, feet_block(self.body.pos), true);
            sky.max(block) >= 12
        };
        let neutral = self.kind == MobKind::Spider && !self.ai.provoked && bright;
        let staggered = (self.age as u64 + self.id).is_multiple_of(10);

        if !self.ai.target_player {
            if player.targetable()
                && dist < FOLLOW_RANGE
                && !neutral
                && staggered
                && physics::line_of_sight(ctx.world, self.eye(), player.eye_position(1.0))
            {
                self.ai.target_player = true;
                self.ai.sees_target = true;
                self.ai.lost_sight = 0;
                self.set_path(None, 0.0);
                self.ai.path_cooldown = 0;
            }
        } else {
            if (self.age as u64 + self.id).is_multiple_of(5) {
                self.ai.sees_target =
                    physics::line_of_sight(ctx.world, self.eye(), player.eye_position(1.0));
                if self.ai.sees_target {
                    self.ai.lost_sight = 0;
                } else {
                    self.ai.lost_sight += 5;
                }
            }
            let calm = neutral && ctx.rng.one_in(100);
            if !player.targetable() || dist > FORGET_RANGE || self.ai.lost_sight > 100 || calm {
                self.ai.target_player = false;
                self.ai.sees_target = false;
                self.set_path(None, 0.0);
            }
        }

        if !self.ai.target_player {
            if self.kind == MobKind::Creeper {
                self.fuse = (self.fuse - 1).max(0);
            }
            if !self.follow_path(intent)
                && self.ai.wander_cooldown == 0
                && ctx.rng.one_in(120)
                && ctx.take_path_budget()
            {
                if let Some(t) = self.random_target(ctx, 10, 7, None, false) {
                    let cfg = self.path_config(200, true, 0);
                    let p = path::find_path(ctx.world, feet_block(self.body.pos), t, &cfg);
                    self.set_path(p, spec.walk_speed);
                }
                self.ai.wander_cooldown = 60;
            }
            self.look_behaviour(ctx);
            return;
        }

        self.ai.look_target = Some(ctx.player.eye_position(1.0));
        self.ai.look_ticks = 2;
        self.ai.idle_yaw = None;
        match self.kind {
            MobKind::Skeleton => self.skeleton_ai(ctx, intent, dist),
            MobKind::Creeper => self.creeper_ai(ctx, intent, dist),
            _ => {
                self.chase(ctx, intent, spec.chase_speed, dist);
                self.melee(ctx);
                if self.kind == MobKind::Spider
                    && self.body.on_ground
                    && (2.0..4.0).contains(&dist)
                    && self.ai.sees_target
                    && ctx.rng.one_in(10)
                {
                    let d = flat(ctx.player.position - self.body.pos).normalize_or_zero();
                    self.body.vel.x += d.x * 0.4;
                    self.body.vel.z += d.z * 0.4;
                    self.body.vel.y = 0.4;
                }
            }
        }
    }

    /// Ground block under the player to path toward.
    fn player_goal(ctx: &MobCtx, cfg: &PathConfig) -> IVec3 {
        let mut g = feet_block(ctx.player.position);
        for _ in 0..4 {
            if path::standable_cost(ctx.world, g, cfg).is_some() {
                return g;
            }
            g.y -= 1;
        }
        feet_block(ctx.player.position)
    }

    /// Move toward the player: straight when close and visible, else by path.
    fn chase(&mut self, ctx: &mut MobCtx, intent: &mut MoveIntent, speed: f32, dist: f32) {
        let to = flat(ctx.player.position - self.body.pos);
        let dy = ctx.player.position.y - self.body.pos.y;
        if dist < 3.0 && self.ai.sees_target && dy.abs() < 1.5 {
            intent.dir = to.normalize_or_zero();
            intent.speed = speed;
            intent.jump = self.body.collided_h;
            return;
        }
        let cfg = self.path_config(500, false, 1);
        let goal = Self::player_goal(ctx, &cfg);
        // Recompute when there is no path or the target moved; an
        // unreachable target is not searched again until it moves.
        let stale = self.ai.path.is_none()
            || self
                .ai
                .path_goal
                .is_none_or(|g| (g - goal).abs().max_element() > 1);
        if stale && self.ai.path_cooldown == 0 && ctx.take_path_budget() {
            let p = path::find_path(ctx.world, feet_block(self.body.pos), goal, &cfg);
            let ok = p.is_some();
            self.set_path(p, speed);
            self.ai.path_goal = Some(goal);
            self.ai.path_cooldown = if ok {
                10 + ctx.rng.below(10)
            } else {
                40 + ctx.rng.below(20)
            };
        }
        // Path exhausted (or partial): head straight for the remembered
        // target. Spiders climb whatever is in the way.
        if !self.follow_path(intent) {
            intent.dir = to.normalize_or_zero();
            intent.speed = speed;
            intent.jump = self.body.collided_h && self.kind != MobKind::Spider;
        }
    }

    fn melee(&mut self, ctx: &mut MobCtx) {
        if self.ai.attack_cooldown > 0 {
            return;
        }
        let p = &*ctx.player;
        let d = flat(p.position - self.body.pos).length();
        let reach = ((self.body.width * 2.0).powi(2) + crate::player::PLAYER_WIDTH).sqrt();
        let a = self.aabb();
        let b = p.aabb();
        let vertical = a.min.y < b.max.y + 0.5 && a.max.y > b.min.y - 0.5;
        if d <= reach && vertical && p.targetable() {
            let dmg = self.spec().attack_damage;
            ctx.player.damage(
                dmg,
                DamageSource::Mob {
                    attacker: self.body.pos,
                },
            );
            self.ai.attack_cooldown = 20;
            self.attack_anim = 10;
        }
    }

    fn skeleton_ai(&mut self, ctx: &mut MobCtx, intent: &mut MoveIntent, dist: f32) {
        let spec = self.spec();
        if dist > 15.0 || !self.ai.sees_target {
            self.ai.aim_ticks = 0;
            self.chase(ctx, intent, spec.chase_speed, dist);
            return;
        }
        self.set_path(None, 0.0);
        let to = flat(ctx.player.position - self.body.pos);
        let fwd = to.normalize_or_zero();
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        self.ai.strafe_timer += 1;
        if self.ai.strafe == 0.0 || (self.ai.strafe_timer >= 20 && ctx.rng.chance(0.3)) {
            self.ai.strafe = if ctx.rng.chance(0.5) { 1.0 } else { -1.0 };
            self.ai.strafe_timer = 0;
        }
        if self.body.collided_h {
            self.ai.strafe = -self.ai.strafe;
        }
        let mut dir = right * self.ai.strafe * 0.5;
        if dist > 12.0 {
            dir += fwd * 0.7;
        } else if dist < 6.0 {
            dir -= fwd;
        }
        // Do not back off a cliff.
        let probe = self.aabb().translate(dir.normalize_or_zero() * 0.8);
        if !physics::collides(ctx.world, &probe.translate(Vec3::NEG_Y * 3.0)) {
            dir = Vec3::ZERO;
        }
        intent.dir = dir.normalize_or_zero();
        intent.speed = spec.chase_speed * 0.6;
        intent.face = Some(yaw_of(to));
        self.ai.aim_ticks += 1;
        if self.ai.shoot_cooldown == 0 && self.ai.aim_ticks >= 20 {
            self.shoot_arrow(ctx);
            self.ai.shoot_cooldown = 30 + ctx.rng.below(20);
            self.ai.aim_ticks = 0;
        }
    }

    fn shoot_arrow(&mut self, ctx: &mut MobCtx) {
        let from = self.eye() - Vec3::Y * 0.1;
        let p = &*ctx.player;
        let target = p.position + Vec3::Y * (p.height() / 3.0);
        let mut d = target - from;
        let horiz = Vec2::new(d.x, d.z).length();
        // Lob a little to compensate for gravity.
        d.y += horiz * 0.2;
        let mut dir = d.normalize_or_zero();
        dir += Vec3::new(ctx.rng.gaussian(), ctx.rng.gaussian(), ctx.rng.gaussian()) * 0.03;
        let vel = dir.normalize_or_zero() * 1.6;
        ctx.arrows.push(Arrow::new(from, vel, Some(self.id), 2.0));
        ctx.events.push(EntityEvent::sound("random.bow", from));
        self.attack_anim = 10;
    }

    fn creeper_ai(&mut self, ctx: &mut MobCtx, intent: &mut MoveIntent, dist: f32) {
        let fusing = self.fuse > 0 || (dist < 3.0 && self.ai.sees_target);
        if !fusing {
            self.chase(ctx, intent, self.spec().chase_speed, dist);
            return;
        }
        if dist > 7.0 || !self.ai.sees_target {
            // Target got away: defuse while following it again.
            self.fuse = (self.fuse - 1).max(0);
            self.chase(ctx, intent, self.spec().chase_speed, dist);
            return;
        } else {
            if self.fuse == 0 {
                ctx.events
                    .push(EntityEvent::sound("random.fuse", self.body.pos));
            }
            self.fuse += 1;
        }
        self.set_path(None, 0.0);
        intent.face = Some(yaw_of(flat(ctx.player.position - self.body.pos)));
        if self.fuse >= CREEPER_FUSE {
            ctx.explosions.push((self.center(), CREEPER_POWER));
            self.exploded = true;
            self.health = 0.0;
        }
    }

    /// Turn the body toward the movement/facing direction and the head toward
    /// the look target (head stays within ±75° of the body).
    pub(crate) fn update_rotation(&mut self, intent: &MoveIntent) {
        let max_head = 75f32.to_radians();
        if let Some(face) = intent.face {
            self.yaw = approach_angle(self.yaw, face, 30f32.to_radians());
        } else if intent.dir.length_squared() > 1e-4 && intent.speed > 0.0 {
            self.yaw = approach_angle(self.yaw, yaw_of(intent.dir), 20f32.to_radians());
        }
        let (target_yaw, target_pitch) = match self.ai.look_target {
            Some(t) => {
                let d = t - self.eye();
                (yaw_of(d), pitch_of(d))
            }
            None => (self.yaw, 0.0),
        };
        self.head_yaw = approach_angle(self.head_yaw, target_yaw, 40f32.to_radians());
        let rel = wrap_angle(self.head_yaw - self.yaw);
        if rel.abs() > max_head {
            self.yaw = self.head_yaw - rel.signum() * max_head;
        }
        let target_pitch = target_pitch.clamp(-1.2, 1.2);
        let step = 20f32.to_radians();
        self.pitch += (target_pitch - self.pitch).clamp(-step, step);
        if self.kind == MobKind::Sheep && self.graze_ticks > 0 {
            self.pitch = -0.6;
        }
    }
}
