//! Item registry.
//!
//! Item ids `0..BlockId::count()` are the block items (same index as the
//! block). Non-block items follow, in [`ITEM_DEFS`] order. `texture` is the
//! key used in the pack's `textures/item_texture.json` (`texture_data`).
//! Append new items at the END of `ITEM_DEFS`.

use crate::BlockId;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ItemId(pub u16);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ItemKind {
    Block(BlockId),
    Material,
    Food {
        hunger: u8,
        saturation: f32,
    },
    Tool {
        tool: crate::block::Tool,
        tier: u8,
        durability: u16,
        speed: f32,
        damage: f32,
    },
    Armor {
        slot: u8,
        defense: u8,
        durability: u16,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ItemDef {
    pub name: &'static str,
    pub display: &'static str,
    /// Key into `item_texture.json` (`texture_data`). Unused for block items.
    pub texture: &'static str,
    pub max_stack: u8,
    pub kind: ItemKind,
}

const fn mat(name: &'static str, display: &'static str, texture: &'static str) -> ItemDef {
    ItemDef {
        name,
        display,
        texture,
        max_stack: 64,
        kind: ItemKind::Material,
    }
}

const fn food(
    name: &'static str,
    display: &'static str,
    texture: &'static str,
    hunger: u8,
    saturation: f32,
) -> ItemDef {
    ItemDef {
        name,
        display,
        texture,
        max_stack: 64,
        kind: ItemKind::Food { hunger, saturation },
    }
}

/// Tool tiers: 0 wood/gold, 1 stone, 2 iron, 3 diamond.
const fn tool(
    name: &'static str,
    display: &'static str,
    texture: &'static str,
    tool: crate::block::Tool,
    tier: u8,
    durability: u16,
    speed: f32,
    damage: f32,
) -> ItemDef {
    ItemDef {
        name,
        display,
        texture,
        max_stack: 1,
        kind: ItemKind::Tool {
            tool,
            tier,
            durability,
            speed,
            damage,
        },
    }
}

use crate::block::Tool as T;

/// Non-block items. Their `ItemId` is `BlockId::count() + index`.
pub static ITEM_DEFS: &[ItemDef] = &[
    mat("stick", "Stick", "stick"),
    mat("coal", "Coal", "coal"),
    mat("raw_iron", "Raw Iron", "raw_iron"),
    mat("raw_copper", "Raw Copper", "raw_copper"),
    mat("raw_gold", "Raw Gold", "raw_gold"),
    mat("iron_ingot", "Iron Ingot", "iron_ingot"),
    mat("copper_ingot", "Copper Ingot", "copper_ingot"),
    mat("gold_ingot", "Gold Ingot", "gold_ingot"),
    mat("diamond", "Diamond", "diamond"),
    mat("emerald", "Emerald", "emerald"),
    mat("redstone", "Redstone Dust", "redstone_dust"),
    mat("lapis_lazuli", "Lapis Lazuli", "dye_powder_blue_new"),
    mat("clay_ball", "Clay Ball", "clay_ball"),
    mat("snowball", "Snowball", "snowball"),
    mat("glowstone_dust", "Glowstone Dust", "glowstone_dust"),
    mat("book", "Book", "book_normal"),
    mat("string", "String", "string"),
    mat("feather", "Feather", "feather"),
    mat("leather", "Leather", "leather"),
    mat("bone", "Bone", "bone"),
    mat("gunpowder", "Gunpowder", "gunpowder"),
    mat("rotten_flesh", "Rotten Flesh", "rotten_flesh"),
    mat("arrow", "Arrow", "arrow"),
    food("apple", "Apple", "apple", 4, 2.4),
    food("bread", "Bread", "bread", 5, 6.0),
    food("porkchop", "Raw Porkchop", "porkchop_raw", 3, 1.8),
    food(
        "cooked_porkchop",
        "Cooked Porkchop",
        "porkchop_cooked",
        8,
        12.8,
    ),
    food("beef", "Raw Beef", "beef_raw", 3, 1.8),
    food("cooked_beef", "Steak", "beef_cooked", 8, 12.8),
    food("chicken", "Raw Chicken", "chicken_raw", 2, 1.2),
    food("cooked_chicken", "Cooked Chicken", "chicken_cooked", 6, 7.2),
    food("mutton", "Raw Mutton", "mutton_raw", 2, 1.2),
    food("cooked_mutton", "Cooked Mutton", "mutton_cooked", 6, 9.6),
    food("melon_slice", "Melon Slice", "melon", 2, 1.2),
    tool(
        "wooden_pickaxe",
        "Wooden Pickaxe",
        "wood_pickaxe",
        T::Pickaxe,
        0,
        59,
        2.0,
        2.0,
    ),
    tool(
        "stone_pickaxe",
        "Stone Pickaxe",
        "stone_pickaxe",
        T::Pickaxe,
        1,
        131,
        4.0,
        3.0,
    ),
    tool(
        "iron_pickaxe",
        "Iron Pickaxe",
        "iron_pickaxe",
        T::Pickaxe,
        2,
        250,
        6.0,
        4.0,
    ),
    tool(
        "diamond_pickaxe",
        "Diamond Pickaxe",
        "diamond_pickaxe",
        T::Pickaxe,
        3,
        1561,
        8.0,
        5.0,
    ),
    tool(
        "wooden_axe",
        "Wooden Axe",
        "wood_axe",
        T::Axe,
        0,
        59,
        2.0,
        7.0,
    ),
    tool(
        "stone_axe",
        "Stone Axe",
        "stone_axe",
        T::Axe,
        1,
        131,
        4.0,
        9.0,
    ),
    tool("iron_axe", "Iron Axe", "iron_axe", T::Axe, 2, 250, 6.0, 9.0),
    tool(
        "diamond_axe",
        "Diamond Axe",
        "diamond_axe",
        T::Axe,
        3,
        1561,
        8.0,
        9.0,
    ),
    tool(
        "wooden_shovel",
        "Wooden Shovel",
        "wood_shovel",
        T::Shovel,
        0,
        59,
        2.0,
        2.5,
    ),
    tool(
        "stone_shovel",
        "Stone Shovel",
        "stone_shovel",
        T::Shovel,
        1,
        131,
        4.0,
        3.5,
    ),
    tool(
        "iron_shovel",
        "Iron Shovel",
        "iron_shovel",
        T::Shovel,
        2,
        250,
        6.0,
        4.5,
    ),
    tool(
        "diamond_shovel",
        "Diamond Shovel",
        "diamond_shovel",
        T::Shovel,
        3,
        1561,
        8.0,
        5.5,
    ),
    tool(
        "wooden_sword",
        "Wooden Sword",
        "wood_sword",
        T::Sword,
        0,
        59,
        1.5,
        4.0,
    ),
    tool(
        "stone_sword",
        "Stone Sword",
        "stone_sword",
        T::Sword,
        1,
        131,
        1.5,
        5.0,
    ),
    tool(
        "iron_sword",
        "Iron Sword",
        "iron_sword",
        T::Sword,
        2,
        250,
        1.5,
        6.0,
    ),
    tool(
        "diamond_sword",
        "Diamond Sword",
        "diamond_sword",
        T::Sword,
        3,
        1561,
        1.5,
        7.0,
    ),
    tool("shears", "Shears", "shears", T::Shears, 2, 238, 5.0, 1.0),
    mat("wheat", "Wheat", "wheat"),
    mat("paper", "Paper", "paper"),
    mat("brick", "Brick", "brick"),
    // Laid by chickens.
    ItemDef {
        name: "egg",
        display: "Egg",
        texture: "egg",
        max_stack: 16,
        kind: ItemKind::Material,
    },
];

impl ItemId {
    pub fn from_block(b: BlockId) -> ItemId {
        ItemId(b.0)
    }
    /// The block this item places, if any.
    pub fn block(self) -> Option<BlockId> {
        ((self.0 as usize) < BlockId::count()).then_some(BlockId(self.0))
    }
    pub fn count() -> usize {
        BlockId::count() + ITEM_DEFS.len()
    }
    pub fn by_name(name: &str) -> Option<ItemId> {
        if let Some(b) = BlockId::by_name(name) {
            return Some(ItemId(b.0));
        }
        ITEM_DEFS
            .iter()
            .position(|d| d.name == name)
            .map(|i| ItemId((BlockId::count() + i) as u16))
    }
    pub fn name(self) -> &'static str {
        match self.block() {
            Some(b) => b.def().name,
            None => self.def().map(|d| d.name).unwrap_or("unknown"),
        }
    }
    pub fn display(self) -> &'static str {
        match self.block() {
            Some(b) => b.def().display,
            None => self.def().map(|d| d.display).unwrap_or("Unknown"),
        }
    }
    /// Definition for non-block items.
    pub fn def(self) -> Option<&'static ItemDef> {
        (self.0 as usize)
            .checked_sub(BlockId::count())
            .and_then(|i| ITEM_DEFS.get(i))
    }
    pub fn kind(self) -> ItemKind {
        match self.block() {
            Some(b) => ItemKind::Block(b),
            None => self.def().map(|d| d.kind).unwrap_or(ItemKind::Material),
        }
    }
    pub fn max_stack(self) -> u8 {
        match self.block() {
            Some(_) => 64,
            None => self.def().map(|d| d.max_stack).unwrap_or(64),
        }
    }
    pub fn all() -> impl Iterator<Item = ItemId> {
        (1..ItemId::count() as u16).map(ItemId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u8,
    /// Damage taken (tools/armor).
    pub damage: u16,
}

impl ItemStack {
    pub fn new(item: ItemId, count: u8) -> Self {
        ItemStack {
            item,
            count,
            damage: 0,
        }
    }
    pub fn of_block(b: BlockId, count: u8) -> Self {
        Self::new(ItemId::from_block(b), count)
    }
    pub fn can_stack_with(&self, o: &ItemStack) -> bool {
        self.item == o.item && self.damage == 0 && o.damage == 0 && self.item.max_stack() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn item_lookup() {
        assert_eq!(
            ItemId::by_name("stone").unwrap().block(),
            Some(blocks::STONE)
        );
        let stick = ItemId::by_name("stick").unwrap();
        assert!(stick.block().is_none());
        assert_eq!(stick.name(), "stick");
        assert_eq!(ItemId::by_name("diamond_pickaxe").unwrap().max_stack(), 1);
    }

    #[test]
    fn block_drops_resolve_to_items() {
        for b in BlockId::all() {
            if let crate::block::Drop::Other(name, _) = b.def().drop {
                assert!(
                    ItemId::by_name(name).is_some(),
                    "drop item {name} of {} missing",
                    b.def().name
                );
            }
        }
    }
}
