//! Ui-level tests: screen flow, input routing and draw-list smoke tests.

use std::sync::OnceLock;

use glam::Vec2;
use mc_assets::{Assets, Pack};
use mc_core::input::{InputState, Key, MouseButton};
use mc_core::render_types::UiDrawList;
use mc_core::{Inventory, ItemId, ItemStack};

use super::*;

fn assets() -> &'static Assets {
    static A: OnceLock<Assets> = OnceLock::new();
    A.get_or_init(|| Assets::load(Pack::find().expect("resource pack")))
}

fn input(w: u32, h: u32) -> InputState {
    InputState {
        window_size: (w, h),
        scale_factor: 1.0,
        ..Default::default()
    }
}

fn st(n: &str, c: u8) -> ItemStack {
    ItemStack::new(ItemId::by_name(n).unwrap(), c)
}

fn press(inp: &mut InputState, k: Key) {
    inp.pressed.insert(k);
}

fn hud() -> HudInfo {
    HudInfo {
        health: 20.0,
        max_health: 20.0,
        food: 20.0,
        air: 300,
        max_air: 300,
        ..Default::default()
    }
}

/// Physical position of a slot's centre.
fn slot_center(ui: &Ui, id: SlotId) -> (f32, f32) {
    let l = ui.layout();
    let s = l.slots.iter().find(|s| s.id == id).expect("slot on screen");
    ((s.x + 8.0) * ui.scale, (s.y + 8.0) * ui.scale)
}

fn click(
    ui: &mut Ui,
    inv: &mut Inventory,
    hud: &HudInfo,
    pos: (f32, f32),
    b: MouseButton,
) -> Vec<UiAction> {
    let mut i = input(1280, 720);
    i.cursor_pos = pos;
    i.mouse_pressed.insert(b);
    i.mouse_held.insert(b);
    let mut a = ui.handle_input(&mut i, inv, hud);
    let mut i = input(1280, 720);
    i.cursor_pos = pos;
    i.mouse_released.insert(b);
    a.extend(ui.handle_input(&mut i, inv, hud));
    a
}

fn click_slot(
    ui: &mut Ui,
    inv: &mut Inventory,
    hud: &HudInfo,
    id: SlotId,
    b: MouseButton,
) -> Vec<UiAction> {
    let pos = slot_center(ui, id);
    click(ui, inv, hud, pos, b)
}

#[test]
fn e_toggles_inventory_and_escape_pauses() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = hud();
    let mut i = input(1280, 720);
    press(&mut i, Key::Inventory);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::Inventory);
    assert!(!i.pressed(Key::Inventory), "key consumed");
    let mut i = input(1280, 720);
    press(&mut i, Key::Inventory);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::None);
    let mut i = input(1280, 720);
    press(&mut i, Key::Escape);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::Pause);
    // E does nothing in the pause menu; Esc resumes.
    let mut i = input(1280, 720);
    press(&mut i, Key::Inventory);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::Pause);
    let mut i = input(1280, 720);
    press(&mut i, Key::Escape);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::None);
    // Creative players get the creative inventory.
    let ch = HudInfo {
        creative: true,
        ..hud()
    };
    let mut i = input(1280, 720);
    press(&mut i, Key::Inventory);
    ui.handle_input(&mut i, &mut inv, &ch);
    assert_eq!(ui.screen, Screen::CreativeInventory);
}

#[test]
fn hotbar_hit_test() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    ui.handle_input(&mut input(1280, 720), &mut inv, &hud());
    // Scale 3: hotbar spans x 367..913, y 654..720 in physical pixels.
    assert_eq!(ui.hotbar_slot_at(Vec2::new(640.0, 700.0)), Some(4));
    assert_eq!(ui.hotbar_slot_at(Vec2::new(372.0, 700.0)), Some(0));
    assert_eq!(ui.hotbar_slot_at(Vec2::new(905.0, 700.0)), Some(8));
    assert_eq!(ui.hotbar_slot_at(Vec2::new(640.0, 600.0)), None);
}

