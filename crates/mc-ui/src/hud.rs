//! In-game HUD: crosshair, hotbar, hearts, hunger, armor, air, XP, the
//! selected item's name and the F3 debug overlay.

use mc_core::{Inventory, ItemId};

use crate::gui::{Color, Painter, Sprite, WHITE, argb, rgb, shade, with_alpha};
use crate::{HudInfo, Ui};

/// How long the selected item's name stays up (seconds).
const NAME_TIME: f32 = 2.0;
const NAME_FADE_IN: f32 = 0.12;
const NAME_FADE_OUT: f32 = 0.5;
/// Heart containers flash for this long after taking damage.
const BLINK_TIME: f32 = 1.0;

#[derive(Default)]
pub(crate) struct HudState {
    pub time: f32,
    last_sel: Option<(usize, Option<ItemId>)>,
    pub name_timer: f32,
    pub name_item: Option<ItemId>,
    prev_health: Option<f32>,
    pub blink_timer: f32,
    pub blink_from: f32,
}

impl HudState {
    pub fn update(&mut self, inv: &Inventory, hud: &HudInfo) {
        let dt = hud.dt.max(0.0);
        self.time += dt;
        // Selected item name.
        let sel = (inv.selected, inv.selected_item());
        if let Some(last) = self.last_sel {
            if last != sel {
                self.name_item = sel.1;
                self.name_timer = if sel.1.is_some() { NAME_TIME } else { 0.0 };
            }
        }
        self.last_sel = Some(sel);
        self.name_timer = (self.name_timer - dt).max(0.0);
        // Damage blink.
        if let Some(prev) = self.prev_health {
            if hud.health < prev {
                self.blink_timer = BLINK_TIME;
                self.blink_from = self.blink_from.max(prev);
            }
        }
        self.prev_health = Some(hud.health);
        self.blink_timer = (self.blink_timer - dt).max(0.0);
        if self.blink_timer == 0.0 {
            self.blink_from = 0.0;
        }
    }

    fn name_alpha(&self) -> f32 {
        if self.name_timer <= 0.0 {
            return 0.0;
        }
        let age = NAME_TIME - self.name_timer;
        (age / NAME_FADE_IN)
            .min(self.name_timer / NAME_FADE_OUT)
            .clamp(0.0, 1.0)
    }

    /// Containers are highlighted in alternating 0.15 s phases while blinking.
    fn blink_on(&self) -> bool {
        self.blink_timer > 0.0 && ((self.blink_timer / 0.15) as i32) % 2 == 0
    }
}

/// Small deterministic hash for per-heart jitter.
fn jitter(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x % 3) as f32 - 1.0
}

impl Ui {
    pub(crate) fn draw_hud(
        &self,
        p: &mut Painter,
        inv: &Inventory,
        hud: &HudInfo,
        dim: f32,
        crosshair: bool,
    ) {
        let (gw, gh) = (self.gui_size.x, self.gui_size.y);
        let c = |col: Color| shade(col, dim);
        let cx = (gw / 2.0).floor();

        if hud.show_debug {
            self.draw_debug(p, hud, dim);
        }
        if crosshair {
            self.draw_crosshair(p, gw, gh);
        }

        // Hotbar.
        let hx = cx - 91.0;
        let hy = gh - 22.0;
        let s = &self.skin;
        if s.hotbar.iter().all(|h| h.ok) && s.hotbar_start.ok && s.hotbar_end.ok {
            p.sprite_at(&s.hotbar_start, hx, hy, c(WHITE));
            for (i, h) in s.hotbar.iter().enumerate() {
                p.sprite(h, hx + 1.0 + 20.0 * i as f32, hy, 20.0, 22.0, c(WHITE));
            }
            p.sprite_at(&s.hotbar_end, hx + 181.0, hy, c(WHITE));
        } else {
            p.bevel(hx, hy, 182.0, 22.0, self.skin.slot.fallback, c(WHITE));
        }
        let sel = inv.selected.min(8) as f32;
        if s.hotbar_selected.ok {
            p.sprite(
                &s.hotbar_selected,
                hx - 1.0 + sel * 20.0,
                hy - 1.0,
                24.0,
                24.0,
                c(WHITE),
            );
        } else {
            p.solid(
                hx - 1.0 + sel * 20.0,
                hy - 1.0,
                24.0,
                24.0,
                c([1.0, 1.0, 1.0, 0.5]),
            );
        }
        // Offhand slot to the left of the hotbar when it holds something.
        if let Some(off) = inv.offhand {
            let ox = hx - 29.0;
            if s.hotbar[0].ok {
                p.sprite_at(&s.hotbar_start, ox, hy, c(WHITE));
                p.sprite(&s.hotbar[0], ox + 1.0, hy, 20.0, 22.0, c(WHITE));
                p.sprite_at(&s.hotbar_end, ox + 21.0, hy, c(WHITE));
            }
            self.draw_stack_dim(p, &off, ox + 3.0, hy + 3.0, dim);
        }
        for i in 0..9 {
            if let Some(st) = inv.slots[i] {
                self.draw_stack_dim(p, &st, hx + 3.0 + 20.0 * i as f32, hy + 3.0, dim);
            }
        }

        if !hud.creative {
            self.draw_status_bars(p, hud, hx, gh, dim);
        }

        // Selected item name (not while a screen covers the HUD).
        let a = self.hud_state.name_alpha();
        if let (Some(item), true) = (self.hud_state.name_item, a > 0.0 && dim >= 1.0) {
            let name = item.display();
            let w = self.font.width(name, p.scale);
            let y = if hud.creative { gh - 45.0 } else { gh - 59.0 };
            self.font.draw(
                p,
                name,
                (cx - w / 2.0).round(),
                y,
                c(with_alpha(WHITE, a)),
                true,
                1.0,
            );
        }
    }

