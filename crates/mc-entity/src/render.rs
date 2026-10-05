//! Render hand-off: mobs → `EntityRenderInstance` with procedural bone poses,
//! dropped items → `ItemEntityInstance`, falling blocks → `BlockModelInstance`.
//!
//! Conventions (shared with the model/renderer crates):
//! - Mob transform: `translate(pos) * rotate_y(-body_yaw)` (models face -Z),
//!   then the death tilt `rotate_z(0..90°)`.
//! - Bone rotations are degrees in the pack's Bedrock animation convention:
//!   +x pitches a bone forward/down (legs swing, head looks down), +y turns the
//!   head toward the entity's right (same sign as the yaw difference), and
//!   +z rolls (chicken wings use opposite signs per side, as the pack's
//!   animations do). Arms raised forward are x = -90.
//! - `ItemEntityInstance::transform` maps a unit cube/quad centred on the
//!   origin (−0.5..0.5) to the world: block items are scaled to 0.25, flat
//!   icons to 0.5.
//! - `BlockModelInstance::transform` (falling blocks) likewise maps a unit cube
//!   centred on the origin, so the block occupies its cell exactly.

use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

use glam::{Mat4, Vec3};
use mc_core::render_types::{
    BlockModelInstance, BonePose, EntityRenderInstance, FrameData, ItemEntityInstance, TextureKey,
};

use crate::EntityManager;
use crate::items::{FallingBlock, ItemEntity};
use crate::mob::{
    ARROW_MODEL, ARROW_TEXTURE, CREEPER_FUSE, DEATH_TICKS, Mob, MobKind, SHEEP_SHEARED_MODEL,
    wrap_angle,
};
use crate::projectile::Arrow;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + wrap_angle(b - a) * t
}

fn rot(bone: &str, x: f32, y: f32, z: f32) -> BonePose {
    BonePose::rot(bone, Vec3::new(x, y, z))
}

/// Walk swing in degrees for a leg (`phase` 0 or π for opposite legs).
fn leg_swing(limb: f32, amount: f32, phase: f32) -> f32 {
    ((limb * 0.6662 + phase).cos() * 1.4 * amount).to_degrees()
}

/// Interpolated animation inputs for one mob.
struct Anim {
    limb: f32,
    amount: f32,
    head_yaw: f32,
    head_pitch: f32,
    age: f32,
}