#[test]
fn hotbar_keys_and_scroll() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = hud();
    let mut i = input(1280, 720);
    press(&mut i, Key::Hotbar(4));
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(inv.selected, 4);
    let mut i = input(1280, 720);
    i.scroll = -1.0;
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(inv.selected, 5);
    let mut i = input(1280, 720);
    i.scroll = 1.0;
    inv.selected = 0;
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(inv.selected, 8);
}

#[test]
fn mouse_crafting_flow() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    inv.slots[0] = Some(st("oak_log", 3));
    let h = hud();
    ui.open(Screen::Inventory);
    let mut i = input(1280, 720);
    ui.handle_input(&mut i, &mut inv, &h);
    // Pick up the logs, put one in the crafting grid with a right click.
    let a = click_slot(&mut ui, &mut inv, &h, SlotId::Inv(0), MouseButton::Left);
    assert!(a.is_empty());
    assert_eq!(ui.carried(), Some(st("oak_log", 3)));
    click_slot(&mut ui, &mut inv, &h, SlotId::Craft(0), MouseButton::Right);
    assert_eq!(ui.container.craft[0], Some(st("oak_log", 1)));
    assert_eq!(ui.carried(), Some(st("oak_log", 2)));
    // Put the rest back.
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(0), MouseButton::Left);
    assert_eq!(inv.slots[0], Some(st("oak_log", 2)));
    // Shift-click the result.
    let mut i = input(1280, 720);
    i.held.insert(Key::Shift);
    i.cursor_pos = slot_center(&ui, SlotId::Result);
    i.mouse_pressed.insert(MouseButton::Left);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(inv.count_of(ItemId::by_name("oak_planks").unwrap()), 4);
    assert_eq!(ui.container.craft[0], None);
}

#[test]
fn closing_returns_items_and_outside_click_drops() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = hud();
    ui.open(Screen::CraftingTable);
    ui.container.craft[4] = Some(st("stick", 2));
    ui.container.carried = Some(st("dirt", 5));
    // Right click far outside the panel drops one.
    let a = click(&mut ui, &mut inv, &h, (5.0, 5.0), MouseButton::Right);
    assert_eq!(a, vec![UiAction::Drop(st("dirt", 1))]);
    let mut i = input(1280, 720);
    press(&mut i, Key::Escape);
    ui.handle_input(&mut i, &mut inv, &h);
    assert_eq!(ui.screen, Screen::None);
    assert_eq!(inv.count_of(ItemId::by_name("dirt").unwrap()), 4);
    assert_eq!(inv.count_of(ItemId::by_name("stick").unwrap()), 2);
    assert!(!ui.container.holds_items());
}

#[test]
fn drag_spreads_stack() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = hud();
    ui.open(Screen::Inventory);
    ui.handle_input(&mut input(1280, 720), &mut inv, &h);
    ui.container.carried = Some(st("dirt", 9));
    let targets = [SlotId::Inv(9), SlotId::Inv(10), SlotId::Inv(11)];
    let p0 = slot_center(&ui, targets[0]);
    let mut i = input(1280, 720);
    i.cursor_pos = p0;
    i.mouse_pressed.insert(MouseButton::Left);
    i.mouse_held.insert(MouseButton::Left);
    ui.handle_input(&mut i, &mut inv, &h);
    for t in &targets[1..] {
        let mut i = input(1280, 720);
        i.cursor_pos = slot_center(&ui, *t);
        i.mouse_held.insert(MouseButton::Left);
        ui.handle_input(&mut i, &mut inv, &h);
    }
    let mut i = input(1280, 720);
    i.cursor_pos = slot_center(&ui, targets[2]);
    i.mouse_released.insert(MouseButton::Left);
    ui.handle_input(&mut i, &mut inv, &h);
    for t in [9, 10, 11] {
        assert_eq!(inv.slots[t], Some(st("dirt", 3)));
    }
    assert_eq!(ui.carried(), None);
}

