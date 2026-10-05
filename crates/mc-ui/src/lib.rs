//! HUD, inventory screens, crafting, text rendering.
//!
//! OWNER: inventory & UI agent. Public API used by `mc-game` (keep stable,
//! add freely):
//! - [`Ui::new`], [`Ui::handle_input`], [`Ui::build`], [`Ui::screen_open`],
//!   [`Ui::open`], [`Ui::close`], [`Ui::pointer`] (device-agnostic pointer
//!   events, e.g. for touch input)
//! - [`HudInfo`] (filled by the game every frame), [`UiAction`], [`Screen`]
//! - [`recipes::RecipeBook`], [`container::Container`] (pure click logic)
//! - [`preview::SoftwareRenderer`] (CPU rasteriser for `UiDrawList`s, used
//!   for previews and tests)
//!
//! The UI never touches the GPU: it emits a `UiDrawList` of textured quads in
//! physical pixels which `mc-render` draws on top of the world. GUI art is
//! read from the resource pack at runtime (`textures/ui/*`, `textures/gui/*`);
//! the font atlas (`@font`) and item icon atlas (`@items`) are uploaded as
//! dynamic textures on the first frame. All GUI textures are pixel art and
//! should be sampled with nearest filtering.

pub mod container;
pub mod creative;
pub mod furnace;
pub mod gui;
mod hud;
pub mod icons;
pub mod preview;
pub mod recipes;
mod screens;
pub mod text;

use glam::{IVec3, Vec2};
use mc_core::input::{InputState, Key, MouseButton};
use mc_core::render_types::UiDrawList;
use mc_core::{Inventory, ItemId, ItemStack};
use rustc_hash::FxHashMap as HashMap;

use container::{ClickButton, Container, Drag, MenuKind, SlotId};
use gui::{Painter, Skin};
use icons::ItemIcons;
use recipes::RecipeBook;
use text::Font;

/// Values the HUD displays; filled by `mc-game` each frame.
#[derive(Clone, Debug, Default)]
pub struct HudInfo {
    pub health: f32,
    pub max_health: f32,
    pub food: f32,
    pub air: i32,
    pub max_air: i32,
    pub armor: u32,
    pub xp_level: u32,
    pub xp_progress: f32,
    pub creative: bool,
    pub show_debug: bool,
    /// Lines for the F3 overlay (left column).
    pub debug_lines: Vec<String>,
    /// Name of the targeted block (for the debug overlay).
    pub target: Option<String>,
    pub dead: bool,
    /// Seconds since the previous frame (drives HUD animations such as the
    /// selected-item name fade and heart blinking). 0 freezes animations.
    pub dt: f32,
}

/// Things the UI asks the game to do.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    /// Drop an item stack in front of the player.
    Drop(mc_core::ItemStack),
    /// Player clicked "Respawn".
    Respawn,
    /// Pause menu "Quit".
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    None,
    /// Survival inventory (2×2 crafting, armor).
    Inventory,
    /// Crafting table (3×3 crafting).
    CraftingTable,
    Pause,
    Death,
    /// Creative item palette with category tabs.
    CreativeInventory,
    /// Furnace (open with [`Ui::open_furnace`]).
    Furnace,
}

impl Screen {
    /// Screens with item slots.
    pub fn has_slots(self) -> bool {
        matches!(
            self,
            Screen::Inventory | Screen::CraftingTable | Screen::CreativeInventory | Screen::Furnace
        )
    }
}

/// Device-agnostic pointer input (mouse now, touch later). Positions are in
/// physical window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Move(Vec2),
    Down(Vec2, ClickButton),
    Up(Vec2, ClickButton),
    /// Scroll wheel / two-finger scroll, in lines (positive = up).
    Scroll(Vec2, f32),
}

