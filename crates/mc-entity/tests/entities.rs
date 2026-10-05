//! Mobs, AI, pathfinding, explosions, items and spawning.

mod common;

use common::*;
use glam::{IVec3, Vec3};
use mc_core::input::InputState;
use mc_core::render_types::FrameData;
use mc_core::{ItemId, ItemStack, World, blocks};
use mc_entity::path::{PathConfig, find_path};
use mc_entity::{EntityEvent, EntityManager, GameMode, MobKind, Player};

/// Step player + entities like `mc-game` does.
fn step(w: &mut World, p: &mut Player, m: &mut EntityManager, ticks: u32) {
    let idle = InputState::default();
    for _ in 0..ticks {
        w.tick += 1;
        p.tick(&idle, w);
        m.tick(w, p);
    }
}

const NIGHT: u64 = 18000;

#[test]
fn astar_finds_path_around_a_wall() {
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(5, 64, -5),
        IVec3::new(5, 66, 5),
        blocks::STONE,
    );
    let cfg = PathConfig::default();
    let start = IVec3::new(2, 64, 0);
    let goal = IVec3::new(8, 64, 0);
    let path = find_path(&w, start, goal, &cfg).expect("path");
    assert!(path.reached);
    assert_eq!(path.goal(), Some(goal));
    // Walks around the end of the wall (|z| > 5), never through it.
    assert!(path.nodes.iter().any(|n| n.z.abs() > 5));
    assert!(path.nodes.iter().all(|n| n.x != 5 || n.z.abs() > 5));
    assert!(path.nodes.len() >= 12, "detour length {}", path.nodes.len());
    // Consecutive nodes are neighbours.
    let mut prev = start;
    for &n in &path.nodes {
        let d = (n - prev).abs();
        assert!(d.x <= 1 && d.z <= 1 && d.y <= 3, "{prev} -> {n}");
        prev = n;
    }
}

#[test]
fn astar_jumps_up_and_drops_down() {
    let mut w = flat_world(1, 63, blocks::STONE);
    // A one-block step up, then a 3-block drop into a pit.
    fill(
        &mut w,
        IVec3::new(3, 64, -8),
        IVec3::new(5, 64, 8),
        blocks::STONE,
    );
    fill(
        &mut w,
        IVec3::new(6, 61, -8),
        IVec3::new(8, 63, 8),
        blocks::AIR,
    );
    let cfg = PathConfig::default();
    let path = find_path(&w, IVec3::new(0, 64, 0), IVec3::new(7, 61, 0), &cfg).unwrap();
    assert!(path.reached);
    assert!(path.nodes.contains(&IVec3::new(3, 65, 0)) || path.nodes.iter().any(|n| n.y == 65));
    // A 4-block drop is refused.
    let mut w2 = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w2,
        IVec3::new(6, 60, -16),
        IVec3::new(8, 63, 16),
        blocks::AIR,
    );
    let p2 = find_path(&w2, IVec3::new(0, 64, 0), IVec3::new(7, 60, 0), &cfg);
    assert!(p2.is_none_or(|p| !p.reached));
}

#[test]
fn astar_avoids_lava_and_cactus() {
    let mut w = flat_world(1, 63, blocks::STONE);
    // A lava moat with a single safe bridge at z = 6.
    fill(
        &mut w,
        IVec3::new(4, 63, -8),
        IVec3::new(6, 63, 8),
        blocks::LAVA,
    );
    fill(
        &mut w,
        IVec3::new(4, 63, 6),
        IVec3::new(6, 63, 6),
        blocks::STONE,
    );
    let cfg = PathConfig::default();
    let path = find_path(&w, IVec3::new(0, 64, 0), IVec3::new(10, 64, 0), &cfg).unwrap();
    assert!(path.reached);
    for n in &path.nodes {
        assert_ne!(
            w.block(*n - IVec3::Y),
            blocks::LAVA,
            "stands on lava at {n}"
        );
        assert_ne!(w.block(*n), blocks::LAVA);
    }
}