#[test]
fn pause_and_death_buttons() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = hud();
    ui.open(Screen::Pause);
    ui.handle_input(&mut input(1280, 720), &mut inv, &h);
    let l = ui.layout();
    let quit = l
        .buttons
        .iter()
        .find(|b| b.id == screens::ButtonId::Quit)
        .unwrap()
        .rect;
    let pos = ((quit[0] + 5.0) * ui.scale, (quit[1] + 5.0) * ui.scale);
    let mut i = input(1280, 720);
    ui.handle_input(&mut i, &mut inv, &h);
    let a = click(&mut ui, &mut inv, &h, pos, MouseButton::Left);
    assert_eq!(a, vec![UiAction::Quit]);

    let dead = HudInfo {
        dead: true,
        ..hud()
    };
    let mut i = input(1280, 720);
    ui.handle_input(&mut i, &mut inv, &dead);
    assert_eq!(ui.screen, Screen::Death);
    // Escape can't leave the death screen.
    let mut i = input(1280, 720);
    press(&mut i, Key::Escape);
    ui.handle_input(&mut i, &mut inv, &dead);
    assert_eq!(ui.screen, Screen::Death);
    let l = ui.layout();
    let r = l
        .buttons
        .iter()
        .find(|b| b.id == screens::ButtonId::Respawn)
        .unwrap()
        .rect;
    let pos = ((r[0] + 5.0) * ui.scale, (r[1] + 5.0) * ui.scale);
    let a = click(&mut ui, &mut inv, &dead, pos, MouseButton::Left);
    assert_eq!(a.first(), Some(&UiAction::Respawn));
    assert_eq!(ui.screen, Screen::None);
}

#[test]
fn creative_tabs_scroll_and_take() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = HudInfo {
        creative: true,
        ..hud()
    };
    ui.open(Screen::CreativeInventory);
    let mut i = input(1280, 720);
    ui.handle_input(&mut i, &mut inv, &h);
    let l = ui.layout();
    let first = l
        .slots
        .iter()
        .find_map(|s| match s.id {
            SlotId::Palette(item) => Some(item),
            _ => None,
        })
        .unwrap();
    click_slot(
        &mut ui,
        &mut inv,
        &h,
        SlotId::Palette(first),
        MouseButton::Left,
    );
    assert_eq!(ui.carried().map(|c| c.item), Some(first));
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(3), MouseButton::Left);
    assert_eq!(inv.slots[3].map(|s| s.item), Some(first));
    // Scrolling moves the palette when there is more than one page.
    let mut i = input(1280, 720);
    i.scroll = -3.0;
    ui.handle_input(&mut i, &mut inv, &h);
    assert!(ui.creative_scroll <= ui.max_scroll_for_tests());
    // Switch tabs by clicking.
    let (tab, r) = ui.layout().tabs[INV_TAB];
    let pos = ((r[0] + 13.0) * ui.scale, (r[1] + 13.0) * ui.scale);
    click(&mut ui, &mut inv, &h, pos, MouseButton::Left);
    assert_eq!(ui.creative_tab, tab);
    assert!(ui.layout().slots.iter().any(|s| s.id == SlotId::Inv(20)));
}

const INV_TAB: usize = creative::INVENTORY_TAB;

impl Ui {
    fn max_scroll_for_tests(&self) -> usize {
        let rows = self.tabs[self.creative_tab].items.len().div_ceil(9);
        rows.saturating_sub(5)
    }
}

#[test]
fn build_emits_quads_and_uploads_once() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    inv.slots[0] = Some(st("stone", 12));
    let h = hud();
    let mut out = UiDrawList::default();
    ui.build(&mut out, 1280.0, 720.0, &inv, &h);
    assert_eq!(ui.scale, 3.0);
    assert_eq!(out.upload.len(), 2);
    assert!(out.quads.len() > 20);
    assert!(!out.dim_world);
    let mut out = UiDrawList::default();
    ui.build(&mut out, 1280.0, 720.0, &inv, &h);
    assert!(out.upload.is_empty());
    for s in [
        Screen::Inventory,
        Screen::CraftingTable,
        Screen::CreativeInventory,
        Screen::Pause,
        Screen::Death,
    ] {
        ui.open(s);
        let mut out = UiDrawList::default();
        ui.build(&mut out, 1280.0, 720.0, &inv, &h);
        assert!(out.dim_world);
        assert!(out.quads.len() > 10, "{s:?}");
    }
}

