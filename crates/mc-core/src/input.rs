//! Platform-independent input snapshot, filled by `mc-game` from winit events
//! once per frame and read by gameplay/UI crates.

use std::collections::HashSet;

/// Logical keys the game cares about. `mc-game` maps physical keys to these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Sneak,
    Sprint,
    Inventory,
    Drop,
    Escape,
    /// F3 debug overlay.
    Debug,
    /// F1 hide HUD.
    HideHud,
    /// F2 screenshot.
    Screenshot,
    /// F5 third-person toggle.
    TogglePerspective,
    /// Creative/survival toggle (F4 in this clone).
    ToggleGameMode,
    /// Toggle fly (double tap jump in creative also works).
    ToggleFly,
    Chat,
    Enter,
    Backspace,
    Hotbar(u8),
    /// Shift held (for shift-click in inventories). Same physical key as Sneak.
    Shift,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, Default)]
pub struct InputState {
    /// Keys currently held.
    pub held: HashSet<Key>,
    /// Keys that went down this frame.
    pub pressed: HashSet<Key>,
    /// Keys that went up this frame.
    pub released: HashSet<Key>,
    pub mouse_held: HashSet<MouseButton>,
    pub mouse_pressed: HashSet<MouseButton>,
    pub mouse_released: HashSet<MouseButton>,
    /// Raw mouse motion this frame (pixels), only meaningful while the cursor is grabbed.
    pub mouse_delta: (f32, f32),
    /// Cursor position in window pixels (physical).
    pub cursor_pos: (f32, f32),
    /// Scroll wheel lines this frame (positive = up).
    pub scroll: f32,
    /// Text typed this frame (for chat/sign editing).
    pub typed: String,
    /// Window size in physical pixels.
    pub window_size: (u32, u32),
    /// The cursor is grabbed for mouselook (no screen open).
    pub cursor_grabbed: bool,
    /// Window DPI scale factor (1.0 on standard displays).
    pub scale_factor: f32,
    /// Analog movement (x = strafe right, y = forward), each -1..1. Added to
    /// the WASD direction; reserved for touch/gamepad controls in the web port.
    pub move_axis: (f32, f32),
}

impl InputState {
    pub fn held(&self, k: Key) -> bool {
        self.held.contains(&k)
    }
    pub fn pressed(&self, k: Key) -> bool {
        self.pressed.contains(&k)
    }
    pub fn mouse_held(&self, b: MouseButton) -> bool {
        self.mouse_held.contains(&b)
    }
    pub fn mouse_pressed(&self, b: MouseButton) -> bool {
        self.mouse_pressed.contains(&b)
    }
    /// Clear per-frame edges. Called by `mc-game` after the frame is processed.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.mouse_pressed.clear();
        self.mouse_released.clear();
        self.mouse_delta = (0.0, 0.0);
        self.scroll = 0.0;
        self.typed.clear();
    }
    /// Consume a key press so later systems don't also act on it.
    pub fn consume(&mut self, k: Key) -> bool {
        self.pressed.remove(&k)
    }
    pub fn consume_mouse(&mut self, b: MouseButton) -> bool {
        self.mouse_pressed.remove(&b)
    }
}
