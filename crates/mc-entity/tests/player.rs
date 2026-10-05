//! Player physics and survival mechanics, simulated tick by tick.

mod common;

use common::*;
use glam::{IVec3, Vec3};
use mc_core::input::{InputState, Key, MouseButton};
use mc_core::{ItemId, ItemStack, blocks};
use mc_entity::{DamageSource, EAT_TICKS, GameMode, Player};

fn run(p: &mut Player, w: &mc_core::World, input: &InputState, ticks: u32) {
    for _ in 0..ticks {
        p.tick(input, w);
    }
}

#[test]
fn falls_and_lands_on_ground() {
    let w = flat_world(1, 63, blocks::GRASS_BLOCK);
    let mut p = survival_player(Vec3::new(0.5, 66.0, 0.5));
    run(&mut p, &w, &InputState::default(), 60);
    assert!(p.on_ground);
    assert!((p.position.y - 64.0).abs() < 1e-3, "y = {}", p.position.y);
    assert_eq!(p.health, 20.0, "a 2 block fall does no damage");
    // Standing still stays on the ground every tick.
    for _ in 0..20 {
        p.tick(&InputState::default(), &w);
        assert!(p.on_ground);
        assert!((p.position.y - 64.0).abs() < 1e-3);
    }
}

#[test]
fn gravity_and_terminal_velocity() {
    let w = empty_world(1);
    let mut p = survival_player(Vec3::new(0.5, 300.0, 0.5));
    p.game_mode = GameMode::Creative;
    let mut last = p.position.y;
    let mut speeds = Vec::new();
    for _ in 0..200 {
        p.tick(&InputState::default(), &w);
        speeds.push(last - p.position.y);
        last = p.position.y;
    }
    assert!((speeds[1] - 0.0784).abs() < 1e-3, "{:?}", &speeds[..3]);
    let terminal = *speeds.last().unwrap();
    assert!((3.5..4.0).contains(&terminal), "terminal {terminal}");
}

