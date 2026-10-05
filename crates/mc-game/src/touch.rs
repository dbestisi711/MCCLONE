//! On-screen touch controls (phones/tablets in the web build).
//!
//! Layout, in physical pixels with `u = min(w, h) / 9`:
//! - left 40% of the screen below the top quarter: a floating joystick that
//!   drives `InputState::move_axis` (centred where the finger lands);
//! - the rest of the screen: drag to look, tap to place/use (or hit the mob
//!   in front), press and hold to break blocks;
//! - buttons: jump and sneak (bottom right), inventory and pause (top right);
//! - taps on the hotbar select a slot.
//!
//! While a screen (inventory, pause...) is open, touches act as the mouse so
//! the UI's normal click handling works. Controls switch on with the first
//! touch, so desktop play is unaffected.

use std::sync::Arc;

use glam::Vec2;
use mc_core::Rgba8Image;
use mc_core::input::{Key, MouseButton};
use mc_core::render_types::{TextureKey, UiDrawList, UiQuad};
use rustc_hash::FxHashMap as HashMap;
use web_time::Instant;

use crate::game::Game;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Jump,
    Sneak,
    Inventory,
    Pause,
}

const BUTTONS: [Button; 4] = [
    Button::Jump,
    Button::Sneak,
    Button::Inventory,
    Button::Pause,
];

#[derive(Clone, Copy, Debug)]
enum Role {
    Joystick {
        origin: Vec2,
        current: Vec2,
    },
    Look {
        last: Vec2,
        started: Instant,
        moved: f32,
        breaking: bool,
    },
    Button(Button),
    /// Acting as the mouse on an open screen.
    Pointer,
    /// Hotbar tap (already handled) or ignored touch.
    Inert,
}

/// Seconds a still finger must stay down before it starts breaking.
const HOLD_TO_BREAK: f32 = 0.3;
/// Look sensitivity relative to mouse pixels.
const LOOK_SCALE: f32 = 1.4;

pub struct TouchControls {
    /// Set by the first touch; controls are drawn and applied from then on.
    pub active: bool,
    touches: HashMap<u64, Role>,
    size: Vec2,
    texture_sent: bool,
}

impl Default for TouchControls {
    fn default() -> Self {
        Self::new()
    }
}

impl TouchControls {
    pub fn new() -> Self {
        TouchControls {
            active: false,
            touches: HashMap::default(),
            size: Vec2::new(1280.0, 720.0),
            texture_sent: false,
        }
    }

    fn unit(&self) -> f32 {
        self.size.x.min(self.size.y) / 9.0
    }

    /// Centre and radius of a button.
    fn button_circle(&self, b: Button) -> (Vec2, f32) {
        let u = self.unit();
        let (w, h) = (self.size.x, self.size.y);
        match b {
            Button::Jump => (Vec2::new(w - 1.5 * u, h - 2.6 * u), 0.85 * u),
            Button::Sneak => (Vec2::new(w - 3.3 * u, h - 1.5 * u), 0.7 * u),
            Button::Inventory => (Vec2::new(w - 0.9 * u, 0.9 * u), 0.55 * u),
            Button::Pause => (Vec2::new(w - 2.2 * u, 0.9 * u), 0.55 * u),
        }
    }

    fn button_at(&self, p: Vec2) -> Option<Button> {
        BUTTONS.into_iter().find(|&b| {
            let (c, r) = self.button_circle(b);
            p.distance(c) <= r * 1.15
        })
    }

    fn in_joystick_zone(&self, p: Vec2) -> bool {
        p.x < self.size.x * 0.4 && p.y > self.size.y * 0.25
    }

    fn joystick_radius(&self) -> f32 {
        1.3 * self.unit()
    }

    /// Handle one touch event (positions in physical window pixels).
    pub fn event(&mut self, game: &mut Game, id: u64, phase: Phase, pos: Vec2) {
        self.active = true;
        let (w, h) = game.input.window_size;
        self.size = Vec2::new(w.max(1) as f32, h.max(1) as f32);
        match phase {
            Phase::Started => self.start(game, id, pos),
            Phase::Moved => self.moved(game, id, pos),
            Phase::Ended | Phase::Cancelled => self.end(game, id, pos, phase == Phase::Ended),
        }
    }

