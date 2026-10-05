//! Test scenes for screenshot mode (`--scene NAME`): small structures built
//! with `World::set_block` around the spawn so the renderer can be checked
//! on water, leaves, glass, plants, torches, caves and entities regardless
//! of what the terrain generator produces.

use glam::{IVec3, Mat4, Vec3};
use mc_core::render_types::{
    BlockModelInstance, DebugBox, EntityRenderInstance, FrameData, ItemEntityInstance, TextureKey,
};
use mc_core::{BlockId, ItemId, World, blocks};

/// Where to put the camera for a scene.
pub struct SceneView {
    pub eye: Vec3,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

fn fill(world: &mut World, a: IVec3, b: IVec3, id: BlockId) {
    let (lo, hi) = (a.min(b), a.max(b));
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                world.set_block(IVec3::new(x, y, z), id);
            }
        }
    }
}

fn tree(world: &mut World, base: IVec3, log: BlockId, leaves: BlockId) {
    for dy in 3..=6 {
        let r: i32 = if dy >= 5 { 1 } else { 2 };
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() == r && dz.abs() == r && dy != 4 {
                    continue;
                }
                world.set_block(base + IVec3::new(dx, dy, dz), leaves);
            }
        }
    }
    for dy in 0..5 {
        world.set_block(base + IVec3::new(0, dy, 0), log);
    }
}

/// Build a scene around `origin` (a surface position). Returns the camera.
pub fn build(name: &str, world: &mut World, origin: IVec3) -> Option<SceneView> {
    let o = origin;
    match name {
        "showcase" | "garden" => {
            // Flat stone-brick platform.
            fill(
                world,
                o + IVec3::new(-12, -1, -24),
                o + IVec3::new(12, -1, 0),
                blocks::GRASS_BLOCK,
            );
            fill(
                world,
                o + IVec3::new(-12, 0, -24),
                o + IVec3::new(12, 12, 0),
                blocks::AIR,
            );
            fill(
                world,
                o + IVec3::new(-12, -4, -24),
                o + IVec3::new(12, -2, 0),
                blocks::DIRT,
            );
            // Water pool with a sand rim.
            fill(
                world,
                o + IVec3::new(-10, -1, -10),
                o + IVec3::new(-3, -1, -4),
                blocks::SAND,
            );
            fill(
                world,
                o + IVec3::new(-9, -3, -9),
                o + IVec3::new(-4, -1, -5),
                blocks::WATER,
            );
            world.set_block(o + IVec3::new(-6, 0, -7), blocks::LILY_PAD);
            // Lava pit.
            fill(
                world,
                o + IVec3::new(6, -2, -6),
                o + IVec3::new(8, -1, -4),
                blocks::LAVA,
            );
            fill(
                world,
                o + IVec3::new(5, -1, -7),
                o + IVec3::new(9, -1, -7),
                blocks::COBBLESTONE,
            );
            // Trees.
            tree(
                world,
                o + IVec3::new(-6, 0, -16),
                blocks::OAK_LOG,
                blocks::OAK_LEAVES,
            );
            tree(
                world,
                o + IVec3::new(6, 0, -18),
                blocks::BIRCH_LOG,
                blocks::BIRCH_LEAVES,
            );
            // Glass house with a torch and glowstone.
            fill(
                world,
                o + IVec3::new(0, 0, -14),
                o + IVec3::new(3, 3, -11),
                blocks::GLASS,
            );
            fill(
                world,
                o + IVec3::new(1, 0, -13),
                o + IVec3::new(2, 2, -12),
                blocks::AIR,
            );
            world.set_block(o + IVec3::new(1, 0, -13), blocks::GLOWSTONE);
            // Ice, snow, cactus, plants.
            fill(
                world,
                o + IVec3::new(-2, 0, -6),
                o + IVec3::new(-1, 1, -5),
                blocks::ICE,
            );
            fill(
                world,
                o + IVec3::new(9, -1, -12),
                o + IVec3::new(11, -1, -10),
                blocks::SAND,
            );
            world.set_block(o + IVec3::new(10, 0, -11), blocks::CACTUS);
            world.set_block(o + IVec3::new(10, 1, -11), blocks::CACTUS);
            for (i, p) in [
                blocks::POPPY,
                blocks::DANDELION,
                blocks::SHORT_GRASS,
                blocks::BLUE_ORCHID,
                blocks::FERN,
                blocks::CORNFLOWER,
                blocks::SHORT_GRASS,
                blocks::SHORT_GRASS,
            ]
            .iter()
            .enumerate()
            {
                world.set_block(o + IVec3::new(-2 + i as i32, 0, -3), *p);
            }
            fill(
                world,
                o + IVec3::new(4, 0, -3),
                o + IVec3::new(6, 0, -2),
                blocks::SNOW_LAYER,
            );
            // Torches along the front.
            for x in [-11, -4, 4, 11] {
                world.set_block(o + IVec3::new(x, 0, -1), blocks::TORCH);
            }
            // A few solid blocks for AO.
            world.set_block(o + IVec3::new(3, 0, -7), blocks::CRAFTING_TABLE);
            world.set_block(o + IVec3::new(4, 0, -7), blocks::BOOKSHELF);
            world.set_block(o + IVec3::new(4, 1, -7), blocks::PUMPKIN);
            Some(SceneView {
                eye: o.as_vec3() + Vec3::new(0.5, 4.5, 4.0),
                yaw_deg: 0.0,
                pitch_deg: -22.0,
            })
        }
        "cave" => {
            // A torch-lit cave hall 20 blocks below the surface.
            let c = o - IVec3::new(0, 22, 0);
            fill(
                world,
                c + IVec3::new(-14, -6, -20),
                c + IVec3::new(14, 10, 8),
                blocks::STONE,
            );
            fill(
                world,
                c + IVec3::new(-10, -3, -16),
                c + IVec3::new(10, 4, 4),
                blocks::AIR,
            );
            fill(
                world,
                c + IVec3::new(-4, -3, -24),
                c + IVec3::new(4, 2, -16),
                blocks::AIR,
            );
            fill(
                world,
                c + IVec3::new(-10, -4, -16),
                c + IVec3::new(10, -4, 4),
                blocks::STONE,
            );
            for (x, z) in [(-8, -14), (8, -14), (0, -6), (-8, 2), (8, 2), (0, -22)] {
                world.set_block(c + IVec3::new(x, -3, z), blocks::TORCH);
            }
            fill(
                world,
                c + IVec3::new(-6, -4, -12),
                c + IVec3::new(-2, -4, -8),
                blocks::WATER,
            );
            world.set_block(c + IVec3::new(5, -3, -10), blocks::IRON_ORE);
            world.set_block(c + IVec3::new(6, -3, -10), blocks::COAL_ORE);
            world.set_block(c + IVec3::new(6, -2, -10), blocks::DIAMOND_ORE);
            fill(
                world,
                c + IVec3::new(3, -4, -6),
                c + IVec3::new(4, -4, -5),
                blocks::LAVA,
            );
            Some(SceneView {
                eye: c.as_vec3() + Vec3::new(0.5, 1.6, 3.0),
                yaw_deg: 0.0,
                pitch_deg: -12.0,
            })
        }
        "underwater" => {
            fill(
                world,
                o + IVec3::new(-16, -12, -24),
                o + IVec3::new(16, -1, 4),
                blocks::WATER,
            );
            fill(
                world,
                o + IVec3::new(-16, -13, -24),
                o + IVec3::new(16, -13, 4),
                blocks::SAND,
            );
            for (x, z, b) in [
                (-6, -12, blocks::GRAVEL),
                (4, -8, blocks::CLAY),
                (8, -16, blocks::GRAVEL),
            ] {
                fill(
                    world,
                    o + IVec3::new(x - 1, -13, z - 1),
                    o + IVec3::new(x + 1, -13, z + 1),
                    b,
                );
            }
            tree(
                world,
                o + IVec3::new(0, -12, -14),
                blocks::OAK_LOG,
                blocks::OAK_LEAVES,
            );
            Some(SceneView {
                eye: o.as_vec3() + Vec3::new(0.5, -6.0, 2.0),
                yaw_deg: 0.0,
                pitch_deg: -5.0,
            })
        }
        _ => None,
    }
}

