//! Physical key → logical [`Key`] mapping.

use mc_core::input::{InputState, Key, MouseButton};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

fn map(code: KeyCode) -> &'static [Key] {
    use Key::*;
    match code {
        KeyCode::KeyW => &[Forward],
        KeyCode::KeyS => &[Back],
        KeyCode::KeyA => &[Left],
        KeyCode::KeyD => &[Right],
        KeyCode::Space => &[Jump],
        KeyCode::ShiftLeft | KeyCode::ShiftRight => &[Sneak, Shift],
        KeyCode::ControlLeft | KeyCode::ControlRight => &[Sprint],
        KeyCode::KeyE => &[Inventory],
        KeyCode::KeyQ => &[Drop],
        KeyCode::Escape => &[Escape],
        KeyCode::F1 => &[HideHud],
        KeyCode::F2 => &[Screenshot],
        KeyCode::F3 => &[Debug],
        KeyCode::F4 => &[ToggleGameMode],
        KeyCode::F5 => &[TogglePerspective],
        KeyCode::KeyF => &[ToggleFly],
        KeyCode::KeyT => &[Chat],
        KeyCode::Enter => &[Enter],
        KeyCode::Backspace => &[Backspace],
        KeyCode::Digit1 => &[Hotbar(0)],
        KeyCode::Digit2 => &[Hotbar(1)],
        KeyCode::Digit3 => &[Hotbar(2)],
        KeyCode::Digit4 => &[Hotbar(3)],
        KeyCode::Digit5 => &[Hotbar(4)],
        KeyCode::Digit6 => &[Hotbar(5)],
        KeyCode::Digit7 => &[Hotbar(6)],
        KeyCode::Digit8 => &[Hotbar(7)],
        KeyCode::Digit9 => &[Hotbar(8)],
        _ => &[],
    }
}

pub fn key_event(input: &mut InputState, e: &KeyEvent) {
    let PhysicalKey::Code(code) = e.physical_key else {
        return;
    };
    for &k in map(code) {
        match e.state {
            ElementState::Pressed => {
                if !e.repeat {
                    input.pressed.insert(k);
                }
                input.held.insert(k);
            }
            ElementState::Released => {
                input.held.remove(&k);
                input.released.insert(k);
            }
        }
    }
}

pub fn mouse_button(b: winit::event::MouseButton) -> Option<MouseButton> {
    match b {
        winit::event::MouseButton::Left => Some(MouseButton::Left),
        winit::event::MouseButton::Right => Some(MouseButton::Right),
        winit::event::MouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

/// `+` / `-` (main row or keypad) change the render distance.
pub fn render_distance_delta(e: &KeyEvent) -> Option<i32> {
    match e.physical_key {
        PhysicalKey::Code(KeyCode::Equal | KeyCode::NumpadAdd) => Some(1),
        PhysicalKey::Code(KeyCode::Minus | KeyCode::NumpadSubtract) => Some(-1),
        _ => None,
    }
}