pub struct Ui {
    pub screen: Screen,
    /// GUI scale factor (each GUI pixel = `scale` physical pixels).
    pub scale: f32,
    pub hide_hud: bool,
    /// Upper limit for the automatic GUI scale (0 = no limit).
    pub max_gui_scale: u32,
    pub(crate) skin: Skin,
    pub(crate) font: Font,
    pub(crate) icons: ItemIcons,
    pub recipes: RecipeBook,
    pub(crate) container: Container,
    pub(crate) drag: Option<Drag>,
    /// Cursor position in GUI pixels.
    pub(crate) cursor: Vec2,
    pub(crate) shift: bool,
    pub(crate) pressed_button: Option<screens::ButtonId>,
    pub(crate) tabs: Vec<creative::Tab>,
    pub(crate) creative_tab: usize,
    pub(crate) creative_scroll: usize,
    pub(crate) scroll_drag: bool,
    /// Last primary click (slot, HUD time) for double-click detection.
    pub(crate) last_click: Option<(SlotId, f32)>,
    /// Items taken out of a closed screen, returned on the next input pass.
    pending_return: Vec<ItemStack>,
    textures_sent: bool,
    pub(crate) hud_state: hud::HudState,
    pub(crate) gui_size: Vec2,
    /// "Respawn" was clicked; don't reopen the death screen until the game
    /// reports the player alive.
    pub(crate) respawn_requested: bool,
    /// Furnace contents by block position (they keep smelting while closed).
    furnaces: HashMap<IVec3, furnace::Furnace>,
    /// Position of the furnace shown by the open furnace screen.
    furnace_pos: Option<IVec3>,
}

/// Key for a furnace opened without a block position (previews, tests).
const PORTABLE_FURNACE: IVec3 = IVec3::new(i32::MIN, i32::MIN, i32::MIN);

impl Ui {
    pub fn new(assets: &mc_assets::Assets) -> Self {
        Ui {
            screen: Screen::None,
            scale: 2.0,
            hide_hud: false,
            max_gui_scale: 0,
            skin: Skin::load(&assets.pack),
            font: Font::load(&assets.pack),
            icons: ItemIcons::build(assets),
            recipes: RecipeBook::standard(),
            container: Container::new(MenuKind::Inventory),
            drag: None,
            cursor: Vec2::new(-1000.0, -1000.0),
            shift: false,
            pressed_button: None,
            tabs: creative::tabs(),
            creative_tab: 0,
            creative_scroll: 0,
            scroll_drag: false,
            last_click: None,
            pending_return: Vec::new(),
            textures_sent: false,
            hud_state: hud::HudState::default(),
            gui_size: Vec2::new(640.0, 360.0),
            respawn_requested: false,
            furnaces: HashMap::default(),
            furnace_pos: None,
        }
    }

    pub fn screen_open(&self) -> bool {
        self.screen != Screen::None
    }

    /// Open a screen (e.g. crafting table when the player right-clicks one).
    /// Items in a previously open crafting grid / on the cursor are returned
    /// to the inventory on the next [`Ui::handle_input`].
    pub fn open(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        self.stash_container();
        self.screen = screen;
        match screen {
            Screen::Inventory => self.container.set_kind(MenuKind::Inventory),
            Screen::CraftingTable => self.container.set_kind(MenuKind::CraftingTable),
            Screen::CreativeInventory => self.container.set_kind(MenuKind::Creative),
            Screen::Furnace => {
                let pos = self.furnace_pos.unwrap_or(PORTABLE_FURNACE);
                self.furnace_pos = Some(pos);
                self.container.set_kind(MenuKind::Furnace);
                self.container.furnace = Some(self.furnaces.remove(&pos).unwrap_or_default());
            }
            _ => {}
        }
    }

    /// Open the furnace screen for the furnace block at `pos`.
    pub fn open_furnace(&mut self, pos: IVec3) {
        if self.screen == Screen::Furnace && self.furnace_pos == Some(pos) {
            return;
        }
        self.close();
        self.furnace_pos = Some(pos);
        self.open(Screen::Furnace);
    }

    /// The furnace block at `pos` was broken: forget it and return its
    /// contents (for the game to drop).
    pub fn remove_furnace(&mut self, pos: IVec3) -> Vec<ItemStack> {
        if self.screen == Screen::Furnace && self.furnace_pos == Some(pos) {
            self.close();
        }
        self.furnaces
            .remove(&pos)
            .map(|f| f.contents())
            .unwrap_or_default()
    }

    /// State of the furnace at `pos` (e.g. to render it lit).
    pub fn furnace(&self, pos: IVec3) -> Option<&furnace::Furnace> {
        if self.furnace_pos == Some(pos)
            && let Some(f) = &self.container.furnace
        {
            return Some(f);
        }
        self.furnaces.get(&pos)
    }

