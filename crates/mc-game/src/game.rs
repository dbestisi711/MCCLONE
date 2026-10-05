//! Game state and per-frame orchestration.
//!
//! Order each frame:
//! 1. UI consumes input (screens, hotbar)
//! 2. mouse look
//! 3. fixed 20 TPS ticks: player physics, entities, world time
//! 4. block interaction (break/place/attack)
//! 5. chunk streaming
//! 6. assemble `FrameData` for the renderer

use std::sync::Arc;

use glam::{IVec3, Vec3};
use mc_assets::{Assets, Pack};
use mc_core::block::{Drop, Tool};
use mc_core::input::{InputState, Key, MouseButton};
use mc_core::item::ItemKind;
use mc_core::raycast::{RayHit, raycast};
use mc_core::render_types::{DebugBox, FrameData, SkyState};
use mc_core::{BlockId, ChunkPos, DAY_LENGTH_TICKS, ItemId, ItemStack, TICK_DT, World, blocks};
use mc_entity::{EntityManager, GameMode, Player};
use mc_render::{ChunkStreamer, Renderer};
use mc_ui::{HudInfo, Screen, Ui, UiAction};
use mc_worldgen::WorldGenerator;

pub const REACH: f32 = 4.5;
const MOUSE_SENSITIVITY: f32 = 0.0025;

#[derive(Clone, Debug)]
pub struct Options {
    pub seed: u64,
    pub render_distance: i32,
    pub screenshot: Option<String>,
    pub size: (u32, u32),
    pub time: Option<u64>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub pos: Option<Vec3>,
    pub ui_screen: Option<String>,
    /// Ticks to simulate before taking a screenshot (lets mobs spawn/move).
    pub ticks: u32,
    pub survival: bool,
}

impl Options {
    pub fn from_args(args: Vec<String>) -> Self {
        let mut o = Options {
            seed: 12345,
            render_distance: 10,
            screenshot: None,
            size: (1280, 720),
            time: None,
            yaw: None,
            pitch: None,
            pos: None,
            ui_screen: None,
            ticks: 0,
            survival: false,
        };
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().unwrap_or_default();
            match a.as_str() {
                "--seed" => o.seed = val().parse().unwrap_or(o.seed),
                "--render-distance" | "--rd" => {
                    o.render_distance = val().parse().unwrap_or(o.render_distance)
                }
                "--screenshot" => o.screenshot = Some(val()),
                "--size" => {
                    let v = val();
                    if let Some((w, h)) = v.split_once('x') {
                        o.size = (w.parse().unwrap_or(1280), h.parse().unwrap_or(720));
                    }
                }
                "--time" => o.time = val().parse().ok(),
                "--yaw" => o.yaw = val().parse::<f32>().ok().map(f32::to_radians),
                "--pitch" => o.pitch = val().parse::<f32>().ok().map(f32::to_radians),
                "--pos" => {
                    let v: Vec<f32> = val().split(',').filter_map(|s| s.parse().ok()).collect();
                    if v.len() == 3 {
                        o.pos = Some(Vec3::new(v[0], v[1], v[2]));
                    }
                }
                "--ui" => o.ui_screen = Some(val()),
                "--ticks" => o.ticks = val().parse().unwrap_or(0),
                "--survival" => o.survival = true,
                other => log::warn!("unknown argument {other}"),
            }
        }
        o
    }
}

pub struct Game {
    pub assets: Arc<Assets>,
    pub world: World,
    pub generator: Arc<WorldGenerator>,
    pub streamer: ChunkStreamer,
    pub player: Player,
    pub entities: EntityManager,
    pub ui: Ui,
    pub input: InputState,
    pub quit_requested: bool,
    tick_accum: f32,
    target: Option<RayHit>,
    breaking: Option<(IVec3, f32)>,
    swing: f32,
    place_cooldown: u32,
    show_debug: bool,
    third_person: bool,
    fps: FpsCounter,
    paused: bool,
}