#[test]
fn software_render_smoke() {
    let a = assets();
    let mut ui = Ui::new(a);
    let inv = Inventory::default();
    let mut out = UiDrawList::default();
    ui.open(Screen::Inventory);
    ui.cursor = Vec2::new(100.0, 100.0);
    ui.build(&mut out, 320.0, 240.0, &inv, &hud());
    let mut img = preview::backdrop(a, 320, 240);
    let before = img.data.clone();
    preview::SoftwareRenderer::new(a).render(&out, &mut img);
    assert_ne!(before, img.data);
}

#[test]
fn furnace_persists_per_block_and_drops_contents() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = HudInfo { dt: 0.0, ..hud() };
    let pos = glam::IVec3::new(3, 64, -2);
    ui.open_furnace(pos);
    assert_eq!(ui.screen, Screen::Furnace);
    ui.handle_input(&mut input(1280, 720), &mut inv, &h);
    inv.slots[0] = Some(st("sand", 2));
    inv.slots[1] = Some(st("coal", 1));
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(0), MouseButton::Left);
    click_slot(
        &mut ui,
        &mut inv,
        &h,
        SlotId::FurnaceInput,
        MouseButton::Left,
    );
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(1), MouseButton::Left);
    click_slot(
        &mut ui,
        &mut inv,
        &h,
        SlotId::FurnaceFuel,
        MouseButton::Left,
    );
    assert_eq!(ui.furnace(pos).unwrap().input, Some(st("sand", 2)));
    // Close: the furnace keeps its items and keeps smelting.
    ui.close();
    ui.handle_input(&mut input(1280, 720), &mut inv, &h);
    assert_eq!(inv.count_of(ItemId::by_name("sand").unwrap()), 0);
    ui.tick_furnaces(furnace::COOK_TIME + 0.5);
    let f = ui.furnace(pos).unwrap();
    assert_eq!(f.output, Some(st("glass", 1)));
    // Another furnace is independent.
    ui.open_furnace(glam::IVec3::ZERO);
    assert_eq!(ui.container().furnace.unwrap().input, None);
    ui.close();
    // Breaking the block returns everything inside.
    let drops = ui.remove_furnace(pos);
    assert!(drops.contains(&st("glass", 1)));
    assert!(drops.contains(&st("sand", 1)));
    assert!(ui.furnace(pos).is_none());
}

#[test]
fn double_click_collects_matching_items() {
    let mut ui = Ui::new(assets());
    let mut inv = Inventory::default();
    let h = HudInfo { dt: 0.05, ..hud() };
    inv.slots[9] = Some(st("dirt", 10));
    inv.slots[20] = Some(st("dirt", 7));
    ui.open(Screen::Inventory);
    ui.handle_input(&mut input(1280, 720), &mut inv, &h);
    let frame = |ui: &mut Ui, inv: &Inventory| {
        let mut out = UiDrawList::default();
        ui.build(&mut out, 1280.0, 720.0, inv, &h);
    };
    frame(&mut ui, &inv);
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(9), MouseButton::Left);
    assert_eq!(ui.carried(), Some(st("dirt", 10)));
    frame(&mut ui, &inv);
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(9), MouseButton::Left);
    assert_eq!(ui.carried(), Some(st("dirt", 17)));
    assert_eq!(inv.slots[20], None);
    // A slow second click just places the stack.
    for _ in 0..20 {
        frame(&mut ui, &inv);
    }
    click_slot(&mut ui, &mut inv, &h, SlotId::Inv(9), MouseButton::Left);
    assert_eq!(inv.slots[9], Some(st("dirt", 17)));
}