    /// Advance all furnaces by `dt` seconds. Called from [`Ui::build`] with
    /// [`HudInfo::dt`] (not while paused).
    pub fn tick_furnaces(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        for f in self.furnaces.values_mut() {
            f.tick(dt);
        }
        if let Some(f) = &mut self.container.furnace {
            f.tick(dt);
        }
        // Drop empty, idle furnaces from the map.
        self.furnaces
            .retain(|_, f| f.burning() || !f.contents().is_empty());
    }

    /// Close the current screen.
    pub fn close(&mut self) {
        self.open(Screen::None);
    }

    /// Ask for the font/icon atlases to be uploaded again (e.g. after the
    /// renderer lost its textures).
    pub fn invalidate_textures(&mut self) {
        self.textures_sent = false;
    }

    /// Slot state of the open screen (crafting grid, carried stack).
    pub fn container(&self) -> &Container {
        &self.container
    }

    pub fn container_mut(&mut self) -> &mut Container {
        &mut self.container
    }

    /// Hotbar slot under a physical-pixel position when no screen is open
    /// (for tapping the hotbar on touch screens).
    pub fn hotbar_slot_at(&self, pos: Vec2) -> Option<usize> {
        let p = pos / self.scale;
        let x0 = (self.gui_size.x / 2.0).floor() - 91.0;
        let y0 = self.gui_size.y - 22.0;
        if p.y < y0 || p.y >= y0 + 22.0 || p.x < x0 + 1.0 || p.x >= x0 + 181.0 {
            return None;
        }
        Some((((p.x - x0 - 1.0) / 20.0) as usize).min(8))
    }

    /// The stack currently carried on the cursor.
    pub fn carried(&self) -> Option<ItemStack> {
        self.container.carried
    }

    fn stash_container(&mut self) {
        self.drag = None;
        self.scroll_drag = false;
        self.pressed_button = None;
        if let Some(c) = self.container.carried.take() {
            self.pending_return.push(c);
        }
        for slot in self.container.craft.iter_mut() {
            if let Some(s) = slot.take() {
                self.pending_return.push(s);
            }
        }
        if let Some(f) = self.container.furnace.take() {
            let pos = self.furnace_pos.take().unwrap_or(PORTABLE_FURNACE);
            self.furnaces.insert(pos, f);
        }
    }

    fn update_scale(&mut self, w: f32, h: f32) {
        if w > 0.0 && h > 0.0 {
            self.scale = gui::auto_scale(w, h, self.max_gui_scale);
            self.gui_size = Vec2::new((w / self.scale).floor(), (h / self.scale).floor());
        }
    }

