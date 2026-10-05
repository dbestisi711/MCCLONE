//! Player inventory storage. UI/crafting logic lives in `mc-ui`.

use crate::{ItemId, ItemStack};

pub const HOTBAR_SIZE: usize = 9;
/// 9 hotbar + 27 main.
pub const MAIN_SIZE: usize = 36;

#[derive(Clone, Debug)]
pub struct Inventory {
    /// Slots 0..9 are the hotbar, 9..36 the main inventory.
    pub slots: [Option<ItemStack>; MAIN_SIZE],
    /// Helmet, chestplate, leggings, boots.
    pub armor: [Option<ItemStack>; 4],
    pub offhand: Option<ItemStack>,
    /// Selected hotbar slot 0..9.
    pub selected: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            slots: [None; MAIN_SIZE],
            armor: [None; 4],
            offhand: None,
            selected: 0,
        }
    }
}

impl Inventory {
    pub fn selected_stack(&self) -> Option<&ItemStack> {
        self.slots[self.selected].as_ref()
    }

    pub fn selected_item(&self) -> Option<ItemId> {
        self.selected_stack().map(|s| s.item)
    }

    /// Remove `n` items from the selected hotbar slot.
    pub fn consume_selected(&mut self, n: u8) {
        if let Some(s) = &mut self.slots[self.selected] {
            s.count = s.count.saturating_sub(n);
            if s.count == 0 {
                self.slots[self.selected] = None;
            }
        }
    }

    /// Add a stack (merging into existing stacks first, hotbar first).
    /// Returns whatever did not fit.
    pub fn add(&mut self, mut stack: ItemStack) -> Option<ItemStack> {
        let max = stack.item.max_stack();
        for slot in self.slots.iter_mut().flatten() {
            if slot.can_stack_with(&stack) && slot.count < max {
                let moved = (max - slot.count).min(stack.count);
                slot.count += moved;
                stack.count -= moved;
                if stack.count == 0 {
                    return None;
                }
            }
        }
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                let moved = stack.count.min(max);
                *slot = Some(ItemStack {
                    count: moved,
                    ..stack
                });
                stack.count -= moved;
                if stack.count == 0 {
                    return None;
                }
            }
        }
        Some(stack)
    }

    pub fn count_of(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn add_merges_and_overflows() {
        let mut inv = Inventory::default();
        assert!(inv.add(ItemStack::of_block(blocks::DIRT, 40)).is_none());
        assert!(inv.add(ItemStack::of_block(blocks::DIRT, 40)).is_none());
        assert_eq!(inv.slots[0].unwrap().count, 64);
        assert_eq!(inv.slots[1].unwrap().count, 16);
        assert_eq!(inv.count_of(ItemId::from_block(blocks::DIRT)), 80);
    }
}