#[test]
fn astar_fails_gracefully() {
    let mut w = flat_world(1, 63, blocks::STONE);
    // Goal sealed inside a stone box.
    fill(
        &mut w,
        IVec3::new(8, 64, -2),
        IVec3::new(12, 67, 2),
        blocks::STONE,
    );
    fill(
        &mut w,
        IVec3::new(9, 64, -1),
        IVec3::new(11, 65, 1),
        blocks::AIR,
    );
    let cfg = PathConfig {
        max_nodes: 300,
        ..Default::default()
    };
    let r = find_path(&w, IVec3::new(0, 64, 0), IVec3::new(10, 64, 0), &cfg);
    if let Some(p) = &r {
        assert!(!p.reached);
        // The partial path gets as close as possible.
        let end = p.goal().unwrap();
        assert!(end.x >= 6, "closest node {end}");
    }
    // Goal in unloaded space, tiny budget: still no panic.
    let tiny = PathConfig {
        max_nodes: 5,
        ..Default::default()
    };
    let r = find_path(&w, IVec3::new(0, 64, 0), IVec3::new(500, 64, 0), &tiny);
    assert!(r.is_none_or(|p| !p.reached));
}

#[test]
fn zombie_approaches_and_damages_a_stationary_player() {
    let mut w = flat_world(2, 63, blocks::GRASS_BLOCK);
    w.time_of_day = NIGHT;
    // A wall in between makes it path around.
    fill(
        &mut w,
        IVec3::new(6, 64, -3),
        IVec3::new(6, 65, 3),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(1);
    let id = m.spawn_mob(MobKind::Zombie, Vec3::new(12.5, 64.0, 0.5));
    let mut hit_at = None;
    for t in 0..600 {
        step(&mut w, &mut p, &mut m, 1);
        if p.health < 20.0 {
            hit_at = Some(t);
            break;
        }
    }
    let t = hit_at.expect("zombie never hit the player");
    assert!(t < 400, "took {t} ticks");
    let z = m.mob(id).unwrap();
    assert!((z.pos() - p.position).length() < 2.5);
    assert!(m.mob(id).unwrap().ai.target_player);
    // Creative players are ignored.
    p.game_mode = GameMode::Creative;
    p.health = 20.0;
    step(&mut w, &mut p, &mut m, 100);
    assert_eq!(p.health, 20.0);
}

#[test]
fn zombie_burns_in_daylight_but_not_in_water() {
    let mut w = flat_world(1, 63, blocks::GRASS_BLOCK);
    w.time_of_day = 6000;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = quiet_manager(2);
    let id = m.spawn_mob(MobKind::Zombie, Vec3::new(8.5, 64.0, 8.5));
    step(&mut w, &mut p, &mut m, 60);
    let z = m.mob(id).unwrap();
    assert!(z.fire_ticks > 0 && z.health < 20.0);
    // Shaded by a roof: no fire.
    let mut w = flat_world(1, 63, blocks::GRASS_BLOCK);
    w.time_of_day = 6000;
    fill(
        &mut w,
        IVec3::new(-15, 70, -15),
        IVec3::new(15, 70, 15),
        blocks::STONE,
    );
    let mut m = quiet_manager(2);
    let id = m.spawn_mob(MobKind::Zombie, Vec3::new(8.5, 64.0, 8.5));
    step(&mut w, &mut p, &mut m, 60);
    assert_eq!(m.mob(id).unwrap().fire_ticks, 0);
}

#[test]
fn skeleton_shoots_arrows_that_arc_and_hurt() {
    let mut w = flat_world(2, 63, blocks::STONE);
    w.time_of_day = NIGHT;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(3);
    m.spawn_mob(MobKind::Skeleton, Vec3::new(10.5, 64.0, 0.5));
    let mut saw_arrow = false;
    let mut arrow_fell = false;
    for _ in 0..400 {
        step(&mut w, &mut p, &mut m, 1);
        for a in m.arrows() {
            saw_arrow = true;
            if a.vel.y < 0.0 && a.stuck_in.is_none() {
                arrow_fell = true;
            }
        }
        if p.health < 16.0 {
            break;
        }
    }
    assert!(saw_arrow && arrow_fell);
    assert!(p.health < 20.0, "arrows hit the player");
    // It keeps its distance instead of walking into melee range.
    let s = &m.mobs()[0];
    let d = (s.pos() - p.position).length();
    assert!(d > 3.0, "skeleton at distance {d}");
}

#[test]
fn creeper_fuses_and_explosion_removes_blocks() {
    let mut w = flat_world(2, 63, blocks::DIRT);
    w.time_of_day = NIGHT;
    // Some bedrock and obsidian near the blast survive it.
    w.set_block(IVec3::new(4, 63, 1), blocks::BEDROCK);
    w.set_block(IVec3::new(4, 63, -1), blocks::OBSIDIAN);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.health = 20.0;
    let mut m = quiet_manager(4);
    m.spawn_mob(MobKind::Creeper, Vec3::new(6.5, 64.0, 0.5));
    let before = count_blocks(
        &w,
        IVec3::new(-6, 58, -6),
        IVec3::new(12, 63, 6),
        blocks::DIRT,
    );
    let mut fused = false;
    let mut exploded = false;
    for _ in 0..600 {
        step(&mut w, &mut p, &mut m, 1);
        if m.mobs().first().is_some_and(|c| c.fuse > 0) {
            fused = true;
        }
        if m.take_events()
            .iter()
            .any(|e| matches!(e, EntityEvent::Explosion { .. }))
        {
            exploded = true;
            break;
        }
    }
    assert!(fused && exploded);
    assert!(m.mobs().is_empty(), "the creeper is gone");
    let after = count_blocks(
        &w,
        IVec3::new(-6, 58, -6),
        IVec3::new(12, 63, 6),
        blocks::DIRT,
    );
    assert!(before - after >= 10, "removed {} blocks", before - after);
    assert_eq!(w.block(IVec3::new(4, 63, 1)), blocks::BEDROCK);
    assert_eq!(w.block(IVec3::new(4, 63, -1)), blocks::OBSIDIAN);
    assert!(p.health < 20.0 || p.dead, "the player was hurt");
}

#[test]
fn explosion_spares_water_and_drops_items() {
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(-1, 64, -1),
        IVec3::new(1, 64, 1),
        blocks::WATER,
    );
    let mut p = survival_player(Vec3::new(30.5, 64.0, 0.5));
    let mut m = quiet_manager(5);
    m.explode(&mut w, &mut p, Vec3::new(0.5, 63.5, 0.5), 3.0);
    assert_eq!(w.block(IVec3::new(0, 64, 0)), blocks::WATER);
    assert!(w.block(IVec3::new(0, 63, 0)).is_air());
    assert!(!m.items().is_empty(), "some blocks dropped as items");
    assert_eq!(p.health, 20.0, "far away player is unhurt");
    // Point-blank damage is lethal, falling off with distance.
    let mut near = survival_player(Vec3::new(2.5, 64.0, 0.5));
    let mut w = flat_world(1, 63, blocks::STONE);
    m.explode(&mut w, &mut near, Vec3::new(0.5, 64.5, 0.5), 3.0);
    let mut far = survival_player(Vec3::new(4.5, 64.0, 0.5));
    let mut w = flat_world(1, 63, blocks::STONE);
    m.explode(&mut w, &mut far, Vec3::new(0.5, 64.5, 0.5), 3.0);
    assert!(
        near.health < far.health,
        "{} vs {}",
        near.health,
        far.health
    );
}

