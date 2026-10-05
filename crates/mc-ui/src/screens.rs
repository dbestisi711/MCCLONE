//! Screens: survival inventory, crafting table, creative inventory, pause
//! menu and death screen. One [`Layout`] per frame drives both drawing and
//! hit-testing, so input and visuals always agree.

use glam::Vec2;
use mc_core::{Inventory, ItemStack};

use crate::container::{ClickButton, Container, Drag, SlotId};
use crate::creative::INVENTORY_TAB;
use crate::gui::{Color, Painter, WHITE, argb, rgb};
use crate::{HudInfo, Screen, Ui, UiAction};

/// Panel label colour (dark grey, no shadow).
const LABEL: Color = rgb(0x404040);
/// Creative palette: visible rows.
const PALETTE_ROWS: usize = 5;
const PALETTE_COLS: usize = 9;
/// Max seconds between the clicks of a double click.
const DOUBLE_CLICK: f32 = 0.3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ButtonId {
    Resume,
    Quit,
    Respawn,
}

#[derive(Clone, Debug)]
pub(crate) struct Button {
    pub id: ButtonId,
    pub rect: [f32; 4],
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SlotPos {
    pub id: SlotId,
    /// Top-left of the 16×16 item area, GUI pixels (absolute).
    pub x: f32,
    pub y: f32,
    /// Draw a larger frame (crafting result).
    pub big: bool,
}

#[derive(Default)]
pub(crate) struct Layout {
    /// Panel rect (absolute GUI pixels), if the screen has one.
    pub panel: Option<[f32; 4]>,
    pub slots: Vec<SlotPos>,
    pub buttons: Vec<Button>,
    /// Creative tabs: (tab index, rect).
    pub tabs: Vec<(usize, [f32; 4])>,
    /// Creative scrollbar track.
    pub scrollbar: Option<[f32; 4]>,
    /// Decorative empty slot frames (item-area top-left), e.g. the unused
    /// cells of the creative palette.
    pub frames: Vec<(f32, f32)>,
}

fn contains(r: [f32; 4], p: Vec2) -> bool {
    p.x >= r[0] && p.y >= r[1] && p.x < r[0] + r[2] && p.y < r[1] + r[3]
}

impl Layout {
    pub fn slot_at(&self, p: Vec2) -> Option<SlotPos> {
        self.slots.iter().copied().find(|s| {
            let pad = if s.big { 4.0 } else { 1.0 };
            contains(
                [s.x - pad, s.y - pad, 16.0 + 2.0 * pad, 16.0 + 2.0 * pad],
                p,
            )
        })
    }

    pub fn button_at(&self, p: Vec2) -> Option<ButtonId> {
        self.buttons
            .iter()
            .find(|b| contains(b.rect, p))
            .map(|b| b.id)
    }

    /// Is `p` over any part of the screen's UI (panel or tabs)?
    pub fn inside(&self, p: Vec2) -> bool {
        self.panel.is_some_and(|r| contains(r, p)) || self.tabs.iter().any(|(_, r)| contains(*r, p))
    }
}

impl Ui {
    fn palette_rows(&self) -> usize {
        self.tabs
            .get(self.creative_tab)
            .map(|t| t.items.len().div_ceil(PALETTE_COLS))
            .unwrap_or(0)
    }

    fn max_scroll(&self) -> usize {
        self.palette_rows().saturating_sub(PALETTE_ROWS)
    }