/// Bone poses for a mob (bone names from the pack geometry).
fn mob_poses(m: &Mob, a: &Anim, partial: f32) -> Vec<BonePose> {
    let mut poses = Vec::with_capacity(10);
    let head = rot("head", a.head_pitch, a.head_yaw, 0.0);
    match m.kind {
        MobKind::Pig | MobKind::Cow | MobKind::Sheep => {
            poses.push(head);
            let s = leg_swing(a.limb, a.amount, 0.0);
            poses.push(rot("leg0", s, 0.0, 0.0));
            poses.push(rot("leg1", -s, 0.0, 0.0));
            poses.push(rot("leg2", -s, 0.0, 0.0));
            poses.push(rot("leg3", s, 0.0, 0.0));
        }
        MobKind::Chicken => {
            poses.push(head);
            let s = leg_swing(a.limb, a.amount, 0.0);
            poses.push(rot("leg0", s, 0.0, 0.0));
            poses.push(rot("leg1", -s, 0.0, 0.0));
            let flap = lerp(m.prev_flap, m.flap, partial);
            let wing = ((a.age * 1.2).sin() + 1.0) * 0.5 * 75.0 * flap;
            poses.push(rot("wing0", 0.0, 0.0, wing));
            poses.push(rot("wing1", 0.0, 0.0, -wing));
        }
        MobKind::Zombie | MobKind::Skeleton => {
            poses.push(head);
            let s = leg_swing(a.limb, a.amount, 0.0);
            poses.push(rot("rightLeg", s, 0.0, 0.0));
            poses.push(rot("leftLeg", -s, 0.0, 0.0));
            let attack = if m.attack_anim > 0 {
                let t = 1.0 - (m.attack_anim as f32 - partial).max(0.0) / 10.0;
                (t * PI).sin()
            } else {
                0.0
            };
            let sway = (a.age * 0.067).sin() * 3.0;
            let aiming = m.kind == MobKind::Skeleton && m.ai.target_player && m.alive();
            if m.kind == MobKind::Zombie {
                // Arms held out forward, chopping down on attack.
                let x = -90.0 - attack * 40.0;
                poses.push(rot("rightArm", x + sway, -attack * 20.0, 3.0));
                poses.push(rot("leftArm", x - sway, attack * 20.0, -3.0));
            } else if aiming {
                // Bow drawn toward the target.
                let x = a.head_pitch - 90.0;
                poses.push(rot("rightArm", x + sway, a.head_yaw - 6.0, 3.0));
                poses.push(rot("leftArm", x - sway, a.head_yaw + 25.0, -3.0));
            } else {
                let arm = leg_swing(a.limb, a.amount * 0.5, PI);
                poses.push(rot("rightArm", arm, 0.0, 3.0 + sway));
                poses.push(rot("leftArm", -arm, 0.0, -3.0 - sway));
            }
        }
        MobKind::Creeper => {
            poses.push(head);
            let s = leg_swing(a.limb, a.amount, 0.0);
            poses.push(rot("leg0", s, 0.0, 0.0));
            poses.push(rot("leg1", -s, 0.0, 0.0));
            poses.push(rot("leg2", -s, 0.0, 0.0));
            poses.push(rot("leg3", s, 0.0, 0.0));
            let fuse = (lerp(m.prev_fuse as f32, m.fuse as f32, partial) / CREEPER_FUSE as f32)
                .clamp(0.0, 1.0);
            if fuse > 0.0 {
                // `body` is the root bone: scaling it swells the whole creeper.
                let wobble = 1.0 + (fuse * 100.0).sin() * fuse * 0.01;
                let scale = (1.0 + fuse.powi(4) * 0.3) * wobble;
                poses.push(BonePose {
                    bone: Arc::from("body"),
                    rotation: Vec3::ZERO,
                    offset: Vec3::ZERO,
                    scale,
                });
            }
        }
        MobKind::Spider => {
            poses.push(head);
            // Resting fan of eight legs, plus a scuttle while walking.
            const FAN_Y: [f32; 4] = [45.0, 22.5, -22.5, -45.0];
            const SPLAY_Z: [f32; 4] = [45.0, 33.3, 33.3, 45.0];
            for pair in 0..4 {
                let ph = pair as f32 * FRAC_PI_2;
                let walk_y = ((a.limb * 1.3324 + ph).cos().abs() * 0.4 * a.amount).to_degrees();
                let walk_z = ((a.limb * 0.6662 + ph).sin().abs() * 0.4 * a.amount).to_degrees();
                let y = FAN_Y[pair];
                let z = SPLAY_Z[pair];
                // Even legs are on the -x side, odd legs mirror them.
                poses.push(rot(
                    &format!("leg{}", pair * 2),
                    0.0,
                    y - walk_y,
                    -z + walk_z,
                ));
                poses.push(rot(
                    &format!("leg{}", pair * 2 + 1),
                    0.0,
                    -y + walk_y,
                    z - walk_z,
                ));
            }
        }
    }
    poses
}