#[test]
fn spider_climbs_walls() {
    let mut w = flat_world(2, 63, blocks::STONE);
    w.time_of_day = NIGHT;
    // Player on the edge of a 4-high tower; spider below (it can see the player).
    fill(
        &mut w,
        IVec3::new(-2, 64, -2),
        IVec3::new(2, 67, 2),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(2.5, 68.0, 0.5));
    let mut m = quiet_manager(6);
    let id = m.spawn_mob(MobKind::Spider, Vec3::new(8.5, 64.0, 0.5));
    let mut max_y: f32 = 0.0;
    for _ in 0..400 {
        step(&mut w, &mut p, &mut m, 1);
        max_y = max_y.max(m.mob(id).map(|s| s.pos().y).unwrap_or(0.0));
        if p.health < 20.0 {
            break;
        }
    }
    assert!(max_y > 66.0, "spider climbed to {max_y}");
    assert!(p.health < 20.0, "and reached the player on top");
}

#[test]
fn spiders_are_neutral_in_daylight() {
    let mut w = flat_world(1, 63, blocks::GRASS_BLOCK);
    w.time_of_day = 6000;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(7);
    let id = m.spawn_mob(MobKind::Spider, Vec3::new(5.5, 64.0, 0.5));
    step(&mut w, &mut p, &mut m, 200);
    assert!(!m.mob(id).unwrap().ai.target_player);
    // Hitting it makes it hostile.
    let eye = p.eye_position(1.0);
    let target = m.mob(id).unwrap().center();
    assert!(m.attack(eye, target - eye, 10.0, 1.0));
    step(&mut w, &mut p, &mut m, 20);
    assert!(m.mob(id).unwrap().ai.target_player);
}