    pub(crate) fn layout(&self, hud: &HudInfo) -> Layout {
        let (gw, gh) = (self.gui_size.x, self.gui_size.y);
        let mut l = Layout::default();
        let center = |w: f32, h: f32| [((gw - w) / 2.0).floor(), ((gh - h) / 2.0).floor(), w, h];
        let player_rows = |l: &mut Layout, x0: f32, y0: f32, main_y: f32, hot_y: f32| {
            for r in 0..3 {
                for c in 0..9 {
                    l.slots.push(SlotPos {
                        id: SlotId::Inv((9 + r * 9 + c) as u8),
                        x: x0 + 8.0 + 18.0 * c as f32,
                        y: y0 + main_y + 18.0 * r as f32,
                        big: false,
                    });
                }
            }
            for c in 0..9 {
                l.slots.push(SlotPos {
                    id: SlotId::Inv(c as u8),
                    x: x0 + 8.0 + 18.0 * c as f32,
                    y: y0 + hot_y,
                    big: false,
                });
            }
        };
        let slot = |id, x, y| SlotPos {
            id,
            x,
            y,
            big: false,
        };
        match self.screen {
            Screen::Inventory => {
                let r = center(176.0, 166.0);
                let (x0, y0) = (r[0], r[1]);
                l.panel = Some(r);
                for i in 0..4 {
                    l.slots
                        .push(slot(SlotId::Armor(i), x0 + 8.0, y0 + 8.0 + 18.0 * i as f32));
                }
                l.slots.push(slot(SlotId::Offhand, x0 + 77.0, y0 + 62.0));
                for i in 0..4u8 {
                    let (c, rr) = ((i % 2) as f32, (i / 2) as f32);
                    l.slots.push(slot(
                        SlotId::Craft(i),
                        x0 + 92.0 + 18.0 * c,
                        y0 + 18.0 + 18.0 * rr,
                    ));
                }
                l.slots.push(SlotPos {
                    id: SlotId::Result,
                    x: x0 + 152.0,
                    y: y0 + 28.0,
                    big: true,
                });
                player_rows(&mut l, x0, y0, 84.0, 142.0);
            }
            Screen::Furnace => {
                let r = center(176.0, 166.0);
                let (x0, y0) = (r[0], r[1]);
                l.panel = Some(r);
                l.slots
                    .push(slot(SlotId::FurnaceInput, x0 + 56.0, y0 + 17.0));
                l.slots
                    .push(slot(SlotId::FurnaceFuel, x0 + 56.0, y0 + 53.0));
                l.slots.push(SlotPos {
                    id: SlotId::FurnaceOutput,
                    x: x0 + 116.0,
                    y: y0 + 35.0,
                    big: true,
                });
                player_rows(&mut l, x0, y0, 84.0, 142.0);
            }
            Screen::CraftingTable => {
                let r = center(176.0, 166.0);
                let (x0, y0) = (r[0], r[1]);
                l.panel = Some(r);
                for i in 0..9u8 {
                    let (c, rr) = ((i % 3) as f32, (i / 3) as f32);
                    l.slots.push(slot(
                        SlotId::Craft(i),
                        x0 + 30.0 + 18.0 * c,
                        y0 + 17.0 + 18.0 * rr,
                    ));
                }
                l.slots.push(SlotPos {
                    id: SlotId::Result,
                    x: x0 + 124.0,
                    y: y0 + 35.0,
                    big: true,
                });
                player_rows(&mut l, x0, y0, 84.0, 142.0);
            }
            Screen::CreativeInventory => {
                let r = center(195.0, 136.0);
                let (x0, mut y0) = (r[0], r[1]);
                // Leave room for the tabs above the panel.
                y0 = y0.max(26.0);
                l.panel = Some([x0, y0, 195.0, 136.0]);
                for (i, _) in self.tabs.iter().enumerate() {
                    l.tabs
                        .push((i, [x0 + 27.0 * i as f32, y0 - 24.0, 26.0, 26.0]));
                }
                let inv_tab = self.creative_tab == INVENTORY_TAB;
                if inv_tab {
                    for i in 0..4 {
                        l.slots.push(slot(
                            SlotId::Armor(i),
                            x0 + 9.0 + 18.0 * i as f32,
                            y0 + 20.0,
                        ));
                    }
                    l.slots
                        .push(slot(SlotId::Offhand, x0 + 9.0 + 18.0 * 5.0, y0 + 20.0));
                    for rr in 0..3 {
                        for c in 0..9 {
                            l.slots.push(slot(
                                SlotId::Inv((9 + rr * 9 + c) as u8),
                                x0 + 9.0 + 18.0 * c as f32,
                                y0 + 54.0 + 18.0 * rr as f32,
                            ));
                        }
                    }
                } else if let Some(tab) = self.tabs.get(self.creative_tab) {
                    let first = self.creative_scroll.min(self.max_scroll()) * PALETTE_COLS;
                    for n in 0..PALETTE_ROWS * PALETTE_COLS {
                        let (c, rr) = ((n % PALETTE_COLS) as f32, (n / PALETTE_COLS) as f32);
                        l.frames.push((x0 + 9.0 + 18.0 * c, y0 + 18.0 + 18.0 * rr));
                    }
                    for (n, &item) in tab
                        .items
                        .iter()
                        .skip(first)
                        .take(PALETTE_ROWS * PALETTE_COLS)
                        .enumerate()
                    {
                        let (c, rr) = ((n % PALETTE_COLS) as f32, (n / PALETTE_COLS) as f32);
                        l.slots.push(slot(
                            SlotId::Palette(item),
                            x0 + 9.0 + 18.0 * c,
                            y0 + 18.0 + 18.0 * rr,
                        ));
                    }
                    l.scrollbar = Some([
                        x0 + 175.0,
                        y0 + 18.0,
                        12.0,
                        18.0 * PALETTE_ROWS as f32 - 2.0,
                    ]);
                }
                for c in 0..9 {
                    l.slots.push(slot(
                        SlotId::Inv(c as u8),
                        x0 + 9.0 + 18.0 * c as f32,
                        y0 + 112.0,
                    ));
                }
                l.slots.push(slot(SlotId::Trash, x0 + 173.0, y0 + 112.0));
            }
            Screen::Pause => {
                let x = (gw / 2.0 - 100.0).floor();
                let y = (gh / 4.0).floor();
                l.buttons.push(Button {
                    id: ButtonId::Resume,
                    rect: [x, y + 32.0, 200.0, 20.0],
                    label: "Back to Game",
                });
                l.buttons.push(Button {
                    id: ButtonId::Quit,
                    rect: [x, y + 56.0, 200.0, 20.0],
                    label: "Quit Game",
                });
            }
            Screen::Death => {
                let x = (gw / 2.0 - 100.0).floor();
                let y = (gh / 4.0).floor();
                l.buttons.push(Button {
                    id: ButtonId::Respawn,
                    rect: [x, y + 72.0, 200.0, 20.0],
                    label: "Respawn",
                });
                l.buttons.push(Button {
                    id: ButtonId::Quit,
                    rect: [x, y + 96.0, 200.0, 20.0],
                    label: "Quit Game",
                });
            }
            Screen::None => {}
        }
        let _ = hud;
        l
    }