    fn start(&mut self, game: &mut Game, id: u64, pos: Vec2) {
        let input = &mut game.input;
        let role = if let Some(b) = self.button_at(pos) {
            match b {
                Button::Jump => {
                    input.held.insert(Key::Jump);
                    input.pressed.insert(Key::Jump);
                }
                Button::Sneak => {
                    input.held.insert(Key::Sneak);
                    input.pressed.insert(Key::Sneak);
                }
                Button::Inventory => {
                    input.pressed.insert(Key::Inventory);
                }
                Button::Pause => {
                    input.pressed.insert(Key::Escape);
                }
            }
            Role::Button(b)
        } else if game.ui.screen_open() {
            input.cursor_pos = (pos.x, pos.y);
            input.mouse_held.insert(MouseButton::Left);
            input.mouse_pressed.insert(MouseButton::Left);
            Role::Pointer
        } else if let Some(slot) = game.ui.hotbar_slot_at(pos) {
            game.player.inventory.selected = slot;
            Role::Inert
        } else if self.in_joystick_zone(pos) {
            Role::Joystick {
                origin: pos,
                current: pos,
            }
        } else {
            Role::Look {
                last: pos,
                started: Instant::now(),
                moved: 0.0,
                breaking: false,
            }
        };
        self.touches.insert(id, role);
    }

    fn moved(&mut self, game: &mut Game, id: u64, pos: Vec2) {
        let Some(role) = self.touches.get_mut(&id) else {
            return;
        };
        match role {
            Role::Joystick { current, .. } => *current = pos,
            Role::Look { last, moved, .. } => {
                let d = pos - *last;
                *moved += d.length();
                *last = pos;
                game.input.mouse_delta.0 += d.x * LOOK_SCALE;
                game.input.mouse_delta.1 += d.y * LOOK_SCALE;
            }
            Role::Pointer => game.input.cursor_pos = (pos.x, pos.y),
            Role::Button(_) | Role::Inert => {}
        }
    }

    fn end(&mut self, game: &mut Game, id: u64, pos: Vec2, completed: bool) {
        let Some(role) = self.touches.remove(&id) else {
            return;
        };
        let input = &mut game.input;
        match role {
            Role::Joystick { .. } => {}
            Role::Button(Button::Jump) => {
                input.held.remove(&Key::Jump);
            }
            Role::Button(Button::Sneak) => {
                input.held.remove(&Key::Sneak);
            }
            Role::Button(_) | Role::Inert => {}
            Role::Pointer => {
                input.cursor_pos = (pos.x, pos.y);
                input.mouse_held.remove(&MouseButton::Left);
                input.mouse_released.insert(MouseButton::Left);
            }
            Role::Look {
                started,
                moved,
                breaking,
                ..
            } => {
                if breaking {
                    input.mouse_held.remove(&MouseButton::Left);
                    input.mouse_released.insert(MouseButton::Left);
                } else if completed
                    && started.elapsed().as_secs_f32() < HOLD_TO_BREAK
                    && moved < 0.35 * self.unit()
                {
                    // Tap: hit the mob in front, otherwise use/place.
                    let eye = game.player.eye_position(1.0);
                    let dir = game.player.look_dir();
                    let button = if game.entities.raycast(eye, dir, 3.0).is_some() {
                        MouseButton::Left
                    } else {
                        MouseButton::Right
                    };
                    input.mouse_pressed.insert(button);
                    input.mouse_released.insert(button);
                }
            }
        }
    }

    /// Per-frame update before `Game::frame`: joystick axis and hold-to-break.
    pub fn update(&mut self, game: &mut Game) {
        if !self.active {
            return;
        }
        let (w, h) = game.input.window_size;
        self.size = Vec2::new(w.max(1) as f32, h.max(1) as f32);
        let radius = self.joystick_radius();
        let unit = self.unit();
        let screen_open = game.ui.screen_open();
        let mut axis = Vec2::ZERO;
        for role in self.touches.values_mut() {
            match role {
                Role::Joystick { origin, current } => {
                    let d = (*current - *origin) / radius;
                    axis = if d.length() > 1.0 { d.normalize() } else { d };
                }
                Role::Look {
                    started,
                    moved,
                    breaking,
                    ..
                } => {
                    if !*breaking
                        && !screen_open
                        && *moved < 0.35 * unit
                        && started.elapsed().as_secs_f32() >= HOLD_TO_BREAK
                    {
                        *breaking = true;
                        game.input.mouse_held.insert(MouseButton::Left);
                        game.input.mouse_pressed.insert(MouseButton::Left);
                    }
                }
                _ => {}
            }
        }
        // Screen y grows downward; forward is up on the stick.
        game.input.move_axis = if screen_open {
            (0.0, 0.0)
        } else {
            (axis.x, -axis.y)
        };
        // Pushing the stick to the rim sprints.
        if axis.length() > 0.95 && axis.y < -0.7 && !screen_open {
            game.input.held.insert(Key::Sprint);
        } else if !self
            .touches
            .values()
            .any(|r| matches!(r, Role::Joystick { .. }))
            || axis.length() < 0.9
        {
            game.input.held.remove(&Key::Sprint);
        }
    }