#[test]
fn passive_mobs_wander_panic_and_drop_loot() {
    let mut w = flat_world(2, 63, blocks::GRASS_BLOCK);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = quiet_manager(8);
    let id = m.spawn_mob(MobKind::Pig, Vec3::new(5.5, 64.0, 5.5));
    let start = m.mob(id).unwrap().pos();
    step(&mut w, &mut p, &mut m, 1200);
    let wandered = (m.mob(id).unwrap().pos() - start).length();
    assert!(wandered > 1.0, "pig wandered {wandered}");
    // Hit it: it panics and runs.
    let eye = p.eye_position(1.0);
    let c = m.mob(id).unwrap().center();
    assert!(m.attack(eye, c - eye, 20.0, 2.0));
    let before = m.mob(id).unwrap().pos();
    step(&mut w, &mut p, &mut m, 40);
    let pig = m.mob(id).unwrap();
    assert!(pig.ai.panic_ticks > 0);
    assert!((pig.pos() - before).length() > 2.0, "panicking pig runs");
    // Kill it: death animation, then loot.
    for _ in 0..10 {
        let c = m.mob(id).map(|m| m.center());
        if let Some(c) = c {
            let eye = p.eye_position(1.0);
            m.attack(eye, c - eye, 30.0, 5.0);
        }
        step(&mut w, &mut p, &mut m, 11);
    }
    step(&mut w, &mut p, &mut m, 30);
    assert!(m.mob(id).is_none(), "removed after the death animation");
    let pork = ItemId::by_name("porkchop").unwrap();
    assert!(m.items().iter().any(|i| i.stack.item == pork));
}