fn mob_instance(m: &Mob, partial: f32) -> EntityRenderInstance {
    let spec = m.spec();
    let pos = m.prev_pos.lerp(m.body.pos, partial);
    let yaw = lerp_angle(m.prev_yaw, m.yaw, partial);
    let head_yaw = lerp_angle(m.prev_head_yaw, m.head_yaw, partial);
    let pitch = lerp(m.prev_pitch, m.pitch, partial);
    let anim = Anim {
        limb: lerp(m.prev_limb_swing, m.limb_swing, partial),
        amount: lerp(m.prev_limb_amount, m.limb_amount, partial).min(1.0),
        head_yaw: wrap_angle(head_yaw - yaw).to_degrees(),
        head_pitch: -pitch.to_degrees(),
        age: m.age as f32 + partial,
    };
    let mut transform = Mat4::from_translation(pos) * Mat4::from_rotation_y(-yaw);
    let mut hurt = (m.hurt_time as f32 - partial).max(0.0) / 10.0;
    if m.death_time > 0 {
        let t = ((m.death_time as f32 + partial - 1.0) / DEATH_TICKS as f32 * 1.6)
            .max(0.0)
            .sqrt()
            .min(1.0);
        transform *= Mat4::from_rotation_z(t * FRAC_PI_2);
        hurt = 1.0;
    }
    let model = if m.kind == MobKind::Sheep && m.sheared {
        SHEEP_SHEARED_MODEL
    } else {
        spec.model
    };
    EntityRenderInstance {
        model: Arc::from(model),
        texture: TextureKey::new(spec.texture),
        transform,
        poses: mob_poses(m, &anim, partial),
        tint: [1.0; 4],
        hurt,
        light: m.light,
    }
}

fn arrow_instance(a: &Arrow, partial: f32) -> EntityRenderInstance {
    let pos = a.prev_pos.lerp(a.pos, partial);
    let yaw = lerp_angle(a.prev_yaw, a.yaw, partial);
    let pitch = lerp(a.prev_pitch, a.pitch, partial);
    let shake = if a.shake > 0 {
        let s = a.shake as f32 - partial;
        -(s * 3.0).sin() * s * 0.02
    } else {
        0.0
    };
    EntityRenderInstance {
        model: Arc::from(ARROW_MODEL),
        texture: TextureKey::new(ARROW_TEXTURE),
        transform: Mat4::from_translation(pos)
            * Mat4::from_rotation_y(-yaw)
            * Mat4::from_rotation_x(pitch + shake),
        poses: Vec::new(),
        tint: [1.0; 4],
        hurt: 0.0,
        light: a.light,
    }
}

fn item_instances(it: &ItemEntity, partial: f32, out: &mut Vec<ItemEntityInstance>) {
    let pos = it.prev_pos.lerp(it.pos, partial);
    let t = it.age as f32 + partial;
    let bob = ((t / 10.0) + it.phase).sin() * 0.1 + 0.1;
    let spin = t / 20.0 + it.phase;
    let scale = if it.stack.item.block().is_some() {
        0.25
    } else {
        0.5
    };
    let copies = match it.stack.count {
        0..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    };
    let base =
        Mat4::from_translation(pos + Vec3::Y * (scale * 0.5 + bob)) * Mat4::from_rotation_y(spin);
    for k in 0..copies {
        let offset = if k == 0 {
            Vec3::ZERO
        } else {
            let a = it.phase * 7.0 + k as f32 * 2.3;
            Vec3::new(a.sin() * 0.06, k as f32 * 0.03, a.cos() * 0.06)
        };
        out.push(ItemEntityInstance {
            item: it.stack.item,
            transform: base * Mat4::from_translation(offset) * Mat4::from_scale(Vec3::splat(scale)),
            light: it.light,
        });
    }
}

fn falling_instance(f: &FallingBlock, partial: f32) -> BlockModelInstance {
    let pos = f.prev_pos.lerp(f.pos, partial);
    BlockModelInstance {
        block: f.block,
        transform: Mat4::from_translation(pos + Vec3::Y * 0.5),
        light: f.light,
    }
}

impl EntityManager {
    /// Add this frame's mobs and arrows (`frame.entities`), dropped items
    /// (`frame.items`) and falling blocks (`frame.block_models`).
    pub fn fill_frame(&self, partial: f32, frame: &mut FrameData) {
        let partial = partial.clamp(0.0, 1.0);
        for m in &self.mobs {
            if !m.removed() {
                frame.entities.push(mob_instance(m, partial));
            }
        }
        for a in &self.arrows {
            frame.entities.push(arrow_instance(a, partial));
        }
        for it in &self.items {
            item_instances(it, partial, &mut frame.items);
        }
        for f in &self.falling {
            frame.block_models.push(falling_instance(f, partial));
        }
    }
}