impl Game {
    pub fn new(options: Options) -> Self {
        let pack = Pack::find()
            .expect("could not find the resource pack (set MC_PACK_DIR to the repo root)");
        log::info!("resource pack: {}", pack.root.display());
        let t0 = std::time::Instant::now();
        let assets = Arc::new(Assets::load(pack));
        log::info!("assets loaded in {:?}", t0.elapsed());
        let generator = Arc::new(WorldGenerator::new(options.seed));
        let mut world = World::new(options.seed);
        if let Some(t) = options.time {
            world.time_of_day = t % DAY_LENGTH_TICKS;
        }
        let spawn = generator.spawn_point();
        let mut player = Player::new(spawn);
        if options.survival {
            player.game_mode = GameMode::Survival;
            player.flying = false;
        }
        if let Some(p) = options.pos {
            player.position = p;
            player.prev_position = p;
        }
        if let Some(y) = options.yaw {
            player.yaw = y;
        }
        if let Some(p) = options.pitch {
            player.pitch = p;
        }
        give_starter_items(&mut player);
        let mut streamer = ChunkStreamer::new(generator.clone(), options.render_distance);
        streamer.load_blocking(&mut world, ChunkPos::from_world(player.position), 2);
        let mut ui = Ui::new(&assets);
        match options.ui_screen.as_deref() {
            Some("inventory") => ui.open(Screen::Inventory),
            Some("crafting") => ui.open(Screen::CraftingTable),
            Some("pause") => ui.open(Screen::Pause),
            Some("creative") => ui.open(Screen::CreativeInventory),
            Some("death") => ui.open(Screen::Death),
            Some("furnace") => ui.open(Screen::Furnace),
            _ => {}
        }
        Game {
            entities: EntityManager::new(options.seed),
            assets,
            world,
            generator,
            streamer,
            player,
            ui,
            input: InputState::default(),
            quit_requested: false,
            tick_accum: 0.0,
            target: None,
            breaking: None,
            swing: 0.0,
            place_cooldown: 0,
            show_debug: false,
            third_person: false,
            fps: FpsCounter::default(),
            paused: false,
        }
    }

    pub fn wants_cursor_grab(&self) -> bool {
        !self.ui.screen_open()
    }

    pub fn pause(&mut self) {
        if !self.ui.screen_open() {
            self.ui.open(Screen::Pause);
        }
    }

    fn hud_info(&self) -> HudInfo {
        let p = &self.player;
        let mut debug_lines = Vec::new();
        if self.show_debug {
            let pos = p.position;
            let cp = ChunkPos::from_world(pos);
            debug_lines.push(format!(
                "MCCLONE  {:.0} fps ({:.1} ms)",
                self.fps.fps, self.fps.ms
            ));
            debug_lines.push(format!("XYZ: {:.3} / {:.3} / {:.3}", pos.x, pos.y, pos.z));
            debug_lines.push(format!(
                "Chunk: {} {}  loaded: {}",
                cp.x,
                cp.z,
                self.world.chunk_count()
            ));
            let yaw_deg = p.yaw.to_degrees().rem_euclid(360.0);
            let facing = match ((yaw_deg + 45.0) / 90.0) as i32 % 4 {
                0 => "north (-Z)",
                1 => "east (+X)",
                2 => "south (+Z)",
                _ => "west (-X)",
            };
            debug_lines.push(format!(
                "Facing: {facing} ({:.1} / {:.1})",
                yaw_deg,
                p.pitch.to_degrees()
            ));
            let b = self.world.biome(pos.x.floor() as i32, pos.z.floor() as i32);
            debug_lines.push(format!("Biome: {}", b.def().name));
            let light = self.world.light(pos.floor().as_ivec3() + IVec3::Y);
            debug_lines.push(format!("Light: sky {} block {}", light >> 4, light & 15));
            debug_lines.push(format!(
                "Day time: {}  Entities: {}",
                self.world.time_of_day,
                self.entities.count()
            ));
        }
        HudInfo {
            health: p.health,
            max_health: p.max_health,
            food: p.food,
            air: p.air,
            max_air: 300,
            armor: 0,
            xp_level: 0,
            xp_progress: 0.0,
            creative: p.game_mode == GameMode::Creative,
            show_debug: self.show_debug,
            debug_lines,
            target: self.target.map(|t| t.id.def().display.to_string()),
            dead: p.dead,
            dt: 0.0,
        }
    }