#[test]
fn cannot_walk_through_walls() {
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(5, 64, -8),
        IVec3::new(5, 66, 8),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(1.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    run(&mut p, &w, &input(&[Key::Forward, Key::Sprint]), 100);
    assert!(p.position.x <= 5.0 - 0.3 + 1e-3, "x = {}", p.position.x);
    assert!(
        p.position.x > 4.5,
        "walked up to the wall: x = {}",
        p.position.x
    );
    // Jumping does not get over a 3-high wall either.
    run(&mut p, &w, &input(&[Key::Forward, Key::Jump]), 60);
    assert!(p.position.x <= 5.0 - 0.3 + 1e-3);
}

#[test]
fn walking_speed_is_minecraft_like() {
    let w = flat_world(2, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(-20.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    run(&mut p, &w, &input(&[Key::Forward]), 20);
    let x0 = p.position.x;
    run(&mut p, &w, &input(&[Key::Forward]), 20);
    let walk = p.position.x - x0;
    assert!((4.0..4.6).contains(&walk), "walk {walk} m/s");
    run(&mut p, &w, &input(&[Key::Forward, Key::Sprint]), 20);
    let x1 = p.position.x;
    run(&mut p, &w, &input(&[Key::Forward, Key::Sprint]), 20);
    let sprint = p.position.x - x1;
    assert!(p.sprinting);
    assert!((5.2..6.0).contains(&sprint), "sprint {sprint} m/s");
}

#[test]
fn step_up_small_ledges_but_not_full_blocks() {
    // A lily pad (1/16 high) is stepped over without jumping.
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(4, 64, -2),
        IVec3::new(10, 64, 2),
        blocks::LILY_PAD,
    );
    let mut p = survival_player(Vec3::new(1.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    run(&mut p, &w, &input(&[Key::Forward]), 40);
    assert!(p.position.x > 6.0, "x = {}", p.position.x);
    assert!(
        (p.position.y - (64.0 + 1.0 / 16.0)).abs() < 1e-3,
        "y = {}",
        p.position.y
    );

    // A full block stops a walking body with the default step height...
    let mut w = flat_world(1, 63, blocks::STONE);
    w.set_block(IVec3::new(3, 64, 0), blocks::STONE);
    let mut b = mc_entity::physics::Body::new(Vec3::new(1.5, 64.0, 0.5), 0.6, 1.8);
    b.on_ground = true;
    for _ in 0..20 {
        mc_entity::physics::move_body(&w, &mut b, Vec3::new(0.2, -0.08, 0.0), false);
    }
    assert!(b.pos.x <= 2.7 + 1e-3 && (b.pos.y - 64.0).abs() < 1e-3);
    // ...but a body with a taller step climbs it.
    let mut b = mc_entity::physics::Body::new(Vec3::new(1.5, 64.0, 0.5), 0.6, 1.8);
    b.on_ground = true;
    b.step_height = 1.0;
    while b.pos.x < 3.5 {
        mc_entity::physics::move_body(&w, &mut b, Vec3::new(0.2, -0.08, 0.0), false);
    }
    assert!((b.pos.y - 65.0).abs() < 1e-3, "{:?}", b.pos);
}

#[test]
fn auto_jump_climbs_one_block_ledges() {
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(4, 64, -2),
        IVec3::new(12, 64, 2),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(1.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    run(&mut p, &w, &input(&[Key::Forward]), 40);
    assert!(p.position.x < 4.0, "no auto-jump by default");
    p.auto_jump = true;
    run(&mut p, &w, &input(&[Key::Forward]), 40);
    assert!(
        p.position.x > 5.0 && (p.position.y - 65.0).abs() < 1e-3,
        "{:?}",
        p.position
    );
    // Two-block walls are not auto-jumped.
    let mut w = flat_world(1, 63, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(4, 64, -2),
        IVec3::new(4, 65, 2),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(1.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    p.auto_jump = true;
    run(&mut p, &w, &input(&[Key::Forward]), 40);
    assert!(p.position.x < 4.0 && p.position.y < 64.5);
}

#[test]
fn analog_move_axis_moves_the_player() {
    let w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.yaw = 0.0; // facing -Z
    let mut i = InputState {
        move_axis: (0.0, 1.0),
        ..Default::default()
    };
    run(&mut p, &w, &i, 20);
    assert!(p.position.z < -3.0, "{:?}", p.position);
    let z = p.position.z;
    i.move_axis = (1.0, 0.0);
    run(&mut p, &w, &i, 20);
    assert!(p.position.x > 3.0 && (p.position.z - z).abs() < 0.5);
}

#[test]
fn sneaking_does_not_fall_off_edges() {
    // Platform x in -16..5 at y=63, nothing beyond.
    let mut w = empty_world(1);
    fill(
        &mut w,
        IVec3::new(-16, 63, -4),
        IVec3::new(4, 63, 4),
        blocks::STONE,
    );
    let mut p = survival_player(Vec3::new(1.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    run(&mut p, &w, &input(&[Key::Forward, Key::Sneak]), 100);
    assert!(p.on_ground && (p.position.y - 64.0).abs() < 1e-3);
    assert!(
        p.position.x > 5.0 && p.position.x < 5.3 + 1e-3,
        "x = {}",
        p.position.x
    );
    assert!(p.eye_height < 1.4, "lower eye height while sneaking");
    // Without sneaking it walks off.
    run(&mut p, &w, &input(&[Key::Forward]), 40);
    assert!(p.position.y < 60.0, "y = {}", p.position.y);
}

#[test]
fn water_slows_falling_and_swimming_rises() {
    let mut w = flat_world(1, 40, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(-8, 41, -8),
        IVec3::new(8, 70, 8),
        blocks::WATER,
    );
    let mut p = survival_player(Vec3::new(0.5, 69.0, 0.5));
    run(&mut p, &w, &InputState::default(), 20);
    assert!(p.in_water && p.eyes_in_water);
    let fell = 69.0 - p.position.y;
    assert!(
        fell < 2.5,
        "fell {fell} blocks in water in one second (14 in air)"
    );
    assert!(p.velocity.y.abs() < 0.12, "sink speed {}", p.velocity.y);
    // Holding jump swims up.
    let y = p.position.y;
    run(&mut p, &w, &input(&[Key::Jump]), 40);
    assert!(p.position.y > y + 2.0);
    // Landing on the floor of a pool hurts nothing.
    let mut p = survival_player(Vec3::new(0.5, 90.0, 0.5));
    run(&mut p, &w, &InputState::default(), 200);
    assert_eq!(p.health, 20.0);
}

#[test]
fn fall_damage_amounts() {
    for (height, expected) in [
        (3.0, 0.0),
        (4.0, 1.0),
        (5.0, 2.0),
        (10.0, 7.0),
        (15.0, 12.0),
    ] {
        let w = flat_world(1, 63, blocks::STONE);
        let mut p = survival_player(Vec3::new(0.5, 64.0 + height, 0.5));
        // No regeneration during the test.
        p.food = 17.0;
        p.saturation = 0.0;
        run(&mut p, &w, &InputState::default(), 100);
        assert!(p.on_ground);
        assert_eq!(20.0 - p.health, expected, "fall of {height} blocks");
    }
    // Creative players take no fall damage.
    let w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 90.0, 0.5));
    p.game_mode = GameMode::Creative;
    run(&mut p, &w, &InputState::default(), 100);
    assert_eq!(p.health, 20.0);
    // A 25 block fall kills.
    let mut p = survival_player(Vec3::new(0.5, 89.0, 0.5));
    run(&mut p, &w, &InputState::default(), 100);
    assert!(p.dead && p.health == 0.0);
}

#[test]
fn jump_height_and_sprint_jump() {
    let w = flat_world(2, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.tick(&InputState::default(), &w);
    let mut top: f32 = 0.0;
    for _ in 0..15 {
        p.tick(&input(&[Key::Jump]), &w);
        top = top.max(p.position.y);
    }
    assert!((65.2..65.3).contains(&top), "jump apex {top}");
}

#[test]
fn creative_flight_toggles() {
    let w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.game_mode = GameMode::Creative;
    // Double tap jump.
    let mut tap = input(&[Key::Jump]);
    tap.pressed.insert(Key::Jump);
    p.tick(&tap, &w);
    p.tick(&InputState::default(), &w);
    p.tick(&tap, &w);
    assert!(p.flying);
    let y = p.position.y;
    run(&mut p, &w, &input(&[Key::Jump]), 20);
    assert!(p.position.y > y + 5.0, "flies up with space");
    run(&mut p, &w, &input(&[Key::Sneak]), 5);
    assert!(
        !p.sneaking,
        "shift descends instead of sneaking while flying"
    );
    // Fly key toggles off; then falls.
    let mut f = InputState::default();
    f.pressed.insert(Key::ToggleFly);
    p.tick(&f, &w);
    assert!(!p.flying);
    // Survival players cannot toggle flight.
    let mut s = survival_player(Vec3::new(0.5, 64.0, 0.5));
    s.tick(&f, &w);
    assert!(!s.flying);
}

#[test]
fn drowning_and_air() {
    let mut w = flat_world(1, 40, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(-8, 41, -8),
        IVec3::new(8, 60, 8),
        blocks::WATER,
    );
    let mut p = survival_player(Vec3::new(0.5, 41.0, 0.5));
    p.food = 17.0;
    p.saturation = 0.0;
    run(&mut p, &w, &InputState::default(), 300);
    assert_eq!(p.air, 0);
    assert_eq!(p.health, 20.0);
    run(&mut p, &w, &InputState::default(), 41);
    assert_eq!(p.health, 16.0, "2 damage per second without air");
}

#[test]
fn lava_burns_and_void_kills() {
    let mut w = flat_world(1, 40, blocks::STONE);
    fill(
        &mut w,
        IVec3::new(-2, 41, -2),
        IVec3::new(2, 41, 2),
        blocks::LAVA,
    );
    let mut p = survival_player(Vec3::new(0.5, 41.0, 0.5));
    run(&mut p, &w, &InputState::default(), 10);
    assert!(p.health < 20.0 && p.fire_ticks > 0);

    let w = empty_world(1);
    let mut p = survival_player(Vec3::new(0.5, -140.0, 0.5));
    p.game_mode = GameMode::Creative;
    p.flying = true;
    run(&mut p, &w, &InputState::default(), 200);
    assert!(p.dead, "the void kills even creative players");
}

#[test]
fn hunger_regen_starvation_and_sprint_exhaustion() {
    let w = flat_world(2, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.health = 10.0;
    p.saturation = 0.0;
    p.food = 20.0;
    run(&mut p, &w, &InputState::default(), 400);
    // Each healed point costs 6 exhaustion (1.5 food), so regeneration stops
    // once food drops below 18.
    assert_eq!(p.health, 12.0, "regenerates while food >= 18");
    assert_eq!(p.food, 17.0);
    // With saturation and full food it heals fast.
    p.food = 20.0;
    p.saturation = 5.0;
    run(&mut p, &w, &InputState::default(), 100);
    assert!(p.health >= 15.0, "{}", p.health);

    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.food = 0.0;
    p.saturation = 0.0;
    run(&mut p, &w, &InputState::default(), 81 * 30);
    assert_eq!(p.health, 1.0, "starvation stops at half a heart");

    let mut p = survival_player(Vec3::new(-30.5, 64.0, 0.5));
    p.yaw = YAW_EAST;
    p.saturation = 0.0;
    run(&mut p, &w, &input(&[Key::Forward, Key::Sprint]), 200);
    assert!(p.food < 20.0, "sprinting costs food");
}

#[test]
fn eating_restores_food() {
    let w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.food = 10.0;
    p.saturation = 0.0;
    let bread = ItemId::by_name("bread").unwrap();
    p.inventory.slots[0] = Some(ItemStack::new(bread, 2));
    p.inventory.selected = 0;
    let mut i = InputState::default();
    i.mouse_held.insert(MouseButton::Right);
    run(&mut p, &w, &i, EAT_TICKS - 1);
    assert_eq!(p.food, 10.0);
    assert!(p.eating_progress().unwrap() > 0.9);
    p.tick(&i, &w);
    assert_eq!(p.food, 15.0);
    assert!(p.saturation > 0.0);
    assert_eq!(p.inventory.slots[0].unwrap().count, 1);
    // Releasing the button cancels.
    run(&mut p, &w, &i, 10);
    p.tick(&InputState::default(), &w);
    assert!(p.eating.is_none());
    assert_eq!(p.food, 15.0);
}

#[test]
fn damage_invulnerability_and_knockback() {
    let w = flat_world(1, 63, blocks::STONE);
    let mut p = survival_player(Vec3::new(0.5, 64.0, 0.5));
    p.tick(&InputState::default(), &w);
    assert!(p.damage(
        3.0,
        DamageSource::Mob {
            attacker: Vec3::new(-1.0, 64.0, 0.5)
        }
    ));
    assert_eq!(p.health, 17.0);
    assert!(p.velocity.x > 0.3, "knocked away from the attacker");
    assert!(
        !p.damage(2.0, DamageSource::Generic),
        "invulnerable right after a hit"
    );
    assert!(
        p.damage(5.0, DamageSource::Generic),
        "a stronger hit applies the difference"
    );
    assert_eq!(p.health, 15.0);
    run(&mut p, &w, &InputState::default(), 12);
    assert!(p.damage(1.0, DamageSource::Generic));
    let mut c = survival_player(Vec3::ZERO);
    c.game_mode = GameMode::Creative;
    assert!(!c.damage(100.0, DamageSource::Generic));
}

#[test]
fn unloaded_chunks_hold_the_player() {
    // Only the centre chunk is loaded and it is empty: the player stands on
    // the unloaded neighbour instead of falling forever.
    let mut w = mc_core::World::new(1);
    w.insert_chunk(mc_core::Chunk::new(mc_core::ChunkPos::new(0, 0)));
    let mut p = survival_player(Vec3::new(-0.5, 70.0, 8.0));
    run(&mut p, &w, &InputState::default(), 40);
    assert!((p.position.y - 70.0).abs() < 0.2, "y = {}", p.position.y);
}