    /// Draw the controls on top of the UI.
    pub fn draw(&mut self, out: &mut UiDrawList, screen_open: bool) {
        if !self.active {
            return;
        }
        let circle = TextureKey::new("@touch_circle");
        if !self.texture_sent {
            out.upload
                .push((circle.clone(), Arc::new(circle_texture(64))));
            self.texture_sent = true;
        }
        let disc = |out: &mut UiDrawList, c: Vec2, r: f32, color: [f32; 4]| {
            out.push(UiQuad::rect(
                circle.clone(),
                c.x - r,
                c.y - r,
                2.0 * r,
                2.0 * r,
                [0.0, 0.0, 1.0, 1.0],
                color,
            ));
        };
        let pressed = |b: Button| {
            self.touches
                .values()
                .any(|r| matches!(r, Role::Button(x) if *x == b))
        };
        let base = [1.0, 1.0, 1.0, 0.18];
        let active = [1.0, 1.0, 1.0, 0.38];
        let icon = [1.0, 1.0, 1.0, 0.85];
        let u = self.unit();

        for b in BUTTONS {
            if screen_open && matches!(b, Button::Jump | Button::Sneak) {
                continue;
            }
            let (c, r) = self.button_circle(b);
            disc(out, c, r, if pressed(b) { active } else { base });
            let s = r * 0.45;
            match b {
                Button::Jump => chevron(out, c, s, true, icon),
                Button::Sneak => chevron(out, c, s, false, icon),
                Button::Inventory => {
                    for i in -1..=1 {
                        let p = c + Vec2::new(i as f32 * s * 0.8, 0.0);
                        disc(out, p, s * 0.22, icon);
                    }
                }
                Button::Pause => {
                    for dx in [-0.35, 0.35] {
                        out.push(UiQuad::solid(
                            c.x + dx * s - s * 0.15,
                            c.y - s * 0.6,
                            s * 0.3,
                            s * 1.2,
                            icon,
                        ));
                    }
                }
            }
        }
        if screen_open {
            return;
        }
        // Joystick: base + knob where the finger is, or a faint hint.
        let radius = self.joystick_radius();
        let stick = self.touches.values().find_map(|r| match r {
            Role::Joystick { origin, current } => Some((*origin, *current)),
            _ => None,
        });
        match stick {
            Some((o, cur)) => {
                let d = cur - o;
                let knob = o + if d.length() > radius {
                    d.normalize() * radius
                } else {
                    d
                };
                disc(out, o, radius, base);
                disc(out, knob, 0.55 * u, active);
            }
            None => {
                let o = Vec2::new(2.2 * u, self.size.y - 2.4 * u);
                disc(out, o, radius, [1.0, 1.0, 1.0, 0.08]);
                disc(out, o, 0.55 * u, [1.0, 1.0, 1.0, 0.14]);
            }
        }
    }
}

/// A chevron (up or down) made of two thick bars.
fn chevron(out: &mut UiDrawList, c: Vec2, s: f32, up: bool, color: [f32; 4]) {
    let dir = if up { -1.0 } else { 1.0 };
    let tip = c + Vec2::new(0.0, dir * s * 0.55);
    let thick = s * 0.28;
    for side in [-1.0f32, 1.0] {
        let end = c + Vec2::new(side * s * 0.9, -dir * s * 0.35);
        let along = (end - tip).normalize();
        let n = Vec2::new(-along.y, along.x) * thick * 0.5;
        out.push(UiQuad {
            texture: TextureKey::white(),
            pos: [tip + n, end + n, end - n, tip - n],
            uv: [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            layer: 0,
            color,
        });
    }
}

/// White disc with a soft edge, used for all touch controls.
fn circle_texture(size: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(size, size);
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = Vec2::new(x as f32 - c, y as f32 - c).length() / c;
            let a = ((1.0 - d) * size as f32 * 0.25).clamp(0.0, 1.0);
            img.put(x, y, [255, 255, 255, (a * 255.0) as u8]);
        }
    }
    img
}
