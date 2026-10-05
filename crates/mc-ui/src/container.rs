//! Inventory-screen slot logic, independent of rendering and input devices.
//!
//! A [`Container`] owns the crafting grid and the stack carried on the
//! cursor; the player's [`Inventory`] is passed in. All interactions are
//! expressed as clicks on [`SlotId`]s so the same logic serves mouse and
//! touch input.

use mc_core::item::ItemKind;
use mc_core::{Inventory, ItemId, ItemStack};

use crate::furnace::{Furnace, fuel_time, smelt_result};
use crate::recipes::RecipeBook;

/// Identifies one slot of an open screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SlotId {
    /// Player inventory: `0..9` hotbar, `9..36` main.
    Inv(u8),
    /// Helmet, chestplate, leggings, boots.
    Armor(u8),
    Offhand,
    /// Crafting grid cell, row-major (`craft_w` wide).
    Craft(u8),
    /// Crafting output.
    Result,
    /// Creative palette entry showing this item.
    Palette(ItemId),
    /// Creative "destroy item" slot.
    Trash,
    /// Furnace slots.
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ClickButton {
    /// Left mouse / tap.
    Primary,
    /// Right mouse / long-press.
    Secondary,
    Middle,
}

/// Which menu the container belongs to (affects shift-click routing).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Inventory,
    CraftingTable,
    Creative,
    Furnace,
}

/// An in-progress drag across slots while carrying a stack.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub button: ClickButton,
    pub slots: Vec<SlotId>,
}

pub struct Container {
    pub kind: MenuKind,
    /// Crafting grid (only the first `craft_w * craft_w` cells are used).
    pub craft: [Option<ItemStack>; 9],
    pub craft_w: usize,
    pub carried: Option<ItemStack>,
    /// The open furnace (only for [`MenuKind::Furnace`]).
    pub furnace: Option<Furnace>,
}

const HOTBAR: std::ops::Range<usize> = 0..9;
const MAIN: std::ops::Range<usize> = 9..36;

impl Container {
    pub fn new(kind: MenuKind) -> Self {
        Container {
            kind,
            craft: [None; 9],
            craft_w: if kind == MenuKind::CraftingTable {
                3
            } else {
                2
            },
            carried: None,
            furnace: None,
        }
    }

    /// Switch to another menu (keeps the carried stack; the caller must have
    /// emptied the crafting grid first).
    pub fn set_kind(&mut self, kind: MenuKind) {
        self.kind = kind;
        self.craft_w = if kind == MenuKind::CraftingTable {
            3
        } else {
            2
        };
    }

    pub fn grid(&self) -> &[Option<ItemStack>] {
        &self.craft[..self.craft_w * self.craft_w]
    }

    pub fn result(&self, recipes: &RecipeBook) -> Option<ItemStack> {
        recipes.result(self.grid(), self.craft_w, self.craft_w)
    }

    /// Current contents of a slot (the crafting result is computed).
    pub fn get(&self, inv: &Inventory, recipes: &RecipeBook, s: SlotId) -> Option<ItemStack> {
        match s {
            SlotId::Inv(i) => inv.slots.get(i as usize).copied().flatten(),
            SlotId::Armor(i) => inv.armor.get(i as usize).copied().flatten(),
            SlotId::Offhand => inv.offhand,
            SlotId::Craft(i) => self.craft.get(i as usize).copied().flatten(),
            SlotId::Result => self.result(recipes),
            SlotId::Palette(item) => Some(ItemStack::new(item, 1)),
            SlotId::Trash => None,
            SlotId::FurnaceInput => self.furnace.and_then(|f| f.input),
            SlotId::FurnaceFuel => self.furnace.and_then(|f| f.fuel),
            SlotId::FurnaceOutput => self.furnace.and_then(|f| f.output),
        }
    }

