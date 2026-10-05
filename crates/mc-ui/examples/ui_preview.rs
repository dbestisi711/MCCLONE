//! Render the HUD and every screen to PNGs on the CPU (no GPU needed).
//!
//! cargo run -p mc-ui --example ui_preview -- [out_dir] [WIDTHxHEIGHT]

use glam::Vec2;
use mc_assets::{Assets, Pack};
use mc_core::input::InputState;
use mc_core::render_types::UiDrawList;
use mc_core::{Inventory, ItemId, ItemStack, Rgba8Image};
use mc_ui::preview::{SoftwareRenderer, backdrop};
use mc_ui::{HudInfo, Screen, Ui};

fn st(n: &str, c: u8) -> Option<ItemStack> {
    Some(ItemStack::new(
        ItemId::by_name(n).unwrap_or_else(|| panic!("no item {n}")),
        c,
    ))
}

fn save(path: &str, img: &Rgba8Image) {
    image::save_buffer(
        path,
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    )
    .expect("save png");
    println!("wrote {path}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = args.first().cloned().unwrap_or_else(|| ".".into());
    let (w, h) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .map(|(a, b)| (a.parse().unwrap_or(1280), b.parse().unwrap_or(720)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out_dir).ok();
    let assets = Assets::load(Pack::find().expect("resource pack"));

    let mut inv = Inventory::default();
    let hotbar = [
        st("grass_block", 64),
        st("stone", 32),
        st("oak_planks", 17),
        st("torch", 12),
        st("diamond_pickaxe", 1),
        st("bread", 5),
        st("oak_log", 1),
        st("poppy", 3),
        st("glass", 48),
    ];
    inv.slots[..9].copy_from_slice(&hotbar);
    if let Some(s) = &mut inv.slots[4] {
        s.damage = 900;
    }
    let main = [
        (9, st("cobblestone", 64)),
        (10, st("cobblestone", 40)),
        (11, st("coal", 23)),
        (12, st("iron_ingot", 9)),
        (13, st("stick", 8)),
        (14, st("sand", 64)),
        (18, st("crafting_table", 1)),
        (19, st("chest", 2)),
        (20, st("furnace", 1)),
        (22, st("oak_leaves", 30)),
        (23, st("birch_log", 16)),
        (27, st("apple", 3)),
        (28, st("diamond", 4)),
        (29, st("iron_sword", 1)),
        (31, st("bookshelf", 2)),
        (32, st("tnt", 6)),
        (35, st("snow_layer", 7)),
    ];
    for (i, s) in main {
        inv.slots[i] = s;
    }
    inv.selected = 4;

    let mut hud = HudInfo {
        health: 13.0,
        max_health: 20.0,
        food: 15.0,
        air: 140,
        max_air: 300,
        armor: 7,
        xp_level: 12,
        xp_progress: 0.62,
        dt: 0.016,
        ..Default::default()
    };

    let render = |ui: &mut Ui, inv: &Inventory, hud: &HudInfo, name: &str| {
        // Each preview uses a fresh rasteriser, so ask for the atlases again.
        ui.invalidate_textures();
        let mut list = UiDrawList::default();
        ui.build(&mut list, w as f32, h as f32, inv, hud);
        let mut img = backdrop(&assets, w, h);
        SoftwareRenderer::new(&assets).render(&list, &mut img);
        save(&format!("{out_dir}/{name}.png"), &img);
    };

    let mut ui = Ui::new(&assets);
    // Switch the hotbar slot so the selected item's name shows.
    inv.selected = 3;
    render(&mut ui, &inv, &hud, "_warmup");
    inv.selected = 4;
    hud.dt = 0.2;
    render(&mut ui, &inv, &hud, "ui_hud");

    hud.show_debug = true;
    hud.debug_lines = vec![
        "MCCLONE  60 fps (16.7 ms)".into(),
        "XYZ: 12.500 / 71.000 / -8.250".into(),
        "Chunk: 0 -1  loaded: 441".into(),
        "Facing: north (-Z) (12.0 / -20.0)".into(),
        "Biome: plains".into(),
        "Light: sky 15 block 0".into(),
    ];
    hud.target = Some("Grass Block".into());
    render(&mut ui, &inv, &hud, "ui_hud_debug");
    hud.show_debug = false;
    hud.debug_lines.clear();

    // Survival inventory: logs in the grid, carrying sticks, hovering a slot.
    let mut input = InputState {
        window_size: (w, h),
        ..Default::default()
    };
    ui.open(Screen::Inventory);
    ui.handle_input(&mut input, &mut inv, &hud);
    let s = ui.scale;
    set_grid(&mut ui, &[(1, st("oak_log", 3))]);
    move_cursor(
        &mut ui,
        &mut inv,
        &hud,
        w,
        h,
        Vec2::new(0.5, 0.5) * Vec2::new(w as f32, h as f32) + Vec2::new(-60.0, 30.0) * s,
    );
    render(&mut ui, &inv, &hud, "ui_inventory");

    // Crafting table with a pickaxe recipe, hovering the result.
    ui.open(Screen::CraftingTable);
    ui.handle_input(&mut input, &mut inv, &hud);
    set_grid(
        &mut ui,
        &[
            (0, st("cobblestone", 1)),
            (1, st("cobblestone", 1)),
            (2, st("cobblestone", 1)),
            (4, st("stick", 1)),
            (7, st("stick", 1)),
        ],
    );
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    move_cursor(
        &mut ui,
        &mut inv,
        &hud,
        w,
        h,
        Vec2::new(cx + 45.0 * s, cy - 32.0 * s),
    );
    render(&mut ui, &inv, &hud, "ui_crafting");
    set_grid(&mut ui, &[]);

    // Creative inventory.
    hud.creative = true;
    ui.open(Screen::CreativeInventory);
    ui.handle_input(&mut input, &mut inv, &hud);
    move_cursor(
        &mut ui,
        &mut inv,
        &hud,
        w,
        h,
        Vec2::new(cx - 40.0 * s, cy - 20.0 * s),
    );
    render(&mut ui, &inv, &hud, "ui_creative");
    hud.creative = false;

    ui.open(Screen::Pause);
    move_cursor(
        &mut ui,
        &mut inv,
        &hud,
        w,
        h,
        Vec2::new(cx, h as f32 / 4.0 + 42.0 * s),
    );
    render(&mut ui, &inv, &hud, "ui_pause");

    hud.dead = true;
    hud.health = 0.0;
    ui.open(Screen::Death);
    move_cursor(&mut ui, &mut inv, &hud, w, h, Vec2::new(10.0, 10.0));
    render(&mut ui, &inv, &hud, "ui_death");
    std::fs::remove_file(format!("{out_dir}/_warmup.png")).ok();
}

fn set_grid(ui: &mut Ui, cells: &[(usize, Option<ItemStack>)]) {
    let c = ui.container_mut();
    c.craft = [None; 9];
    for &(i, s) in cells {
        c.craft[i] = s;
    }
}

fn move_cursor(ui: &mut Ui, inv: &mut Inventory, hud: &HudInfo, w: u32, h: u32, pos: Vec2) {
    let mut input = InputState {
        window_size: (w, h),
        cursor_pos: (pos.x, pos.y),
        ..Default::default()
    };
    ui.handle_input(&mut input, inv, hud);
}
