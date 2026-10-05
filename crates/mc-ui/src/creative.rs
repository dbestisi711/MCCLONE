//! Creative inventory tabs: every obtainable item sorted into a category.

use mc_core::ItemId;
use mc_core::block::Shape;
use mc_core::item::ItemKind;

pub struct Tab {
    pub name: &'static str,
    /// Item shown on the tab.
    pub icon: ItemId,
    pub items: Vec<ItemId>,
    /// The survival-inventory tab (shows the player's inventory instead of a
    /// palette).
    pub inventory: bool,
}

const NAMES: [(&str, &str); 7] = [
    ("Building Blocks", "bricks"),
    ("Natural Blocks", "grass_block"),
    ("Functional Blocks", "crafting_table"),
    ("Tools & Combat", "iron_pickaxe"),
    ("Food", "apple"),
    ("Ingredients", "iron_ingot"),
    ("Inventory", "chest"),
];

pub const INVENTORY_TAB: usize = 6;

const FUNCTIONAL: &[&str] = &[
    "crafting_table",
    "furnace",
    "chest",
    "torch",
    "bookshelf",
    "tnt",
    "glowstone",
];

const NATURAL: &[&str] = &[
    "grass_block",
    "dirt",
    "coarse_dirt",
    "podzol",
    "sand",
    "red_sand",
    "gravel",
    "clay",
    "mud",
    "snow_block",
    "snow_layer",
    "ice",
    "packed_ice",
    "cactus",
    "pumpkin",
    "melon",
    "mycelium",
    "moss_block",
    "bedrock",
    "obsidian",
    "lily_pad",
    "vine",
];

/// Category index for an item, or `None` when it should not be listed
/// (air, fluids).
pub fn category(item: ItemId) -> Option<usize> {
    match item.kind() {
        ItemKind::Block(b) => {
            let def = b.def();
            if matches!(def.shape, Shape::None | Shape::Liquid) {
                return None;
            }
            let n = def.name;
            if FUNCTIONAL.contains(&n) {
                Some(2)
            } else if NATURAL.contains(&n)
                || n.ends_with("_ore")
                || n.ends_with("_log")
                || n.ends_with("_leaves")
                || matches!(def.shape, Shape::Cross)
            {
                Some(1)
            } else {
                Some(0)
            }
        }
        ItemKind::Tool { .. } | ItemKind::Armor { .. } => Some(3),
        ItemKind::Food { .. } => Some(4),
        ItemKind::Material if item.name() == "arrow" => Some(3),
        ItemKind::Material => Some(5),
    }
}

pub fn tabs() -> Vec<Tab> {
    let mut tabs: Vec<Tab> = NAMES
        .iter()
        .enumerate()
        .map(|(i, (name, icon))| Tab {
            name,
            icon: ItemId::by_name(icon).unwrap_or(ItemId(1)),
            items: Vec::new(),
            inventory: i == INVENTORY_TAB,
        })
        .collect();
    for item in ItemId::all() {
        if let Some(c) = category(item) {
            tabs[c].items.push(item);
        }
    }
    tabs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listable_item_is_in_exactly_one_tab() {
        let tabs = tabs();
        let total: usize = tabs.iter().map(|t| t.items.len()).sum();
        let listable = ItemId::all().filter(|i| category(*i).is_some()).count();
        assert_eq!(total, listable);
        assert!(tabs[INVENTORY_TAB].items.is_empty());
        let stone = ItemId::by_name("stone").unwrap();
        assert!(tabs[0].items.contains(&stone));
        let water = ItemId::by_name("water").unwrap();
        assert!(tabs.iter().all(|t| !t.items.contains(&water)));
        for t in &tabs[..INVENTORY_TAB] {
            assert!(!t.items.is_empty(), "tab {} is empty", t.name);
        }
    }
}