    /// Advance the game by `dt` seconds and produce the frame to render.
    pub fn frame(&mut self, dt: f32) -> FrameData {
        self.fps.update(dt);
        let hud = self.hud_info();

        // 1. UI.
        let was_open = self.ui.screen_open();
        let actions = self
            .ui
            .handle_input(&mut self.input, &mut self.player.inventory, &hud);
        for a in actions {
            match a {
                UiAction::Drop(stack) => {
                    let eye = self.player.eye_position(1.0);
                    self.entities
                        .spawn_item(stack, eye + self.player.look_dir() * 0.5);
                }
                UiAction::Respawn => {
                    let spawn = self.generator.spawn_point();
                    let inv = std::mem::take(&mut self.player.inventory);
                    let mode = self.player.game_mode;
                    self.player = Player::new(spawn);
                    self.player.game_mode = mode;
                    self.player.flying = mode == GameMode::Creative;
                    if mode == GameMode::Creative {
                        self.player.inventory = inv;
                    }
                }
                UiAction::Quit => self.quit_requested = true,
            }
        }
        self.paused = self.ui.screen == Screen::Pause;
        let screen_open = self.ui.screen_open();

        // Global keys.
        if self.input.consume(Key::Debug) {
            self.show_debug = !self.show_debug;
        }
        if self.input.consume(Key::TogglePerspective) {
            self.third_person = !self.third_person;
        }
        if self.input.consume(Key::HideHud) {
            self.ui.hide_hud = !self.ui.hide_hud;
        }
        if self.input.consume(Key::ToggleGameMode) {
            self.player.game_mode = match self.player.game_mode {
                GameMode::Creative => GameMode::Survival,
                GameMode::Survival => GameMode::Creative,
            };
            self.player.flying = false;
        }

        // 2. Mouse look.
        if !screen_open && !was_open {
            let (dx, dy) = self.input.mouse_delta;
            self.player.look(dx, dy, MOUSE_SENSITIVITY);
        }

        // 3. Fixed ticks.
        let mut tick_input = self.input.clone();
        if screen_open {
            tick_input.held.clear();
            tick_input.mouse_held.clear();
        }
        if !self.paused {
            self.tick_accum += dt;
            let mut n = 0;
            while self.tick_accum >= TICK_DT && n < 5 {
                self.tick_accum -= TICK_DT;
                n += 1;
                self.tick(&tick_input);
                // Edge-triggered keys only apply to the first tick of a frame.
                tick_input.pressed.clear();
                tick_input.mouse_pressed.clear();
            }
            if n == 5 {
                self.tick_accum = 0.0;
            }
        }
        let partial = (self.tick_accum / TICK_DT).clamp(0.0, 1.0);

        // 4. Interaction (frame-rate for responsiveness).
        if !screen_open && !self.player.dead {
            self.interact(dt);
        } else {
            self.breaking = None;
        }
        self.swing = (self.swing - dt * 3.0).max(0.0);

        // 5. Streaming.
        self.streamer
            .update(&mut self.world, ChunkPos::from_world(self.player.position));

        // 6. Frame.
        let mut camera = self.player.camera(partial);
        if self.third_person {
            camera.position -= camera.forward() * 4.0;
        }
        let eye_block = camera.position.floor().as_ivec3();
        let eye_id = self.world.block(eye_block);
        let biome = self.world.biome(eye_block.x, eye_block.z).def();
        let sky = SkyState {
            time_of_day: self.world.time_of_day,
            daylight: self.world.daylight(),
            sky_color: biome.sky_color,
            fog_color: biome.fog_color,
            underwater: eye_id == blocks::WATER,
            in_lava: eye_id == blocks::LAVA,
            rain: 0.0,
        };
        let mut frame = FrameData {
            camera,
            sky,
            breaking: self.breaking,
            held_item: self.player.inventory.selected_item(),
            swing: self.swing,
            third_person: self.third_person,
            ..Default::default()
        };
        self.entities.fill_frame(partial, &mut frame);
        if let Some(t) = self.target {
            let p = t.block.as_vec3();
            frame.boxes.push(DebugBox {
                min: p - Vec3::splat(0.002),
                max: p + Vec3::splat(1.002),
                color: [0.0, 0.0, 0.0, 0.6],
            });
        }
        let (w, h) = self.input.window_size;
        let mut hud = self.hud_info();
        hud.dt = dt;
        self.ui.build(
            &mut frame.ui,
            w as f32,
            h as f32,
            &self.player.inventory,
            &hud,
        );
        self.input.end_frame();
        frame
    }