    fn draw_stack_dim(&self, p: &mut Painter, st: &mc_core::ItemStack, x: f32, y: f32, dim: f32) {
        if dim < 1.0 {
            self.icons
                .draw(p, st.item, x, y, 16.0, [dim, dim, dim, 1.0]);
        } else {
            self.icons.draw_stack(p, &self.font, st, x, y);
        }
    }

    fn draw_crosshair(&self, p: &mut Painter, gw: f32, gh: f32) {
        let s = &self.skin.crosshair;
        let x = (gw / 2.0).floor() - 7.0;
        let y = (gh / 2.0).floor() - 7.0;
        if s.ok {
            // A thin dark outline keeps the white cross readable on bright
            // backgrounds (approximates the original's inverted blending).
            let o = 1.0 / p.scale;
            for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o)] {
                p.sprite_at(s, x + dx, y + dy, [0.0, 0.0, 0.0, 0.35]);
            }
            p.sprite_at(s, x, y, [1.0, 1.0, 1.0, 0.9]);
        } else {
            p.solid(x, y + 7.0, 15.0, 1.0, WHITE);
            p.solid(x + 7.0, y, 1.0, 15.0, WHITE);
        }
    }

    fn draw_status_bars(&self, p: &mut Painter, hud: &HudInfo, hx: f32, gh: f32, dim: f32) {
        let s = &self.skin;
        let c = |col: Color| shade(col, dim);
        let tick = (self.hud_state.time * 20.0) as u32;

        // XP bar and level.
        let xy = gh - 29.0;
        p.nine(&s.xp_empty, hx, xy, 182.0, 5.0, c(WHITE));
        let prog = hud.xp_progress.clamp(0.0, 1.0);
        if prog > 0.0 {
            let saved = p.clip;
            p.clip = Some([hx, xy, hx + (182.0 * prog).round().max(1.0), xy + 5.0]);
            p.nine(&s.xp_full, hx, xy, 182.0, 5.0, c(WHITE));
            p.clip = saved;
        }
        if hud.xp_level > 0 {
            let t = hud.xp_level.to_string();
            let w = self.font.width(&t, p.scale);
            let cap = self.font.cap_height(p.scale);
            let x = (hx + 91.0 - w / 2.0).round();
            self.font.draw_outlined(
                p,
                &t,
                x,
                gh - 28.0 - cap,
                c(rgb(0x80FF20)),
                c(rgb(0x000000)),
            );
        }

        // Hearts.
        let max_hp = hud.max_health.max(0.0);
        let hearts = (max_hp / 2.0).ceil() as usize;
        let rows = hearts.div_ceil(10).max(1);
        let row_step = (10.0 - (rows as f32 - 2.0)).clamp(3.0, 10.0);
        let hy = gh - 39.0;
        let hp = hud.health.max(0.0).ceil() as i32;
        let blink = self.hud_state.blink_on();
        let blink_hp = self.hud_state.blink_from.ceil() as i32;
        for i in 0..hearts {
            let x = hx + (i % 10) as f32 * 8.0;
            let mut y = hy - (i / 10) as f32 * row_step;
            if hp <= 4 && hud.health > 0.0 {
                y += jitter(tick.wrapping_mul(31).wrapping_add(i as u32));
            }
            let bg = if blink { &s.heart_blink } else { &s.heart_bg };
            p.sprite_at(bg, x, y, c(WHITE));
            let v = 2 * i as i32 + 1; // half-heart index
            if blink && v <= blink_hp && v > hp {
                // Health just lost flashes white.
                let spr = if v == blink_hp && blink_hp % 2 == 1 {
                    &s.heart_flash_half
                } else {
                    &s.heart_flash
                };
                p.sprite_at(spr, x, y, c(WHITE));
            }
            if v < hp {
                p.sprite_at(&s.heart_full, x, y, c(WHITE));
            } else if v == hp {
                p.sprite_at(&s.heart_half, x, y, c(WHITE));
            }
        }

        // Armor above the hearts.
        if hud.armor > 0 {
            let ay = hy - rows as f32 * row_step;
            let armor = hud.armor.min(20) as i32;
            for i in 0..10 {
                let x = hx + i as f32 * 8.0;
                let v = 2 * i + 1;
                let spr = if v < armor {
                    &s.armor_full
                } else if v == armor {
                    &s.armor_half
                } else {
                    &s.armor_empty
                };
                p.sprite_at(spr, x, ay, c(WHITE));
            }
        }

        // Hunger, right to left.
        let food = hud.food.round().clamp(0.0, 20.0) as i32;
        let right = hx + 182.0;
        for i in 0..10 {
            let x = right - 9.0 - i as f32 * 8.0;
            let mut y = hy;
            if food == 0 {
                y += jitter(tick.wrapping_mul(17).wrapping_add(100 + i as u32));
            }
            p.sprite_at(&s.hunger_bg, x, y, c(WHITE));
            let v = 2 * i + 1;
            if v < food {
                p.sprite_at(&s.hunger_full, x, y, c(WHITE));
            } else if v == food {
                p.sprite_at(&s.hunger_half, x, y, c(WHITE));
            }
        }

        // Air bubbles above the hunger bar while underwater.
        if hud.max_air > 0 && hud.air < hud.max_air {
            let air = hud.air.max(0) as f32;
            let max = hud.max_air as f32;
            let full = (((air - 2.0) * 10.0 / max).ceil()).max(0.0) as i32;
            let popping = ((air * 10.0 / max).ceil() as i32 - full).max(0);
            let by = hy - 10.0;
            for i in 0..(full + popping).min(10) {
                let x = right - 9.0 - i as f32 * 8.0;
                let spr: &Sprite = if i < full { &s.bubble } else { &s.bubble_pop };
                p.sprite_at(spr, x, by, c(WHITE));
            }
        }
    }

    fn draw_debug(&self, p: &mut Painter, hud: &HudInfo, dim: f32) {
        let lh = self.font.line_height(p.scale).max(9.0);
        let cap = self.font.cap_height(p.scale);
        let pad = ((lh - cap) / 2.0).floor();
        let bg = shade(argb(0x9050_5050), dim);
        let fg = shade(rgb(0xE0E0E0), dim);
        for (i, line) in hud.debug_lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let y = 2.0 + i as f32 * lh;
            let w = self.font.width(line, p.scale);
            p.solid(1.0, y - 1.0, w + 3.0, lh, bg);
            self.font.draw(p, line, 2.0, y - 1.0 + pad, fg, false, 1.0);
        }
        // Right column.
        let mut right = vec![format!("GUI scale: {}", p.scale)];
        if let Some(t) = &hud.target {
            right.push(format!("Targeted block: {t}"));
        }
        let gw = self.gui_size.x;
        for (i, line) in right.iter().enumerate() {
            let y = 2.0 + i as f32 * lh;
            let w = self.font.width(line, p.scale);
            let x = gw - 2.0 - w;
            p.solid(x - 1.0, y - 1.0, w + 3.0, lh, bg);
            self.font.draw(p, line, x, y - 1.0 + pad, fg, false, 1.0);
        }
    }
}
