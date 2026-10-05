//! HUD, inventory screens, crafting, text rendering.
//!
//! OWNER: inventory & UI agent. Public API used by `mc-game` (keep stable,
//! add freely):
//! - [`Ui::new`], [`Ui::handle_input`], [`Ui::build`], [`Ui::screen_open`]
//!
//! The UI never touches the GPU: it emits a `UiDrawList` of textured quads in
//! physical pixels which `mc-render` draws on top of the world.

use mc_core::Inventory;
use mc_core::input::{InputState, Key};
use mc_core::render_types::{UiDrawList, UiQuad};

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
    Inventory,
    CraftingTable,
    Pause,
    Death,
}

pub struct Ui {
    pub screen: Screen,
    /// GUI scale factor (each GUI pixel = `scale` physical pixels).
    pub scale: f32,
    pub hide_hud: bool,
}

impl Ui {
    pub fn new(_assets: &mc_assets::Assets) -> Self {
        Ui {
            screen: Screen::None,
            scale: 2.0,
            hide_hud: false,
        }
    }

    pub fn screen_open(&self) -> bool {
        self.screen != Screen::None
    }

    /// Open a screen (e.g. crafting table when the player right-clicks one).
    pub fn open(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// Process input for open screens and the hotbar. Consumes the keys/clicks
    /// it handles so the game doesn't also act on them.
    pub fn handle_input(
        &mut self,
        input: &mut InputState,
        inventory: &mut Inventory,
        _hud: &HudInfo,
    ) -> Vec<UiAction> {
        if input.consume(Key::Inventory) {
            self.screen = if self.screen_open() {
                Screen::None
            } else {
                Screen::Inventory
            };
        }
        if input.consume(Key::Escape) {
            self.screen = if self.screen_open() {
                Screen::None
            } else {
                Screen::Pause
            };
        }
        for i in 0..9u8 {
            if input.consume(Key::Hotbar(i)) {
                inventory.selected = i as usize;
            }
        }
        if input.scroll != 0.0 && !self.screen_open() {
            let n = inventory.selected as i32 - input.scroll.signum() as i32;
            inventory.selected = n.rem_euclid(9) as usize;
        }
        Vec::new()
    }

    /// Build this frame's draw list.
    pub fn build(
        &mut self,
        out: &mut UiDrawList,
        screen_w: f32,
        screen_h: f32,
        inventory: &Inventory,
        _hud: &HudInfo,
    ) {
        // Crosshair.
        let (cx, cy) = (screen_w / 2.0, screen_h / 2.0);
        out.push(UiQuad::solid(
            cx - 9.0,
            cy - 1.0,
            18.0,
            2.0,
            [1.0, 1.0, 1.0, 0.8],
        ));
        out.push(UiQuad::solid(
            cx - 1.0,
            cy - 9.0,
            2.0,
            18.0,
            [1.0, 1.0, 1.0, 0.8],
        ));
        // Placeholder hotbar.
        let slot = 20.0 * self.scale;
        let x0 = cx - slot * 4.5;
        let y0 = screen_h - slot - 4.0;
        for i in 0..9 {
            let c = if i == inventory.selected {
                [1.0, 1.0, 1.0, 0.6]
            } else {
                [0.0, 0.0, 0.0, 0.5]
            };
            out.push(UiQuad::solid(
                x0 + i as f32 * slot + 2.0,
                y0 + 2.0,
                slot - 4.0,
                slot - 4.0,
                c,
            ));
        }
        out.dim_world = self.screen_open();
    }
}