    pub(crate) fn hovered_slot(&self, _inv: &Inventory, hud: &HudInfo) -> Option<SlotId> {
        self.layout(hud).slot_at(self.cursor).map(|s| s.id)
    }

    pub(crate) fn scroll(&mut self, amount: f32) {
        if self.screen == Screen::CreativeInventory && amount != 0.0 {
            let max = self.max_scroll() as i32;
            let n = self.creative_scroll as i32 - amount.signum() as i32;
            self.creative_scroll = n.clamp(0, max) as usize;
        }
    }

    fn scroll_to_cursor(&mut self, track: [f32; 4]) {
        let max = self.max_scroll();
        if max == 0 {
            self.creative_scroll = 0;
            return;
        }
        let t = ((self.cursor.y - track[1] - 7.5) / (track[3] - 15.0)).clamp(0.0, 1.0);
        self.creative_scroll = (t * max as f32).round() as usize;
    }

    pub(crate) fn pointer_move(&mut self, inv: &mut Inventory, hud: &HudInfo) {
        let lay = self.layout(hud);
        if self.scroll_drag
            && let Some(track) = lay.scrollbar
        {
            self.scroll_to_cursor(track);
        }
        if let Some(mut drag) = self.drag.take() {
            if let Some(s) = lay.slot_at(self.cursor)
                && self.container.drag_accepts(inv, &drag, s.id)
            {
                drag.slots.push(s.id);
            }
            self.drag = Some(drag);
        }
    }