    pub fn after_render(&mut self, _renderer: &mut Renderer) {}

    fn tick(&mut self, input: &InputState) {
        self.world.tick += 1;
        self.world.time_of_day = (self.world.time_of_day + 1) % DAY_LENGTH_TICKS;
        self.player.tick(input, &self.world);
        self.entities.tick(&mut self.world, &mut self.player);
        for ev in self.entities.take_events() {
            // No audio backend yet, so sounds are dropped here. Explosions
            // have already edited the world (dirty blocks get remeshed).
            if let mc_entity::EntityEvent::Explosion { pos, power } = ev {
                log::debug!("explosion at {pos} (power {power})");
            }
        }
        if self.place_cooldown > 0 {
            self.place_cooldown -= 1;
        }
        if self.player.dead && self.ui.screen != Screen::Death {
            self.ui.open(Screen::Death);
        }
    }

    fn interact(&mut self, dt: f32) {
        let eye = self.player.eye_position(1.0);
        let dir = self.player.look_dir();
        self.target = raycast(&self.world, eye, dir, REACH);
        let entity_dist = self.entities.raycast(eye, dir, REACH.min(3.0));
        let entity_first = match (entity_dist, self.target) {
            (Some(e), Some(t)) => e < t.distance,
            (Some(_), None) => true,
            _ => false,
        };
        let creative = self.player.game_mode == GameMode::Creative;

        // Attack / break.
        if self.input.mouse_pressed(MouseButton::Left) {
            self.swing = 1.0;
            if entity_first {
                let dmg = match self.player.inventory.selected_item().map(|i| i.kind()) {
                    Some(ItemKind::Tool { damage, .. }) => damage,
                    _ => 1.0,
                };
                if self.entities.attack(eye, dir, 3.0, dmg) && !creative {
                    self.player.exhaustion += 0.1;
                }
                self.breaking = None;
                return;
            }
        }
        if self.input.mouse_held(MouseButton::Left) && !entity_first {
            if let Some(t) = self.target {
                self.swing = self.swing.max(0.5);
                let time = break_time(t.id, self.player.inventory.selected_item(), creative);
                let progress = match self.breaking {
                    Some((p, prog)) if p == t.block => prog,
                    _ => 0.0,
                };
                if creative && !self.input.mouse_pressed(MouseButton::Left) {
                    // Creative: one block per click.
                } else if time.is_finite() {
                    let progress = if time <= 0.0 {
                        1.0
                    } else {
                        progress + dt / time
                    };
                    if progress >= 1.0 {
                        self.break_block(t.block, t.id, creative);
                        self.breaking = None;
                    } else {
                        self.breaking = Some((t.block, progress));
                    }
                }
            } else {
                self.breaking = None;
            }
        } else {
            self.breaking = None;
        }

        // Use on an entity (e.g. shears on a sheep).
        if self.input.mouse_pressed(MouseButton::Right) && entity_first {
            let held = self.player.inventory.selected_item();
            if self.entities.interact(eye, dir, 3.0, held) {
                self.swing = 1.0;
                self.place_cooldown = 4;
                if !creative {
                    self.wear_selected_tool();
                }
                return;
            }
        }

        // Use / place.
        let use_pressed = self.input.mouse_pressed(MouseButton::Right)
            || (self.input.mouse_held(MouseButton::Right) && self.place_cooldown == 0);
        if use_pressed {
            if let Some(t) = self.target {
                if t.id == blocks::CRAFTING_TABLE && !self.input.held(Key::Sneak) {
                    self.ui.open(Screen::CraftingTable);
                    return;
                }
                if t.id == blocks::FURNACE && !self.input.held(Key::Sneak) {
                    self.ui.open_furnace(t.block);
                    return;
                }
                if let Some(block) = self
                    .player
                    .inventory
                    .selected_item()
                    .and_then(|i| i.block())
                {
                    let target_def = t.id.def();
                    let pos = if target_def.replaceable {
                        t.block
                    } else {
                        t.block + t.face.normal()
                    };
                    let existing = self.world.block(pos);
                    let bbox = mc_core::Aabb::block(pos);
                    let blocked = block.def().solid
                        && (bbox.intersects(&self.player.aabb())
                            || self.entities.any_mob_in(&bbox));
                    if existing.def().replaceable
                        && !blocked
                        && pos.y >= mc_core::WORLD_MIN_Y
                        && pos.y < mc_core::WORLD_MAX_Y
                    {
                        let below = self.world.block(pos - IVec3::Y);
                        if !block.def().needs_support || below.def().solid {
                            self.world.set_block(pos, block);
                            self.entities.on_block_changed(&self.world, pos);
                            self.swing = 1.0;
                            if !creative {
                                self.player.inventory.consume_selected(1);
                            }
                        }
                    }
                }
            }
            self.place_cooldown = 4;
        }

        // Middle click: pick block (creative).
        if creative && self.input.mouse_pressed(MouseButton::Middle) {
            if let Some(t) = self.target {
                let item = ItemId::from_block(t.id);
                let inv = &mut self.player.inventory;
                if let Some(i) = inv.slots[..9]
                    .iter()
                    .position(|s| s.map(|s| s.item) == Some(item))
                {
                    inv.selected = i;
                } else {
                    inv.slots[inv.selected] = Some(ItemStack::new(item, 64));
                }
            }
        }

        // Drop held item.
        if self.input.consume(Key::Drop) {
            if let Some(stack) = self.player.inventory.selected_stack().copied() {
                let n = if self.input.held(Key::Sprint) {
                    stack.count
                } else {
                    1
                };
                self.player.inventory.consume_selected(n);
                self.entities
                    .throw_item(ItemStack { count: n, ..stack }, eye, dir);
            }
        }
    }