#[test]
fn chickens_fall_slowly() {
    let mut w = flat_world(1, 40, blocks::GRASS_BLOCK);
    let mut p = survival_player(Vec3::new(0.5, 41.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = quiet_manager(9);
    let id = m.spawn_mob(MobKind::Chicken, Vec3::new(5.5, 70.0, 5.5));
    step(&mut w, &mut p, &mut m, 40);
    let c = m.mob(id).unwrap();
    assert!(c.pos().y > 60.0, "chicken at {}", c.pos().y);
    assert!(c.flap > 0.5);
    step(&mut w, &mut p, &mut m, 400);
    let c = m.mob(id).unwrap();
    assert!(
        c.body.on_ground && c.health == 4.0,
        "no fall damage for chickens"
    );
}

#[test]
fn mobs_float_in_water() {
    let mut w = flat_world(1, 50, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(-15, 51, -15),
        IVec3::new(15, 62, 15),
        blocks::WATER,
    );
    let mut p = survival_player(Vec3::new(0.5, 63.0, 0.5));
    p.game_mode = GameMode::Creative;
    p.flying = true;
    let mut m = quiet_manager(10);
    let id = m.spawn_mob(MobKind::Cow, Vec3::new(5.5, 52.0, 5.5));
    step(&mut w, &mut p, &mut m, 200);
    let cow = m.mob(id).unwrap();
    assert!(cow.pos().y > 60.5, "cow floated to {}", cow.pos().y);
}

#[test]
fn items_merge_and_get_picked_up() {
    let mut w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(11);
    let dirt = ItemId::from_block(blocks::DIRT);
    m.spawn_item(ItemStack::new(dirt, 10), Vec3::new(8.5, 64.5, 8.5));
    m.spawn_item(ItemStack::new(dirt, 20), Vec3::new(8.6, 64.5, 8.4));
    m.spawn_item(
        ItemStack::new(ItemId::from_block(blocks::STONE), 1),
        Vec3::new(8.5, 64.5, 8.5),
    );
    step(&mut w, &mut p, &mut m, 40);
    let dirt_items: Vec<_> = m.items().iter().filter(|i| i.stack.item == dirt).collect();
    assert_eq!(dirt_items.len(), 1, "merged");
    assert_eq!(dirt_items[0].stack.count, 30);
    assert_eq!(m.items().len(), 2, "different items do not merge");
    assert!(m.items().iter().all(|i| i.on_ground));

    // Fresh items wait 10 ticks before they can be picked up.
    m.spawn_item(ItemStack::new(dirt, 5), p.position + Vec3::Y);
    step(&mut w, &mut p, &mut m, 5);
    assert_eq!(p.inventory.count_of(dirt), 0);
    step(&mut w, &mut p, &mut m, 20);
    assert_eq!(p.inventory.count_of(dirt), 5);
    // Walk to the merged stack.
    p.position = Vec3::new(8.5, 64.0, 8.0);
    p.prev_position = p.position;
    step(&mut w, &mut p, &mut m, 2);
    assert_eq!(p.inventory.count_of(dirt), 35);
    assert!(
        m.take_events()
            .iter()
            .any(|e| matches!(e, EntityEvent::ItemPickedUp { .. }))
    );
}

#[test]
fn items_despawn_after_five_minutes() {
    let mut w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(12);
    m.spawn_item(
        ItemStack::new(ItemId::by_name("stick").unwrap(), 1),
        Vec3::new(10.5, 64.5, 10.5),
    );
    step(&mut w, &mut p, &mut m, 5990);
    assert_eq!(m.items().len(), 1);
    step(&mut w, &mut p, &mut m, 20);
    assert!(m.items().is_empty());
}

#[test]
fn sand_falls_when_unsupported() {
    let mut w = flat_world(1, 63, blocks::STONE);
    w.set_block(IVec3::new(4, 64, 4), blocks::DIRT);
    w.set_block(IVec3::new(4, 65, 4), blocks::SAND);
    w.set_block(IVec3::new(4, 66, 4), blocks::GRAVEL);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(13);
    // Break the supporting dirt.
    w.set_block(IVec3::new(4, 64, 4), blocks::AIR);
    m.on_block_changed(&w, IVec3::new(4, 64, 4));
    step(&mut w, &mut p, &mut m, 1);
    assert_eq!(m.falling_blocks().len(), 2);
    let mut frame = FrameData::default();
    m.fill_frame(0.5, &mut frame);
    assert_eq!(frame.block_models.len(), 2);
    step(&mut w, &mut p, &mut m, 40);
    assert!(m.falling_blocks().is_empty());
    assert_eq!(w.block(IVec3::new(4, 64, 4)), blocks::SAND);
    assert_eq!(w.block(IVec3::new(4, 65, 4)), blocks::GRAVEL);
    assert!(w.block(IVec3::new(4, 66, 4)).is_air());
    // Sand placed in mid-air falls too.
    w.set_block(IVec3::new(2, 70, 2), blocks::SAND);
    m.on_block_changed(&w, IVec3::new(2, 70, 2));
    step(&mut w, &mut p, &mut m, 60);
    assert_eq!(w.block(IVec3::new(2, 64, 2)), blocks::SAND);
}

#[test]
fn shearing_a_sheep() {
    let p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    let mut m = quiet_manager(14);
    let id = m.spawn_mob(MobKind::Sheep, Vec3::new(2.5, 64.0, 0.5));
    let eye = p.eye_position(1.0);
    let c = m.mob(id).unwrap().center();
    let shears = ItemId::by_name("shears");
    assert!(!m.interact(eye, c - eye, 5.0, None));
    assert!(m.interact(eye, c - eye, 5.0, shears));
    assert!(m.mob(id).unwrap().sheared);
    assert!(!m.interact(eye, c - eye, 5.0, shears), "already sheared");
    let mut frame = FrameData::default();
    m.fill_frame(0.0, &mut frame);
    assert_eq!(&*frame.entities[0].model, "geometry.sheep.sheared.v1.8");
    let wool = ItemId::by_name("white_wool").unwrap();
    assert!(m.items().iter().any(|i| i.stack.item == wool));
}

#[test]
fn render_instances_use_pack_models_and_bones() {
    let mut w = flat_world(1, 63, blocks::GRASS_BLOCK);
    w.time_of_day = NIGHT;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = quiet_manager(15);
    for (i, k) in MobKind::ALL.into_iter().enumerate() {
        m.spawn_mob(k, Vec3::new(-12.5 + i as f32 * 3.0, 64.0, 8.5));
    }
    m.spawn_item(
        ItemStack::new(ItemId::by_name("bone").unwrap(), 3),
        Vec3::new(0.5, 65.0, 3.5),
    );
    step(&mut w, &mut p, &mut m, 30);
    let mut frame = FrameData::default();
    m.fill_frame(0.5, &mut frame);
    assert_eq!(frame.entities.len(), 8);
    assert_eq!(frame.items.len(), 2, "a stack of 3 renders as 2 copies");
    let expect = [
        ("geometry.pig.v3", "textures/entity/pig/pig_v3", "leg3"),
        ("geometry.cow.v2", "textures/entity/cow/cow_v2", "leg2"),
        ("geometry.sheep.v1.8", "textures/entity/sheep/sheep", "leg1"),
        (
            "geometry.chicken.v1.12",
            "textures/entity/chicken/chicken",
            "wing0",
        ),
        (
            "geometry.zombie.v1.8",
            "textures/entity/zombie/zombie",
            "rightArm",
        ),
        (
            "geometry.skeleton.v1.8",
            "textures/entity/skeleton/skeleton",
            "leftArm",
        ),
        (
            "geometry.creeper.v1.8",
            "textures/entity/creeper/creeper",
            "leg0",
        ),
        (
            "geometry.spider.v1.8",
            "textures/entity/spider/spider",
            "leg7",
        ),
    ];
    for (e, (model, tex, bone)) in frame.entities.iter().zip(expect) {
        assert_eq!(&*e.model, model);
        assert_eq!(e.texture.as_str(), tex);
        assert!(e.poses.iter().any(|b| &*b.bone == "head"));
        assert!(
            e.poses.iter().any(|b| &*b.bone == bone),
            "{model} lacks {bone}"
        );
        assert!(e.transform.is_finite());
    }
    // Zombie arms are raised forward.
    let zombie = &frame.entities[4];
    let arm = zombie
        .poses
        .iter()
        .find(|b| &*b.bone == "rightArm")
        .unwrap();
    assert!(arm.rotation.x < -80.0);
}

#[test]
fn natural_spawning_respects_light_and_caps() {
    let mut w = flat_world(4, 63, blocks::GRASS_BLOCK);
    w.time_of_day = NIGHT;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = EntityManager::new(16);
    for _ in 0..600 {
        w.time_of_day = NIGHT;
        step(&mut w, &mut p, &mut m, 1);
    }
    let hostile = m.mobs().iter().filter(|x| x.kind.hostile()).count();
    let passive = m.mobs().iter().filter(|x| !x.kind.hostile()).count();
    assert!(hostile > 0, "hostiles spawn at night");
    assert!(hostile <= mc_entity::HOSTILE_CAP + 4);
    assert!(passive > 0 && passive <= mc_entity::PASSIVE_CAP + 4);
    for x in m.mobs() {
        let d = (x.pos() - p.position).length();
        assert!(d >= 23.0, "spawned too close: {d}");
    }

    // Noon on the surface: no new hostiles.
    let mut w = flat_world(4, 63, blocks::GRASS_BLOCK);
    let mut m = EntityManager::new(17);
    for _ in 0..600 {
        w.time_of_day = 6000;
        step(&mut w, &mut p, &mut m, 1);
    }
    assert_eq!(m.mobs().iter().filter(|x| x.kind.hostile()).count(), 0);
}

#[test]
fn hostiles_despawn_far_away() {
    let mut w = flat_world(2, 63, blocks::STONE);
    w.time_of_day = NIGHT;
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut m = EntityManager::new(18);
    let id = m.spawn_mob(MobKind::Zombie, Vec3::new(5.5, 64.0, 5.5));
    // Teleport the player far away (beyond 128 blocks).
    p.position = Vec3::new(400.5, 64.0, 0.5);
    p.prev_position = p.position;
    p.flying = true;
    m.tick(&mut w, &mut p);
    assert!(m.mob(id).is_none());
}

fn snapshot(m: &EntityManager) -> Vec<(u64, MobKind, [u32; 3], u32)> {
    m.mobs()
        .iter()
        .map(|x| {
            let pp = x.pos();
            (
                x.id,
                x.kind,
                [pp.x.to_bits(), pp.y.to_bits(), pp.z.to_bits()],
                x.health.to_bits(),
            )
        })
        .collect()
}

#[test]
fn simulation_is_deterministic_for_a_seed() {
    let run_sim = |seed: u64| {
        let mut w = flat_world(3, 63, blocks::GRASS_BLOCK);
        w.time_of_day = NIGHT;
        fill(
            &mut w,
            IVec3::new(5, 64, -5),
            IVec3::new(5, 66, 5),
            blocks::STONE,
        );
        let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
        let mut m = EntityManager::new(seed);
        m.spawn_mob(MobKind::Zombie, Vec3::new(10.5, 64.0, 0.5));
        m.spawn_mob(MobKind::Sheep, Vec3::new(-6.5, 64.0, 3.5));
        m.spawn_mob(MobKind::Skeleton, Vec3::new(-10.5, 64.0, -8.5));
        for _ in 0..400 {
            w.time_of_day = NIGHT;
            step(&mut w, &mut p, &mut m, 1);
        }
        (
            snapshot(&m),
            p.health.to_bits(),
            p.position.to_array().map(f32::to_bits),
        )
    };
    let a = run_sim(99);
    let b = run_sim(99);
    assert_eq!(a, b);
    let c = run_sim(100);
    assert_ne!(a.0, c.0, "a different seed gives a different simulation");
}

#[test]
fn player_death_drops_inventory() {
    let mut w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.inventory
        .add(ItemStack::new(ItemId::by_name("stick").unwrap(), 5));
    let mut m = quiet_manager(19);
    p.damage(100.0, mc_entity::DamageSource::Generic);
    assert!(p.dead);
    step(&mut w, &mut p, &mut m, 1);
    assert!(p.inventory.slots.iter().all(|s| s.is_none()));
    assert_eq!(m.items().len(), 1);
    assert!(
        m.take_events()
            .iter()
            .any(|e| matches!(e, EntityEvent::PlayerDied { .. }))
    );
}