/// Extra frame content for `--scene entities`: placeholder mobs, dropped
/// items, a falling block, a selection box and a crack overlay in front of
/// the camera (the entity system may not be producing any yet).
pub fn decorate_frame(name: &str, world: &World, frame: &mut FrameData, origin: IVec3) {
    if name != "showcase" && name != "garden" && name != "entities" {
        return;
    }
    let o = origin.as_vec3();
    let light = |p: Vec3| world.light(p.floor().as_ivec3());
    for (i, (tex, model)) in [
        ("textures/entity/pig/pig", "geometry.pig"),
        ("textures/entity/cow/cow", "geometry.cow"),
        ("textures/entity/sheep/sheep", "geometry.sheep"),
    ]
    .iter()
    .enumerate()
    {
        let p = o + Vec3::new(-1.5 + i as f32 * 2.0, 0.0, -8.5);
        frame.entities.push(EntityRenderInstance {
            model: (*model).into(),
            texture: TextureKey::new(tex),
            transform: Mat4::from_translation(p) * Mat4::from_rotation_y(0.4 * i as f32),
            poses: vec![],
            tint: [1.0; 4],
            hurt: if i == 1 { 1.0 } else { 0.0 },
            light: light(p + Vec3::Y),
        });
    }
    for (i, name) in ["oak_log", "grass_block", "diamond", "apple", "poppy"]
        .iter()
        .enumerate()
    {
        if let Some(item) = ItemId::by_name(name) {
            let p = o + Vec3::new(-3.0 + i as f32 * 1.2, 0.35, -4.5);
            frame.items.push(ItemEntityInstance {
                item,
                transform: Mat4::from_translation(p)
                    * Mat4::from_rotation_y(0.6 + i as f32)
                    * Mat4::from_scale(Vec3::splat(0.35)),
                light: light(p),
            });
        }
    }
    let fb = o + Vec3::new(1.5, 2.5, -6.5);
    frame.block_models.push(BlockModelInstance {
        block: blocks::SAND,
        transform: Mat4::from_translation(fb),
        light: light(fb),
    });
    let target = origin + IVec3::new(3, 0, -7);
    frame.boxes.push(DebugBox {
        min: target.as_vec3() - Vec3::splat(0.002),
        max: target.as_vec3() + Vec3::splat(1.002),
        color: [0.0, 0.0, 0.0, 0.6],
    });
    frame.breaking = Some((target, 0.55));
}