    fn slot_mut<'a>(
        &'a mut self,
        inv: &'a mut Inventory,
        s: SlotId,
    ) -> Option<&'a mut Option<ItemStack>> {
        match s {
            SlotId::Inv(i) => inv.slots.get_mut(i as usize),
            SlotId::Armor(i) => inv.armor.get_mut(i as usize),
            SlotId::Offhand => Some(&mut inv.offhand),
            SlotId::Craft(i) if (i as usize) < self.craft_w * self.craft_w => {
                self.craft.get_mut(i as usize)
            }
            SlotId::FurnaceInput => self.furnace.as_mut().map(|f| &mut f.input),
            SlotId::FurnaceFuel => self.furnace.as_mut().map(|f| &mut f.fuel),
            SlotId::FurnaceOutput => self.furnace.as_mut().map(|f| &mut f.output),
            _ => None,
        }
    }

    /// Max stack size for `item` in slot `s`.
    pub fn slot_limit(s: SlotId, item: ItemId) -> u8 {
        match s {
            SlotId::Armor(_) => 1,
            _ => item.max_stack(),
        }
    }

    /// Can `stack` be placed into slot `s` by the player?
    pub fn accepts(s: SlotId, stack: &ItemStack) -> bool {
        match s {
            SlotId::Armor(i) => {
                matches!(stack.item.kind(), ItemKind::Armor { slot, .. } if slot == i)
            }
            SlotId::FurnaceFuel => fuel_time(stack.item).is_some(),
            SlotId::Result | SlotId::Palette(_) | SlotId::Trash | SlotId::FurnaceOutput => false,
            _ => true,
        }
    }

    /// Left/right/middle click on a slot. `shift` = quick-move.
    pub fn click(
        &mut self,
        inv: &mut Inventory,
        recipes: &RecipeBook,
        s: SlotId,
        button: ClickButton,
        shift: bool,
        creative: bool,
    ) {
        match s {
            SlotId::Result => {
                if shift {
                    self.craft_all(inv, recipes);
                } else if button != ClickButton::Middle {
                    self.take_result(recipes);
                }
            }
            SlotId::Palette(item) => self.click_palette(inv, item, button, shift),
            SlotId::FurnaceOutput if !shift => self.take_output(button),
            SlotId::Trash => {
                if shift {
                    for slot in inv.slots.iter_mut() {
                        *slot = None;
                    }
                } else {
                    self.carried = None;
                }
            }
            _ if shift && button != ClickButton::Middle => self.quick_move(inv, s),
            _ => self.click_slot(inv, s, button, creative),
        }
    }

    fn click_slot(&mut self, inv: &mut Inventory, s: SlotId, button: ClickButton, creative: bool) {
        let mut carried = self.carried;
        let Some(slot) = self.slot_mut(inv, s) else {
            return;
        };
        match (button, carried, *slot) {
            (ClickButton::Primary, None, Some(st)) => {
                carried = Some(st);
                *slot = None;
            }
            (ClickButton::Primary, Some(c), None) => {
                if Self::accepts(s, &c) {
                    let n = c.count.min(Self::slot_limit(s, c.item));
                    *slot = Some(ItemStack { count: n, ..c });
                    carried = take(c, n);
                }
            }
            (ClickButton::Primary, Some(c), Some(st)) => {
                if st.can_stack_with(&c) {
                    let room = Self::slot_limit(s, st.item).saturating_sub(st.count);
                    let n = room.min(c.count);
                    *slot = Some(ItemStack {
                        count: st.count + n,
                        ..st
                    });
                    carried = take(c, n);
                } else if Self::accepts(s, &c) && c.count <= Self::slot_limit(s, c.item) {
                    *slot = Some(c);
                    carried = Some(st);
                }
            }
            (ClickButton::Secondary, None, Some(st)) => {
                let n = st.count.div_ceil(2);
                carried = Some(ItemStack { count: n, ..st });
                *slot = take(st, n);
            }
            (ClickButton::Secondary, Some(c), None) => {
                if Self::accepts(s, &c) {
                    *slot = Some(ItemStack { count: 1, ..c });
                    carried = take(c, 1);
                }
            }
            (ClickButton::Secondary, Some(c), Some(st)) => {
                if st.can_stack_with(&c) {
                    if st.count < Self::slot_limit(s, st.item) {
                        *slot = Some(ItemStack {
                            count: st.count + 1,
                            ..st
                        });
                        carried = take(c, 1);
                    }
                } else if Self::accepts(s, &c) && c.count <= Self::slot_limit(s, c.item) {
                    *slot = Some(c);
                    carried = Some(st);
                }
            }
            (ClickButton::Middle, None, Some(st)) if creative => {
                carried = Some(ItemStack {
                    count: st.item.max_stack(),
                    ..st
                });
            }
            _ => {}
        }
        self.carried = carried;
    }

    /// Creative palette: take a fresh stack, or destroy what is carried.
    fn click_palette(
        &mut self,
        inv: &mut Inventory,
        item: ItemId,
        button: ClickButton,
        shift: bool,
    ) {
        let max = item.max_stack();
        if shift {
            inv.add(ItemStack::new(item, max));
            return;
        }
        match self.carried {
            None => {
                let n = if button == ClickButton::Secondary {
                    1
                } else {
                    max
                };
                self.carried = Some(ItemStack::new(item, n));
            }
            Some(c) if c.item == item && c.damage == 0 && button == ClickButton::Primary => {
                self.carried = Some(ItemStack {
                    count: (c.count + 1).min(max),
                    ..c
                });
            }
            Some(_) => self.carried = None,
        }
    }

    /// Take smelted items: the whole stack (or half with the secondary
    /// button), or as many as fit onto a matching carried stack.
    fn take_output(&mut self, button: ClickButton) {
        let Some(f) = self.furnace.as_mut() else {
            return;
        };
        let Some(out) = f.output else { return };
        match self.carried {
            None => {
                let n = if button == ClickButton::Secondary {
                    out.count.div_ceil(2)
                } else {
                    out.count
                };
                self.carried = Some(ItemStack { count: n, ..out });
                f.output = take(out, n);
            }
            Some(c) if c.can_stack_with(&out) => {
                let n = (c.item.max_stack().saturating_sub(c.count)).min(out.count);
                self.carried = Some(ItemStack {
                    count: c.count + n,
                    ..c
                });
                f.output = take(out, n);
            }
            _ => {}
        }
    }

    /// Consume one of each ingredient.
    fn consume_ingredients(&mut self) {
        let n = self.craft_w * self.craft_w;
        for slot in self.craft[..n].iter_mut() {
            if let Some(s) = slot {
                *slot = take(*s, 1);
            }
        }
    }

    /// Click on the result slot: put the result on the cursor (or add it to
    /// a matching carried stack).
    fn take_result(&mut self, recipes: &RecipeBook) {
        let Some(result) = self.result(recipes) else {
            return;
        };
        match self.carried {
            None => self.carried = Some(result),
            Some(c)
                if c.can_stack_with(&result) && c.count + result.count <= c.item.max_stack() =>
            {
                self.carried = Some(ItemStack {
                    count: c.count + result.count,
                    ..c
                });
            }
            _ => return,
        }
        self.consume_ingredients();
    }

    /// Shift-click on the result: craft as many as fit into the inventory.
    pub fn craft_all(&mut self, inv: &mut Inventory, recipes: &RecipeBook) {
        let order: Vec<usize> = HOTBAR.rev().chain(MAIN.rev()).collect();
        for _ in 0..64 * 9 {
            let Some(result) = self.result(recipes) else {
                break;
            };
            // Only craft when the whole result fits.
            let mut probe = inv.slots;
            if move_into(&mut probe, result, &order).is_some() {
                break;
            }
            inv.slots = probe;
            self.consume_ingredients();
        }
    }

    /// Shift-click routing.
    pub fn quick_move(&mut self, inv: &mut Inventory, s: SlotId) {
        let main_then_hotbar: Vec<usize> = MAIN.chain(HOTBAR).collect();
        let Some(stack) = self.get_raw(inv, s) else {
            return;
        };
        let rest = match s {
            SlotId::Craft(_)
            | SlotId::Armor(_)
            | SlotId::Offhand
            | SlotId::FurnaceInput
            | SlotId::FurnaceFuel => move_into(&mut inv.slots, stack, &main_then_hotbar),
            SlotId::FurnaceOutput => {
                let order: Vec<usize> = HOTBAR.rev().chain(MAIN.rev()).collect();
                move_into(&mut inv.slots, stack, &order)
            }
            SlotId::Inv(i) => {
                let i = i as usize;
                // Armor goes to its armor slot first in the survival inventory.
                if let (MenuKind::Inventory, ItemKind::Armor { slot, .. }) =
                    (self.kind, stack.item.kind())
                {
                    let a = slot as usize;
                    if a < 4 && inv.armor[a].is_none() {
                        inv.armor[a] = Some(ItemStack { count: 1, ..stack });
                        self.set_raw(inv, s, take(stack, 1));
                        return;
                    }
                }
                // Furnace: smeltables to the input, fuel to the fuel slot.
                if self.kind == MenuKind::Furnace {
                    let target = if smelt_result(stack.item).is_some() {
                        Some(SlotId::FurnaceInput)
                    } else if fuel_time(stack.item).is_some() {
                        Some(SlotId::FurnaceFuel)
                    } else {
                        None
                    };
                    if let Some(t) = target {
                        let rest = match self.slot_mut(inv, t) {
                            Some(slot) => insert_into(slot, stack),
                            None => Some(stack),
                        };
                        self.set_raw(inv, s, rest);
                        return;
                    }
                }
                let targets: Vec<usize> = if i < 9 {
                    MAIN.collect()
                } else {
                    HOTBAR.collect()
                };
                // Take the stack out first so it can't merge with itself.
                inv.slots[i] = None;
                move_into(&mut inv.slots, stack, &targets)
            }
            _ => return,
        };
        self.set_raw(inv, s, rest);
    }

    fn get_raw(&mut self, inv: &mut Inventory, s: SlotId) -> Option<ItemStack> {
        self.slot_mut(inv, s).and_then(|v| *v)
    }

    fn set_raw(&mut self, inv: &mut Inventory, s: SlotId, v: Option<ItemStack>) {
        if let Some(slot) = self.slot_mut(inv, s) {
            *slot = v;
        }
    }

    /// Can a drag that started with the carried stack also cover slot `s`?
    pub fn drag_accepts(&self, inv: &Inventory, drag: &Drag, s: SlotId) -> bool {
        let Some(c) = self.carried else { return false };
        if drag.slots.contains(&s) || drag.slots.len() >= c.count as usize {
            return false;
        }
        if !Self::accepts(s, &c) {
            return false;
        }
        let existing = match s {
            SlotId::Inv(i) => inv.slots[i as usize],
            SlotId::Armor(i) => inv.armor[i as usize],
            SlotId::Offhand => inv.offhand,
            SlotId::Craft(i) if (i as usize) < self.craft_w * self.craft_w => {
                self.craft[i as usize]
            }
            SlotId::FurnaceInput | SlotId::FurnaceFuel if self.furnace.is_some() => {
                self.get_furnace_slot(s)
            }
            _ => return false,
        };
        match existing {
            None => true,
            Some(st) => st.can_stack_with(&c) && st.count < Self::slot_limit(s, st.item),
        }
    }

    /// How many items each slot of a drag receives (preview and finish).
    pub fn drag_share(&self, drag: &Drag) -> u8 {
        let Some(c) = self.carried else { return 0 };
        match drag.button {
            ClickButton::Secondary => 1,
            _ => (c.count as usize / drag.slots.len().max(1)) as u8,
        }
    }

    /// Spread the carried stack over the dragged slots.
    pub fn finish_drag(&mut self, inv: &mut Inventory, drag: &Drag) {
        let per = self.drag_share(drag);
        for &s in &drag.slots {
            let Some(c) = self.carried else { break };
            let Some(slot) = self.slot_mut(inv, s) else {
                continue;
            };
            let limit = Self::slot_limit(s, c.item);
            let have = slot.map(|x| x.count).unwrap_or(0);
            let n = per.min(limit.saturating_sub(have)).min(c.count);
            if n == 0 {
                continue;
            }
            *slot = Some(ItemStack {
                count: have + n,
                ..c
            });
            self.carried = take(c, n);
        }
    }

    /// Click outside the panel: drop the whole carried stack (primary) or
    /// one item (secondary).
    pub fn click_outside(&mut self, button: ClickButton) -> Option<ItemStack> {
        let c = self.carried?;
        let n = if button == ClickButton::Secondary {
            1
        } else {
            c.count
        };
        self.carried = take(c, n);
        Some(ItemStack { count: n, ..c })
    }

    /// Number key while hovering a slot: swap it with hotbar slot `k`.
    pub fn swap_with_hotbar(
        &mut self,
        inv: &mut Inventory,
        recipes: &RecipeBook,
        s: SlotId,
        k: usize,
    ) {
        if k >= 9 || s == SlotId::Inv(k as u8) {
            return;
        }
        match s {
            SlotId::Result => {
                if inv.slots[k].is_none()
                    && let Some(r) = self.result(recipes)
                {
                    inv.slots[k] = Some(r);
                    self.consume_ingredients();
                }
            }
            SlotId::Palette(item) => {
                inv.slots[k] = Some(ItemStack::new(item, item.max_stack()));
            }
            SlotId::Trash => {}
            _ => {
                let here = self.get_raw(inv, s);
                let there = inv.slots[k];
                if let Some(t) = there
                    && (!Self::accepts(s, &t) || t.count > Self::slot_limit(s, t.item))
                {
                    return;
                }
                inv.slots[k] = here;
                self.set_raw(inv, s, there);
            }
        }
    }

    /// Drop key while hovering a slot: one item, or the whole stack.
    pub fn drop_from(
        &mut self,
        inv: &mut Inventory,
        recipes: &RecipeBook,
        s: SlotId,
        whole: bool,
    ) -> Option<ItemStack> {
        match s {
            SlotId::Result => {
                let r = self.result(recipes)?;
                self.consume_ingredients();
                Some(r)
            }
            SlotId::Palette(_) | SlotId::Trash => None,
            _ => {
                let st = self.get_raw(inv, s)?;
                let n = if whole { st.count } else { 1 };
                self.set_raw(inv, s, take(st, n));
                Some(ItemStack { count: n, ..st })
            }
        }
    }

    /// Move the crafting grid and the carried stack back into the inventory.
    /// Returns what did not fit (to be dropped in the world).
    pub fn close(&mut self, inv: &mut Inventory) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        let mut give = |s: ItemStack| {
            if let Some(rest) = inv.add(s) {
                drops.push(rest);
            }
        };
        if let Some(c) = self.carried.take() {
            give(c);
        }
        for slot in self.craft.iter_mut() {
            if let Some(s) = slot.take() {
                give(s);
            }
        }
        drops
    }

    /// Double click: gather items matching the carried stack from the
    /// crafting grid and inventory onto the cursor (partial stacks first).
    pub fn collect(&mut self, inv: &mut Inventory) {
        let Some(mut c) = self.carried else { return };
        let max = c.item.max_stack();
        if max <= 1 || c.damage != 0 {
            return;
        }
        let n = self.craft_w * self.craft_w;
        for full_pass in [false, true] {
            let slots = self.craft[..n].iter_mut().chain(inv.slots.iter_mut());
            for slot in slots {
                if c.count >= max {
                    break;
                }
                let Some(s) = *slot else { continue };
                if !s.can_stack_with(&c) || (s.count >= max) != full_pass {
                    continue;
                }
                let k = (max - c.count).min(s.count);
                c.count += k;
                *slot = take(s, k);
            }
        }
        self.carried = Some(c);
    }

    /// Anything still held by the container (grid or cursor)?
    pub fn holds_items(&self) -> bool {
        self.carried.is_some() || self.craft.iter().any(Option::is_some)
    }
}