    fn wear_selected_tool(&mut self) {
        let inv = &mut self.player.inventory;
        if let Some(stack) = inv.slots[inv.selected].as_mut() {
            if let ItemKind::Tool { durability, .. } = stack.item.kind() {
                stack.damage += 1;
                if stack.damage >= durability {
                    inv.slots[inv.selected] = None;
                }
            }
        }
    }

    fn break_block(&mut self, pos: IVec3, id: BlockId, creative: bool) {
        self.world.set_block(pos, blocks::AIR);
        if id == blocks::FURNACE {
            for s in self.ui.remove_furnace(pos) {
                self.entities
                    .spawn_item(s, pos.as_vec3() + Vec3::splat(0.5));
            }
        }
        self.entities.on_block_changed(&self.world, pos);
        // Unsupported plants/torches above pop off.
        let above = pos + IVec3::Y;
        let up = self.world.block(above);
        if up.def().needs_support {
            self.world.set_block(above, blocks::AIR);
            if !creative {
                self.drop_for(up, above);
            }
        }
        if !creative {
            self.drop_for(id, pos);
            if let Some(stack) =
                self.player.inventory.slots[self.player.inventory.selected].as_mut()
            {
                if let ItemKind::Tool { durability, .. } = stack.item.kind() {
                    stack.damage += 1;
                    if stack.damage >= durability {
                        self.player.inventory.slots[self.player.inventory.selected] = None;
                    }
                }
            }
        }
    }

    fn drop_for(&mut self, id: BlockId, pos: IVec3) {
        let def = id.def();
        if def.needs_tool && !has_right_tool(id, self.player.inventory.selected_item()) {
            return;
        }
        let stack = match def.drop {
            Drop::Itself => Some(ItemStack::of_block(id, 1)),
            Drop::Nothing => None,
            Drop::Other(name, n) => ItemId::by_name(name).map(|i| ItemStack::new(i, n)),
        };
        if let Some(s) = stack {
            self.entities
                .spawn_item(s, pos.as_vec3() + Vec3::splat(0.5));
        }
    }
}

fn has_right_tool(block: BlockId, item: Option<ItemId>) -> bool {
    let def = block.def();
    match item.map(|i| i.kind()) {
        Some(ItemKind::Tool { tool, tier, .. }) => {
            tool == def.tool
                && match block {
                    b if b == blocks::DIAMOND_ORE
                        || b == blocks::DEEPSLATE_DIAMOND_ORE
                        || b == blocks::GOLD_ORE
                        || b == blocks::DEEPSLATE_GOLD_ORE
                        || b == blocks::EMERALD_ORE
                        || b == blocks::REDSTONE_ORE
                        || b == blocks::DEEPSLATE_REDSTONE_ORE =>
                    {
                        tier >= 2
                    }
                    b if b == blocks::OBSIDIAN => tier >= 3,
                    b if b == blocks::IRON_ORE
                        || b == blocks::DEEPSLATE_IRON_ORE
                        || b == blocks::LAPIS_ORE
                        || b == blocks::DEEPSLATE_LAPIS_ORE
                        || b == blocks::COPPER_ORE
                        || b == blocks::DEEPSLATE_COPPER_ORE =>
                    {
                        tier >= 1
                    }
                    _ => true,
                }
        }
        _ => false,
    }
}