    /// Process input for open screens and the hotbar. Consumes the keys/clicks
    /// it handles so the game doesn't also act on them.
    pub fn handle_input(
        &mut self,
        input: &mut InputState,
        inventory: &mut Inventory,
        hud: &HudInfo,
    ) -> Vec<UiAction> {
        let mut actions = Vec::new();
        let (w, h) = input.window_size;
        self.update_scale(w as f32, h as f32);
        self.flush_returns(inventory, &mut actions);

        if !hud.dead {
            self.respawn_requested = false;
            if self.screen == Screen::Death {
                self.close();
            }
        } else if self.screen != Screen::Death && !self.respawn_requested {
            self.open(Screen::Death);
        }
        self.shift = input.held(Key::Shift);

        // Screen toggles.
        if input.consume(Key::Escape) {
            match self.screen {
                Screen::Death => {}
                Screen::None => self.open(Screen::Pause),
                _ => self.close(),
            }
        }
        if input.consume(Key::Inventory) {
            match self.screen {
                Screen::None => self.open(if hud.creative {
                    Screen::CreativeInventory
                } else {
                    Screen::Inventory
                }),
                s if s.has_slots() => self.close(),
                _ => {}
            }
        }

        if !self.screen_open() {
            for i in 0..9u8 {
                if input.consume(Key::Hotbar(i)) {
                    inventory.selected = i as usize;
                }
            }
            if input.scroll != 0.0 {
                let n = inventory.selected as i32 - input.scroll.signum() as i32;
                inventory.selected = n.rem_euclid(9) as usize;
            }
            self.flush_returns(inventory, &mut actions);
            return actions;
        }

        // Pointer input for screens.
        let pos = Vec2::new(input.cursor_pos.0, input.cursor_pos.1);
        self.pointer(PointerEvent::Move(pos), inventory, hud, &mut actions);
        if input.scroll != 0.0 {
            self.pointer(
                PointerEvent::Scroll(pos, input.scroll),
                inventory,
                hud,
                &mut actions,
            );
        }
        for (mb, cb) in [
            (MouseButton::Left, ClickButton::Primary),
            (MouseButton::Right, ClickButton::Secondary),
            (MouseButton::Middle, ClickButton::Middle),
        ] {
            if input.consume_mouse(mb) {
                self.pointer(PointerEvent::Down(pos, cb), inventory, hud, &mut actions);
            }
        }
        for (mb, cb) in [
            (MouseButton::Left, ClickButton::Primary),
            (MouseButton::Right, ClickButton::Secondary),
            (MouseButton::Middle, ClickButton::Middle),
        ] {
            if input.mouse_released.remove(&mb) {
                self.pointer(PointerEvent::Up(pos, cb), inventory, hud, &mut actions);
            }
        }

        // Keys acting on the hovered slot.
        if self.screen.has_slots() {
            let hovered = self.hovered_slot(inventory, hud);
            for i in 0..9u8 {
                if input.consume(Key::Hotbar(i))
                    && let (Some(s), None) = (hovered, self.container.carried)
                {
                    self.container
                        .swap_with_hotbar(inventory, &self.recipes, s, i as usize);
                }
            }
            if input.consume(Key::Drop)
                && let (Some(s), None) = (hovered, self.container.carried)
            {
                let whole = input.held(Key::Sprint);
                if let Some(d) = self.container.drop_from(inventory, &self.recipes, s, whole) {
                    actions.push(UiAction::Drop(d));
                }
            }
        }
        self.flush_returns(inventory, &mut actions);
        actions
    }

    /// Feed one pointer event (mouse or touch) to the open screen.
    pub fn pointer(
        &mut self,
        ev: PointerEvent,
        inventory: &mut Inventory,
        hud: &HudInfo,
        actions: &mut Vec<UiAction>,
    ) {
        let to_gui = |p: Vec2, s: f32| p / s;
        match ev {
            PointerEvent::Move(p) => {
                self.cursor = to_gui(p, self.scale);
                self.pointer_move(inventory, hud);
            }
            PointerEvent::Down(p, b) => {
                self.cursor = to_gui(p, self.scale);
                self.pointer_down(inventory, hud, b, actions);
            }
            PointerEvent::Up(p, b) => {
                self.cursor = to_gui(p, self.scale);
                self.pointer_up(inventory, hud, b, actions);
            }
            PointerEvent::Scroll(p, amount) => {
                self.cursor = to_gui(p, self.scale);
                self.scroll(amount);
            }
        }
    }

    fn flush_returns(&mut self, inventory: &mut Inventory, actions: &mut Vec<UiAction>) {
        for s in self.pending_return.drain(..) {
            if let Some(rest) = inventory.add(s) {
                actions.push(UiAction::Drop(rest));
            }
        }
    }

    /// Build this frame's draw list.
    pub fn build(
        &mut self,
        out: &mut UiDrawList,
        screen_w: f32,
        screen_h: f32,
        inventory: &Inventory,
        hud: &HudInfo,
    ) {
        self.update_scale(screen_w, screen_h);
        if !self.textures_sent {
            out.upload
                .push((self.font.key.clone(), self.font.atlas.clone()));
            out.upload
                .push((self.icons.key.clone(), self.icons.atlas.clone()));
            self.textures_sent = true;
        }
        self.hud_state.update(inventory, hud);
        if self.screen != Screen::Pause {
            self.tick_furnaces(hud.dt);
        }
        let mut p = Painter::new(out, self.scale);
        let open = self.screen != Screen::None;
        if !self.hide_hud {
            // Under a screen the HUD is drawn darkened, like the world.
            let dim = if open { 0.25 } else { 1.0 };
            self.draw_hud(&mut p, inventory, hud, dim, !open);
        }
        if open {
            self.draw_screen(&mut p, inventory, hud);
        }
        out.dim_world = open;
    }

    /// Display name of an item (for tooltips).
    pub fn item_name(item: ItemId) -> &'static str {
        item.display()
    }
}

#[cfg(test)]
mod tests;