    pub(crate) fn pointer_down(
        &mut self,
        inv: &mut Inventory,
        hud: &HudInfo,
        button: ClickButton,
        actions: &mut Vec<UiAction>,
    ) {
        let lay = self.layout(hud);
        if let Some(b) = lay.button_at(self.cursor) {
            if button == ClickButton::Primary {
                self.pressed_button = Some(b);
                self.press_button(b, actions);
            }
            return;
        }
        if !self.screen.has_slots() || self.drag.is_some() {
            return;
        }
        if let Some((tab, _)) = lay.tabs.iter().find(|(_, r)| contains(*r, self.cursor)) {
            if self.creative_tab != *tab {
                self.creative_tab = *tab;
                self.creative_scroll = 0;
            }
            return;
        }
        if let Some(track) = lay.scrollbar
            && contains(
                [
                    track[0] - 1.0,
                    track[1] - 1.0,
                    track[2] + 2.0,
                    track[3] + 2.0,
                ],
                self.cursor,
            )
        {
            self.scroll_drag = true;
            self.scroll_to_cursor(track);
            return;
        }
        let creative = hud.creative || self.screen == Screen::CreativeInventory;
        if let Some(s) = lay.slot_at(self.cursor) {
            // Double click with a stack on the cursor collects matching items.
            let now = self.hud_state.time;
            let double = button == ClickButton::Primary
                && !self.shift
                && self.container.carried.is_some()
                && self
                    .last_click
                    .is_some_and(|(id, t)| id == s.id && now > t && now - t < DOUBLE_CLICK);
            self.last_click = (button == ClickButton::Primary).then_some((s.id, now));
            if double {
                self.last_click = None;
                self.container.collect(inv);
                return;
            }
            // Carrying a stack: start a drag that may spread it over slots.
            if self.container.carried.is_some() && !self.shift && button != ClickButton::Middle {
                let drag = Drag {
                    button,
                    slots: Vec::new(),
                };
                if self.container.drag_accepts(inv, &drag, s.id) {
                    self.drag = Some(Drag {
                        button,
                        slots: vec![s.id],
                    });
                    return;
                }
            }
            self.container
                .click(inv, &self.recipes, s.id, button, self.shift, creative);
            return;
        }
        if !lay.inside(self.cursor)
            && let Some(d) = self.container.click_outside(button)
        {
            actions.push(UiAction::Drop(d));
        }
    }

    pub(crate) fn pointer_up(
        &mut self,
        inv: &mut Inventory,
        hud: &HudInfo,
        button: ClickButton,
        _actions: &mut Vec<UiAction>,
    ) {
        if button == ClickButton::Primary {
            self.scroll_drag = false;
            self.pressed_button = None;
        }
        let creative = hud.creative || self.screen == Screen::CreativeInventory;
        if let Some(drag) = self.drag.take() {
            if drag.button != button {
                self.drag = Some(drag);
                return;
            }
            if drag.slots.len() <= 1 {
                if let Some(&s) = drag.slots.first() {
                    self.container
                        .click(inv, &self.recipes, s, button, false, creative);
                }
            } else {
                self.container.finish_drag(inv, &drag);
            }
        }
    }