impl Container {
    fn get_furnace_slot(&self, s: SlotId) -> Option<ItemStack> {
        let f = self.furnace?;
        match s {
            SlotId::FurnaceInput => f.input,
            SlotId::FurnaceFuel => f.fuel,
            SlotId::FurnaceOutput => f.output,
            _ => None,
        }
    }
}

/// Put as much of `stack` as fits into one slot; returns the remainder.
fn insert_into(slot: &mut Option<ItemStack>, stack: ItemStack) -> Option<ItemStack> {
    match *slot {
        None => {
            *slot = Some(stack);
            None
        }
        Some(s) if s.can_stack_with(&stack) => {
            let n = s.item.max_stack().saturating_sub(s.count).min(stack.count);
            *slot = Some(ItemStack {
                count: s.count + n,
                ..s
            });
            take(stack, n)
        }
        Some(_) => Some(stack),
    }
}

/// Remove `n` items from a stack.
fn take(s: ItemStack, n: u8) -> Option<ItemStack> {
    let left = s.count.saturating_sub(n);
    (left > 0).then_some(ItemStack { count: left, ..s })
}

/// Insert `stack` into `slots[targets]`: merge into matching stacks first,
/// then fill empty slots, in the given order. Returns the remainder.
pub fn move_into(
    slots: &mut [Option<ItemStack>],
    mut stack: ItemStack,
    targets: &[usize],
) -> Option<ItemStack> {
    let max = stack.item.max_stack();
    for &i in targets {
        if let Some(s) = &mut slots[i]
            && s.can_stack_with(&stack)
            && s.count < max
        {
            let n = (max - s.count).min(stack.count);
            s.count += n;
            stack.count -= n;
            if stack.count == 0 {
                return None;
            }
        }
    }
    for &i in targets {
        if slots[i].is_none() {
            let n = stack.count.min(max);
            slots[i] = Some(ItemStack { count: n, ..stack });
            stack.count -= n;
            if stack.count == 0 {
                return None;
            }
        }
    }
    Some(stack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::blocks;

    const L: ClickButton = ClickButton::Primary;
    const R: ClickButton = ClickButton::Secondary;

    fn id(n: &str) -> ItemId {
        ItemId::by_name(n).unwrap()
    }
    fn st(n: &str, c: u8) -> ItemStack {
        ItemStack::new(id(n), c)
    }

    fn setup() -> (Container, Inventory, RecipeBook) {
        (
            Container::new(MenuKind::Inventory),
            Inventory::default(),
            RecipeBook::standard(),
        )
    }

    #[test]
    fn pick_up_and_place() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[0] = Some(st("dirt", 10));
        c.click(&mut inv, &rb, SlotId::Inv(0), L, false, false);
        assert_eq!(c.carried, Some(st("dirt", 10)));
        assert_eq!(inv.slots[0], None);
        c.click(&mut inv, &rb, SlotId::Inv(20), L, false, false);
        assert_eq!(inv.slots[20], Some(st("dirt", 10)));
        assert_eq!(c.carried, None);
    }

    #[test]
    fn swap_different_items() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[1] = Some(st("stone", 5));
        c.carried = Some(st("dirt", 3));
        c.click(&mut inv, &rb, SlotId::Inv(1), L, false, false);
        assert_eq!(inv.slots[1], Some(st("dirt", 3)));
        assert_eq!(c.carried, Some(st("stone", 5)));
    }

    #[test]
    fn merge_with_overflow() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[2] = Some(st("dirt", 60));
        c.carried = Some(st("dirt", 10));
        c.click(&mut inv, &rb, SlotId::Inv(2), L, false, false);
        assert_eq!(inv.slots[2], Some(st("dirt", 64)));
        assert_eq!(c.carried, Some(st("dirt", 6)));
    }

    #[test]
    fn right_click_splits_and_places_one() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[0] = Some(st("dirt", 5));
        c.click(&mut inv, &rb, SlotId::Inv(0), R, false, false);
        assert_eq!(c.carried, Some(st("dirt", 3)));
        assert_eq!(inv.slots[0], Some(st("dirt", 2)));
        c.click(&mut inv, &rb, SlotId::Inv(5), R, false, false);
        c.click(&mut inv, &rb, SlotId::Inv(5), R, false, false);
        assert_eq!(inv.slots[5], Some(st("dirt", 2)));
        assert_eq!(c.carried, Some(st("dirt", 1)));
        c.click(&mut inv, &rb, SlotId::Inv(0), R, false, false);
        assert_eq!(inv.slots[0], Some(st("dirt", 3)));
        assert_eq!(c.carried, None);
        // Splitting a single item takes it.
        inv.slots[7] = Some(st("stick", 1));
        c.click(&mut inv, &rb, SlotId::Inv(7), R, false, false);
        assert_eq!(c.carried, Some(st("stick", 1)));
        assert_eq!(inv.slots[7], None);
    }

    #[test]
    fn tools_do_not_stack() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[0] = Some(st("iron_pickaxe", 1));
        c.carried = Some(st("iron_pickaxe", 1));
        c.click(&mut inv, &rb, SlotId::Inv(0), L, false, false);
        // Swapped rather than merged.
        assert_eq!(inv.slots[0].unwrap().count, 1);
        assert_eq!(c.carried.unwrap().count, 1);
    }

    #[test]
    fn shift_click_hotbar_and_main() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[0] = Some(st("dirt", 30));
        inv.slots[12] = Some(st("dirt", 50));
        c.click(&mut inv, &rb, SlotId::Inv(0), L, true, false);
        // Merges into the main-inventory stack first, then the first empty main slot.
        assert_eq!(inv.slots[0], None);
        assert_eq!(inv.slots[12], Some(st("dirt", 64)));
        assert_eq!(inv.slots[9], Some(st("dirt", 16)));
        // Main → hotbar.
        c.click(&mut inv, &rb, SlotId::Inv(12), L, true, false);
        assert_eq!(inv.slots[0], Some(st("dirt", 64)));
        assert_eq!(inv.slots[12], None);
    }

    #[test]
    fn shift_click_full_inventory_keeps_stack() {
        let (mut c, mut inv, rb) = setup();
        for i in 0..36 {
            inv.slots[i] = Some(st("stone", 64));
        }
        inv.slots[3] = Some(st("dirt", 5));
        c.click(&mut inv, &rb, SlotId::Inv(3), L, true, false);
        assert_eq!(inv.slots[3], Some(st("dirt", 5)));
    }

    #[test]
    fn crafting_result_and_consumption() {
        let (mut c, mut inv, rb) = setup();
        c.craft[0] = Some(st("oak_log", 2));
        assert_eq!(c.result(&rb), Some(st("oak_planks", 4)));
        c.click(&mut inv, &rb, SlotId::Result, L, false, false);
        assert_eq!(c.carried, Some(st("oak_planks", 4)));
        assert_eq!(c.craft[0], Some(st("oak_log", 1)));
        // Clicking again adds to the carried stack.
        c.click(&mut inv, &rb, SlotId::Result, L, false, false);
        assert_eq!(c.carried, Some(st("oak_planks", 8)));
        assert_eq!(c.craft[0], None);
        assert_eq!(c.result(&rb), None);
    }

    #[test]
    fn shift_click_result_crafts_all() {
        let (mut c, mut inv, rb) = setup();
        c.craft[1] = Some(st("birch_log", 5));
        c.click(&mut inv, &rb, SlotId::Result, L, true, false);
        assert_eq!(c.craft[1], None);
        assert_eq!(inv.count_of(id("birch_planks")), 20);
        // Goes to the right end of the hotbar first.
        assert_eq!(inv.slots[8], Some(st("birch_planks", 20)));
    }

    #[test]
    fn crafting_table_uses_3x3() {
        let mut c = Container::new(MenuKind::CraftingTable);
        let mut inv = Inventory::default();
        let rb = RecipeBook::standard();
        let p = Some(st("oak_planks", 1));
        let k = Some(st("stick", 1));
        c.craft = [p, p, p, None, k, None, None, k, None];
        assert_eq!(c.result(&rb).unwrap().item, id("wooden_pickaxe"));
        c.click(&mut inv, &rb, SlotId::Result, L, false, false);
        assert_eq!(c.carried.unwrap().item, id("wooden_pickaxe"));
        assert!(c.craft.iter().all(Option::is_none));
    }

    #[test]
    fn drag_distributes_evenly() {
        let (mut c, mut inv, _rb) = setup();
        c.carried = Some(st("dirt", 10));
        let mut d = Drag {
            button: L,
            slots: vec![],
        };
        for i in [9u8, 10, 11] {
            assert!(c.drag_accepts(&inv, &d, SlotId::Inv(i)));
            d.slots.push(SlotId::Inv(i));
        }
        assert!(!c.drag_accepts(&inv, &d, SlotId::Inv(9)), "no duplicates");
        c.finish_drag(&mut inv, &d);
        for i in [9, 10, 11] {
            assert_eq!(inv.slots[i], Some(st("dirt", 3)));
        }
        assert_eq!(c.carried, Some(st("dirt", 1)));

        // Right-drag places one each.
        c.carried = Some(st("stone", 4));
        let d = Drag {
            button: R,
            slots: vec![SlotId::Inv(20), SlotId::Inv(21)],
        };
        c.finish_drag(&mut inv, &d);
        assert_eq!(inv.slots[20], Some(st("stone", 1)));
        assert_eq!(c.carried, Some(st("stone", 2)));
    }

    #[test]
    fn drop_outside_and_close() {
        let (mut c, mut inv, _rb) = setup();
        c.carried = Some(st("dirt", 5));
        assert_eq!(c.click_outside(R), Some(st("dirt", 1)));
        assert_eq!(c.click_outside(L), Some(st("dirt", 4)));
        assert_eq!(c.carried, None);
        assert_eq!(c.click_outside(L), None);

        c.carried = Some(st("stone", 3));
        c.craft[2] = Some(st("oak_log", 1));
        let drops = c.close(&mut inv);
        assert!(drops.is_empty());
        assert!(!c.holds_items());
        assert_eq!(inv.count_of(id("stone")), 3);
        assert_eq!(inv.count_of(id("oak_log")), 1);

        // Full inventory → leftovers are dropped.
        for i in 0..36 {
            inv.slots[i] = Some(ItemStack::of_block(blocks::GLASS, 64));
        }
        c.carried = Some(st("dirt", 7));
        assert_eq!(c.close(&mut inv), vec![st("dirt", 7)]);
    }

    #[test]
    fn hotbar_swap_and_drop_key() {
        let (mut c, mut inv, rb) = setup();
        inv.slots[15] = Some(st("dirt", 9));
        inv.slots[2] = Some(st("stone", 1));
        c.swap_with_hotbar(&mut inv, &rb, SlotId::Inv(15), 2);
        assert_eq!(inv.slots[2], Some(st("dirt", 9)));
        assert_eq!(inv.slots[15], Some(st("stone", 1)));
        assert_eq!(
            c.drop_from(&mut inv, &rb, SlotId::Inv(2), false),
            Some(st("dirt", 1))
        );
        assert_eq!(inv.slots[2], Some(st("dirt", 8)));
        assert_eq!(
            c.drop_from(&mut inv, &rb, SlotId::Inv(2), true),
            Some(st("dirt", 8))
        );
        assert_eq!(inv.slots[2], None);
    }

    #[test]
    fn armor_slots_only_take_armor() {
        let (mut c, mut inv, rb) = setup();
        c.carried = Some(st("dirt", 1));
        c.click(&mut inv, &rb, SlotId::Armor(0), L, false, false);
        assert_eq!(inv.armor[0], None);
        assert_eq!(c.carried, Some(st("dirt", 1)));
    }

    #[test]
    fn double_click_collects() {
        let (mut c, mut inv, _rb) = setup();
        c.carried = Some(st("dirt", 10));
        inv.slots[3] = Some(st("dirt", 64));
        inv.slots[12] = Some(st("dirt", 20));
        inv.slots[13] = Some(st("stone", 20));
        c.craft[0] = Some(st("dirt", 5));
        c.collect(&mut inv);
        // Partial stacks first (grid 5, then slot 12's 20), then full ones.
        assert_eq!(c.carried, Some(st("dirt", 64)));
        assert_eq!(c.craft[0], None);
        assert_eq!(inv.slots[12], None);
        assert_eq!(inv.slots[3], Some(st("dirt", 35)));
        assert_eq!(inv.slots[13], Some(st("stone", 20)));
    }

    #[test]
    fn furnace_slots() {
        let mut c = Container::new(MenuKind::Furnace);
        c.furnace = Some(Furnace::default());
        let mut inv = Inventory::default();
        let rb = RecipeBook::standard();
        inv.slots[0] = Some(st("sand", 10));
        inv.slots[1] = Some(st("coal", 3));
        inv.slots[2] = Some(st("dirt", 3));
        // Shift-click routes smeltables and fuel.
        c.click(&mut inv, &rb, SlotId::Inv(0), L, true, false);
        c.click(&mut inv, &rb, SlotId::Inv(1), L, true, false);
        c.click(&mut inv, &rb, SlotId::Inv(2), L, true, false);
        let f = c.furnace.unwrap();
        assert_eq!(f.input, Some(st("sand", 10)));
        assert_eq!(f.fuel, Some(st("coal", 3)));
        assert_eq!(inv.slots[2], None);
        assert_eq!(inv.slots[9], Some(st("dirt", 3)));
        // Dirt is not fuel.
        c.carried = Some(st("dirt", 3));
        c.click(&mut inv, &rb, SlotId::FurnaceFuel, L, false, false);
        assert_eq!(c.carried, Some(st("dirt", 3)));
        c.carried = None;
        // Output can be taken but not filled.
        c.furnace.as_mut().unwrap().output = Some(st("glass", 4));
        c.click(&mut inv, &rb, SlotId::FurnaceOutput, R, false, false);
        assert_eq!(c.carried, Some(st("glass", 2)));
        c.click(&mut inv, &rb, SlotId::FurnaceOutput, L, false, false);
        assert_eq!(c.carried, Some(st("glass", 4)));
        assert_eq!(c.furnace.unwrap().output, None);
        c.click(&mut inv, &rb, SlotId::FurnaceOutput, L, false, false);
        assert_eq!(c.furnace.unwrap().output, None);
        assert_eq!(c.carried, Some(st("glass", 4)));
    }

    #[test]
    fn creative_palette() {
        let mut c = Container::new(MenuKind::Creative);
        let mut inv = Inventory::default();
        let rb = RecipeBook::standard();
        let dirt = id("dirt");
        c.click(&mut inv, &rb, SlotId::Palette(dirt), L, false, true);
        assert_eq!(c.carried, Some(st("dirt", 64)));
        // Clicking another palette item destroys the carried stack.
        c.click(&mut inv, &rb, SlotId::Palette(id("stone")), L, false, true);
        assert_eq!(c.carried, None);
        c.click(&mut inv, &rb, SlotId::Palette(dirt), R, false, true);
        assert_eq!(c.carried, Some(st("dirt", 1)));
        c.click(&mut inv, &rb, SlotId::Trash, L, false, true);
        assert_eq!(c.carried, None);
        c.click(&mut inv, &rb, SlotId::Palette(dirt), L, true, true);
        assert_eq!(inv.slots[0], Some(st("dirt", 64)));
        // Middle click clones a full stack in creative.
        c.click(
            &mut inv,
            &rb,
            SlotId::Inv(0),
            ClickButton::Middle,
            false,
            true,
        );
        assert_eq!(c.carried, Some(st("dirt", 64)));
        assert_eq!(inv.slots[0], Some(st("dirt", 64)));
        c.carried = None;
        c.click(&mut inv, &rb, SlotId::Trash, L, true, true);
        assert!(inv.slots.iter().all(Option::is_none));
    }
}