/// Seconds to break `block` holding `item`. Infinite = unbreakable.
fn break_time(block: BlockId, item: Option<ItemId>, creative: bool) -> f32 {
    let def = block.def();
    if def.hardness < 0.0 {
        return f32::INFINITY;
    }
    if creative {
        return 0.0;
    }
    let mut speed = 1.0;
    if let Some(ItemKind::Tool { tool, speed: s, .. }) = item.map(|i| i.kind()) {
        if tool == def.tool || (tool == Tool::Sword && def.tool == Tool::None && def.hardness < 0.5)
        {
            speed = s;
        }
    }
    let penalty = if def.needs_tool && !has_right_tool(block, item) {
        5.0
    } else {
        1.5
    };
    def.hardness * penalty / speed
}

fn give_starter_items(player: &mut Player) {
    let names = [
        "grass_block",
        "stone",
        "oak_planks",
        "oak_log",
        "glass",
        "torch",
        "cobblestone",
        "sand",
        "crafting_table",
    ];
    for (i, n) in names.iter().enumerate() {
        if let Some(item) = ItemId::by_name(n) {
            player.inventory.slots[i] = Some(ItemStack::new(item, 64));
        }
    }
    for n in [
        "diamond_pickaxe",
        "diamond_axe",
        "diamond_shovel",
        "diamond_sword",
        "bread",
    ] {
        if let Some(item) = ItemId::by_name(n) {
            let count = item.max_stack().min(16);
            player.inventory.add(ItemStack::new(item, count));
        }
    }
}

#[derive(Default)]
struct FpsCounter {
    acc: f32,
    frames: u32,
    fps: f32,
    ms: f32,
}

impl FpsCounter {
    fn update(&mut self, dt: f32) {
        self.acc += dt;
        self.frames += 1;
        if self.acc >= 0.5 {
            self.fps = self.frames as f32 / self.acc;
            self.ms = self.acc * 1000.0 / self.frames as f32;
            self.acc = 0.0;
            self.frames = 0;
        }
    }
}

/// Headless mode: generate the world around the player, render one frame
/// offscreen (works with a software Vulkan driver), and save it as PNG.
pub fn run_screenshot(options: &Options, path: &str) {
    let mut game = Game::new(options.clone());
    let center = ChunkPos::from_world(game.player.position);
    game.streamer
        .load_blocking(&mut game.world, center, options.render_distance);
    if options.pos.is_none() {
        // Put the camera a little above the ground at spawn.
        let p = game.player.position;
        let h = game
            .world
            .height(p.x.floor() as i32, p.z.floor() as i32)
            .unwrap_or(64);
        game.player.position.y = h as f32 + 1.0;
        game.player.prev_position = game.player.position;
    }
    let mut renderer = Renderer::new_offscreen(game.assets.clone(), options.size.0, options.size.1);
    log::info!("offscreen renderer: {}", renderer.adapter_info);
    game.input.window_size = options.size;
    game.input.scale_factor = 1.0;
    // Simulate requested ticks (mobs, physics) before the shot.
    let idle = InputState::default();
    for _ in 0..options.ticks {
        game.tick(&idle);
    }
    // Let the streamer / mesher settle for a few frames.
    let mut frame = game.frame(0.0);
    for _ in 0..30 {
        renderer.render(&game.world, &frame);
        frame = game.frame(0.0);
    }
    renderer.render(&game.world, &frame);
    let img = renderer.capture().expect("capture");
    save_png(path, &img);
    log::info!("saved {path}");
}

pub fn save_png(path: &str, img: &mc_core::Rgba8Image) {
    let file = std::fs::File::create(path).expect("create screenshot file");
    let w = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(w, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&img.data))
        .expect("write png");
}