    fn press_button(&mut self, b: ButtonId, actions: &mut Vec<UiAction>) {
        match b {
            ButtonId::Resume => self.close(),
            ButtonId::Quit => actions.push(UiAction::Quit),
            ButtonId::Respawn => {
                actions.push(UiAction::Respawn);
                self.respawn_requested = true;
                self.close();
            }
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    pub(crate) fn draw_screen(&self, p: &mut Painter, inv: &Inventory, hud: &HudInfo) {
        let lay = self.layout(hud);
        let (gw, gh) = (self.gui_size.x, self.gui_size.y);
        match self.screen {
            Screen::Pause => {
                self.text_centered(
                    p,
                    "Game Menu",
                    gw / 2.0,
                    (gh / 4.0).floor() + 8.0,
                    WHITE,
                    1.0,
                );
            }
            Screen::Death => {
                p.gradient(0.0, 0.0, gw, gh, argb(0x6050_0000), argb(0xA080_3030));
                self.text_centered(
                    p,
                    "You died!",
                    gw / 2.0,
                    (gh / 4.0).floor() + 8.0,
                    WHITE,
                    2.0,
                );
            }
            Screen::Inventory
            | Screen::CraftingTable
            | Screen::CreativeInventory
            | Screen::Furnace => {
                self.draw_container(p, inv, hud, &lay);
            }
            Screen::None => {}
        }
        for b in &lay.buttons {
            self.draw_button(p, b);
        }
    }

    fn text_centered(&self, p: &mut Painter, t: &str, cx: f32, y: f32, color: Color, size: f32) {
        let w = self.font.width(t, p.scale) * size;
        self.font
            .draw(p, t, (cx - w / 2.0).round(), y, color, true, size);
    }

    fn draw_button(&self, p: &mut Painter, b: &Button) {
        let hovered = contains(b.rect, self.cursor);
        let n = if self.pressed_button == Some(b.id) {
            &self.skin.button_pressed
        } else if hovered {
            &self.skin.button_hover
        } else {
            &self.skin.button
        };
        let [x, y, w, h] = b.rect;
        // The pack's button art is light; darken it towards the classic grey
        // so white labels stay readable.
        p.nine(n, x, y, w, h, [0.62, 0.62, 0.62, 1.0]);
        let color = if hovered {
            rgb(0xFFFFA0)
        } else {
            rgb(0xE0E0E0)
        };
        let cap = self.font.cap_height(p.scale);
        let ty = (y + (h - cap) / 2.0).round();
        let tw = self.font.width(b.label, p.scale);
        self.font.draw(
            p,
            b.label,
            (x + (w - tw) / 2.0).round(),
            ty,
            color,
            true,
            1.0,
        );
    }

    fn draw_container(&self, p: &mut Painter, inv: &Inventory, hud: &HudInfo, lay: &Layout) {
        let Some([x0, y0, pw, ph]) = lay.panel else {
            return;
        };
        let creative = self.screen == Screen::CreativeInventory;
        // Creative: background tabs go behind the panel.
        if creative {
            for &(i, r) in &lay.tabs {
                if i != self.creative_tab {
                    self.draw_tab(p, i, r, false);
                }
            }
        }
        p.nine(&self.skin.panel, x0, y0, pw, ph, WHITE);
        if creative && let Some(&(i, r)) = lay.tabs.iter().find(|(i, _)| *i == self.creative_tab) {
            self.draw_tab(p, i, r, true);
        }

        // Labels and decorations.
        match self.screen {
            Screen::Inventory => {
                self.draw_doll(p, x0 + 26.0, y0 + 8.0);
                self.font
                    .draw(p, "Crafting", x0 + 91.0, y0 + 7.0, LABEL, false, 1.0);
                self.draw_arrow(p, &self.skin.arrow_small, x0 + 130.0, y0 + 30.0);
            }
            Screen::CraftingTable => {
                self.font
                    .draw(p, "Crafting", x0 + 28.0, y0 + 6.0, LABEL, false, 1.0);
                self.font
                    .draw(p, "Inventory", x0 + 8.0, y0 + 73.0, LABEL, false, 1.0);
                self.draw_arrow(p, &self.skin.arrow, x0 + 92.0, y0 + 35.0);
            }
            Screen::Furnace => {
                let w = self.font.width("Furnace", p.scale);
                let tx = (x0 + (pw - w) / 2.0).round();
                self.font
                    .draw(p, "Furnace", tx, y0 + 6.0, LABEL, false, 1.0);
                self.font
                    .draw(p, "Inventory", x0 + 8.0, y0 + 73.0, LABEL, false, 1.0);
                let f = self.container.furnace.unwrap_or_default();
                self.draw_furnace_gauges(p, &f, x0, y0);
            }
            Screen::CreativeInventory => {
                let name = self
                    .tabs
                    .get(self.creative_tab)
                    .map(|t| t.name)
                    .unwrap_or("");
                self.font
                    .draw(p, name, x0 + 8.0, y0 + 6.0, LABEL, false, 1.0);
                if let Some(track) = lay.scrollbar {
                    self.draw_scrollbar(p, track);
                }
            }
            _ => {}
        }

        // Slots.
        for &(x, y) in &lay.frames {
            p.nine(&self.skin.slot, x - 1.0, y - 1.0, 18.0, 18.0, WHITE);
        }
        let hovered = lay.slot_at(self.cursor);
        let drag_share = self
            .drag
            .as_ref()
            .filter(|d| d.slots.len() > 1)
            .map(|d| (d, self.container.drag_share(d)));
        for s in &lay.slots {
            self.draw_slot_frame(p, s, inv);
            let mut stack = self.container.get(inv, &self.recipes, s.id);
            let mut dragged = false;
            if let Some((d, share)) = &drag_share
                && d.slots.contains(&s.id)
                && let Some(c) = self.container.carried
            {
                let have = stack.map(|x| x.count).unwrap_or(0);
                let limit = Container::slot_limit(s.id, c.item);
                stack = Some(ItemStack {
                    count: (have + share).min(limit),
                    ..c
                });
                dragged = true;
            }
            if dragged {
                p.solid(s.x, s.y, 16.0, 16.0, [1.0, 1.0, 1.0, 0.3]);
            }
            if let Some(st) = stack {
                self.icons.draw_stack(p, &self.font, &st, s.x, s.y);
            }
            if hovered.is_some_and(|h| h.id == s.id) {
                p.solid(s.x, s.y, 16.0, 16.0, [1.0, 1.0, 1.0, 0.45]);
            }
        }

        // Carried stack follows the cursor.
        if let Some(mut c) = self.container.carried {
            if let Some((d, share)) = &drag_share {
                let placed: u32 = d
                    .slots
                    .iter()
                    .map(|&s| {
                        let have = self
                            .container
                            .get(inv, &self.recipes, s)
                            .map(|x| x.count)
                            .unwrap_or(0);
                        let limit = Container::slot_limit(s, c.item);
                        (*share).min(limit.saturating_sub(have)) as u32
                    })
                    .sum();
                c.count = c.count.saturating_sub(placed.min(255) as u8);
            }
            let (cx, cy) = (self.cursor.x - 8.0, self.cursor.y - 8.0);
            if c.count > 0 {
                self.icons.draw_stack(p, &self.font, &c, cx, cy);
            } else {
                self.icons
                    .draw(p, c.item, cx, cy, 16.0, [1.0, 1.0, 1.0, 0.5]);
            }
        } else if let Some(h) = hovered {
            // Tooltip.
            if let Some(st) = self.container.get(inv, &self.recipes, h.id) {
                self.draw_tooltip(p, &st);
            } else if h.id == SlotId::Trash {
                self.draw_tooltip_text(p, &["Destroy Item"]);
            }
        } else if let Some((i, _)) = lay.tabs.iter().find(|(_, r)| contains(*r, self.cursor))
            && let Some(tab) = self.tabs.get(*i)
        {
            self.draw_tooltip_text(p, &[tab.name]);
        }
        let _ = hud;
    }

    fn draw_slot_frame(&self, p: &mut Painter, s: &SlotPos, _inv: &Inventory) {
        let (x, y) = (s.x, s.y);
        if s.big {
            p.nine(&self.skin.slot, x - 4.0, y - 4.0, 24.0, 24.0, WHITE);
        } else {
            p.nine(&self.skin.slot, x - 1.0, y - 1.0, 18.0, 18.0, WHITE);
        }
        // Placeholder icons for empty equipment slots.
        let empty_icon = match s.id {
            SlotId::Armor(i) => self.skin.empty_armor.get(i as usize),
            SlotId::Offhand => Some(&self.skin.empty_offhand),
            SlotId::Trash => Some(&self.skin.trash),
            _ => None,
        };
        if let Some(icon) = empty_icon {
            let empty = match s.id {
                SlotId::Armor(i) => _inv.armor[i as usize].is_none(),
                SlotId::Offhand => _inv.offhand.is_none(),
                _ => true,
            };
            if empty && icon.ok {
                let (w, h) = (icon.width().min(16.0), icon.height().min(16.0));
                p.sprite(
                    icon,
                    x + ((16.0 - w) / 2.0).floor(),
                    y + ((16.0 - h) / 2.0).floor(),
                    w,
                    h,
                    [0.25, 0.25, 0.25, 0.6],
                );
            }
        }
    }

    /// Flame (fuel left) and progress arrow of the furnace screen.
    fn draw_furnace_gauges(&self, p: &mut Painter, f: &crate::furnace::Furnace, x0: f32, y0: f32) {
        let s = &self.skin;
        let (fx, fy) = (x0 + 57.0, y0 + 37.0);
        p.sprite_at(&s.flame_empty, fx, fy, WHITE);
        if f.burning() {
            // The flame shrinks from the top as fuel burns down.
            let h = s.flame_full.height();
            let shown = (h * f.burn_fraction()).ceil();
            p.sprite_part(
                &s.flame_full,
                [0.0, h - shown, s.flame_full.width(), shown],
                [fx, fy + h - shown, s.flame_full.width(), shown],
                WHITE,
            );
        }
        let (ax, ay) = (x0 + 79.0, y0 + 34.0);
        p.sprite_at(&s.arrow_inactive, ax, ay, WHITE);
        let w = (s.arrow_active.width() * f.cook_fraction()).round();
        if w > 0.0 {
            p.sprite_part(
                &s.arrow_active,
                [0.0, 0.0, w, s.arrow_active.height()],
                [ax, ay, w, s.arrow_active.height()],
                WHITE,
            );
        }
    }

    fn draw_arrow(&self, p: &mut Painter, a: &crate::gui::Sprite, x: f32, y: f32) {
        if a.ok {
            p.sprite_at(a, x, y, WHITE);
        } else {
            p.solid(x, y + 6.0, 14.0, 3.0, rgb(0x8B8B8B));
        }
    }

    fn draw_tab(&self, p: &mut Painter, i: usize, r: [f32; 4], front: bool) {
        let [x, y, w, h] = r;
        let n = if front {
            &self.skin.tab_front
        } else {
            &self.skin.tab_back
        };
        // Front tab extends down into the panel so they join seamlessly.
        let hh = if front { h + 4.0 } else { h };
        p.nine(n, x, y, w, hh, WHITE);
        if let Some(tab) = self.tabs.get(i) {
            let tint = if front { WHITE } else { [0.8, 0.8, 0.8, 1.0] };
            self.icons
                .draw(p, tab.icon, x + (w - 16.0) / 2.0, y + 5.0, 16.0, tint);
        }
        if front {
            // Hide the panel's top border under the active tab.
            p.solid(x + 2.0, y + h - 1.0, w - 4.0, 4.0, rgb(0xC6C6C6));
        }
    }

    fn draw_scrollbar(&self, p: &mut Painter, track: [f32; 4]) {
        let [x, y, w, h] = track;
        p.nine(&self.skin.slot, x, y, w, h, WHITE);
        let max = self.max_scroll();
        let t = if max == 0 {
            0.0
        } else {
            self.creative_scroll as f32 / max as f32
        };
        let hy = (y + 1.0 + (h - 17.0) * t).round();
        let color = if max == 0 {
            [0.6, 0.6, 0.6, 1.0]
        } else {
            WHITE
        };
        p.nine(&self.skin.button, x + 1.0, hy, w - 2.0, 15.0, color);
    }

    fn draw_tooltip(&self, p: &mut Painter, st: &ItemStack) {
        let mut lines = vec![st.item.display().to_string()];
        if let mc_core::item::ItemKind::Tool { durability, .. } = st.item.kind()
            && st.damage > 0
        {
            lines.push(format!(
                "§7Durability: {} / {}",
                durability.saturating_sub(st.damage),
                durability
            ));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        self.draw_tooltip_text(p, &refs);
    }

    fn draw_tooltip_text(&self, p: &mut Painter, lines: &[&str]) {
        let lh = self.font.line_height(p.scale).max(10.0);
        let cap = self.font.cap_height(p.scale);
        let w = lines
            .iter()
            .map(|l| self.font.width(l, p.scale))
            .fold(0.0, f32::max)
            .ceil();
        let h = cap + lh * (lines.len() as f32 - 1.0);
        let (gw, gh) = (self.gui_size.x, self.gui_size.y);
        let mut x = (self.cursor.x + 12.0).round();
        let mut y = (self.cursor.y - 12.0).round();
        if x + w + 4.0 > gw {
            x = (self.cursor.x - 16.0 - w).round().max(4.0);
        }
        y = y.clamp(4.0, (gh - h - 4.0).max(4.0));
        // Dark box with a soft coloured border.
        let bg = argb(0xF010_0010);
        p.solid(x - 3.0, y - 4.0, w + 6.0, 1.0, bg);
        p.solid(x - 3.0, y + h + 3.0, w + 6.0, 1.0, bg);
        p.solid(x - 3.0, y - 3.0, w + 6.0, h + 6.0, bg);
        p.solid(x - 4.0, y - 3.0, 1.0, h + 6.0, bg);
        p.solid(x + w + 3.0, y - 3.0, 1.0, h + 6.0, bg);
        p.gradient(
            x - 3.0,
            y - 2.0,
            1.0,
            h + 4.0,
            argb(0x5050_00FF),
            argb(0x5028_007F),
        );
        p.gradient(
            x + w + 2.0,
            y - 2.0,
            1.0,
            h + 4.0,
            argb(0x5050_00FF),
            argb(0x5028_007F),
        );
        p.solid(x - 3.0, y - 3.0, w + 6.0, 1.0, argb(0x5050_00FF));
        p.solid(x - 3.0, y + h + 2.0, w + 6.0, 1.0, argb(0x5028_007F));
        for (i, l) in lines.iter().enumerate() {
            self.font.draw(p, l, x, y + i as f32 * lh, WHITE, true, 1.0);
        }
    }

    /// Flat front view of the player skin (head, body, arms, legs and their
    /// overlay layers) inside a dark frame.
    fn draw_doll(&self, p: &mut Painter, x: f32, y: f32) {
        p.nine(&self.skin.slot, x - 1.0, y - 1.0, 51.0, 72.0, WHITE);
        p.solid(x, y, 49.0, 70.0, rgb(0x000000));
        let s = &self.skin.skin;
        if !s.ok {
            return;
        }
        let k: f32 = 2.0; // GUI px per skin texel
        let ox = x + ((49.0 - 16.0 * k) / 2.0).floor();
        let oy = y + ((70.0 - 32.0 * k) / 2.0).floor();
        let two_layer = s.height() >= 64.0;
        // (src x, src y, w, h, dst x, dst y) in skin texels.
        let mut parts: Vec<[f32; 6]> = vec![
            [8.0, 8.0, 8.0, 8.0, 4.0, 0.0],    // head
            [20.0, 20.0, 8.0, 12.0, 4.0, 8.0], // body
            [44.0, 20.0, 4.0, 12.0, 0.0, 8.0], // right arm (viewer's left)
            [4.0, 20.0, 4.0, 12.0, 4.0, 20.0], // right leg
        ];
        if two_layer {
            parts.push([36.0, 52.0, 4.0, 12.0, 12.0, 8.0]); // left arm
            parts.push([20.0, 52.0, 4.0, 12.0, 8.0, 20.0]); // left leg
            parts.push([40.0, 8.0, 8.0, 8.0, 4.0, 0.0]); // hat
            parts.push([20.0, 36.0, 8.0, 12.0, 4.0, 8.0]); // jacket
            parts.push([44.0, 36.0, 4.0, 12.0, 0.0, 8.0]); // right sleeve
            parts.push([52.0, 52.0, 4.0, 12.0, 12.0, 8.0]); // left sleeve
            parts.push([4.0, 36.0, 4.0, 12.0, 4.0, 20.0]); // right pants
            parts.push([4.0, 52.0, 4.0, 12.0, 8.0, 20.0]); // left pants
        } else {
            parts.push([44.0, 20.0, 4.0, 12.0, 12.0, 8.0]);
            parts.push([4.0, 20.0, 4.0, 12.0, 8.0, 20.0]);
        }
        for [sx, sy, w, h, dx, dy] in parts {
            p.sprite_part(
                s,
                [sx, sy, w, h],
                [ox + dx * k, oy + dy * k, w * k, h * k],
                WHITE,
            );
        }
    }
}
