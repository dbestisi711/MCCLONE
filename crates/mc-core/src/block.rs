//! Block registry.
//!
//! Blocks are identified by a dense `BlockId(u16)` index into [`BLOCK_DEFS`].
//! `pack_name` is the key used in the resource pack's `blocks.json`; the asset
//! loader resolves face textures through it, so it must match that file.
//!
//! To add a block: append a line to the `define_blocks!` invocation (append at
//! the END of the list so existing ids stay stable and merges stay trivial).

use glam::IVec3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockId(pub u16);

impl BlockId {
    #[inline]
    pub fn def(self) -> &'static BlockDef {
        BLOCK_DEFS.get(self.0 as usize).unwrap_or(&BLOCK_DEFS[0])
    }
    #[inline]
    pub fn is_air(self) -> bool {
        self.0 == 0
    }
    pub fn by_name(name: &str) -> Option<BlockId> {
        BLOCK_DEFS
            .iter()
            .position(|d| d.name == name)
            .map(|i| BlockId(i as u16))
    }
    pub fn count() -> usize {
        BLOCK_DEFS.len()
    }
    pub fn all() -> impl Iterator<Item = BlockId> {
        (0..BLOCK_DEFS.len() as u16).map(BlockId)
    }
}

/// Cube face, also used as a direction. Order matches `Face::ALL`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Face {
    /// +Y
    Up = 0,
    /// -Y
    Down = 1,
    /// -Z
    North = 2,
    /// +Z
    South = 3,
    /// +X
    East = 4,
    /// -X
    West = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Up,
        Face::Down,
        Face::North,
        Face::South,
        Face::East,
        Face::West,
    ];
    pub fn normal(self) -> IVec3 {
        match self {
            Face::Up => IVec3::Y,
            Face::Down => IVec3::NEG_Y,
            Face::North => IVec3::NEG_Z,
            Face::South => IVec3::Z,
            Face::East => IVec3::X,
            Face::West => IVec3::NEG_X,
        }
    }
    pub fn opposite(self) -> Face {
        match self {
            Face::Up => Face::Down,
            Face::Down => Face::Up,
            Face::North => Face::South,
            Face::South => Face::North,
            Face::East => Face::West,
            Face::West => Face::East,
        }
    }
    pub fn index(self) -> usize {
        self as usize
    }
    /// Name used by `blocks.json` per-face texture maps.
    pub fn pack_key(self) -> &'static str {
        match self {
            Face::Up => "up",
            Face::Down => "down",
            Face::North => "north",
            Face::South => "south",
            Face::East => "east",
            Face::West => "west",
        }
    }
}

/// How a block is turned into geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Nothing rendered (air).
    None,
    /// Full unit cube.
    Cube,
    /// Two diagonal quads (flowers, grass, saplings).
    Cross,
    /// Fluid surface (water/lava): cube with lowered top, faces only against non-fluid.
    Liquid,
    /// Thin layer on the floor (snow layer, lily pad, carpet). Height in 1/16ths.
    Layer(u8),
    /// Small centred post (torch). Rendered as a thin box.
    Torch,
    /// Cube inset by 1/16 on the sides (cactus).
    Inset,
}

/// Which render pass a block's faces go into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Opaque,
    /// Alpha-tested (leaves, glass, plants).
    Cutout,
    /// Alpha-blended, sorted (water, ice).
    Translucent,
}

/// Biome-dependent colour multiplier applied to a block's textures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tint {
    None,
    /// Grass colormap. For grass blocks only the top face (and the side overlay) is tinted.
    Grass,
    /// Foliage colormap (leaves, vines).
    Foliage,
    /// Water colour.
    Water,
    /// Fixed colour (e.g. birch/spruce leaves), 0xRRGGBB.
    Fixed(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    None,
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Shears,
    Sword,
}

/// What a block drops when broken.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drop {
    /// The block's own item.
    Itself,
    Nothing,
    /// Another item, by item name, and count.
    Other(&'static str, u8),
}

#[derive(Clone, Copy, Debug)]
pub struct BlockDef {
    /// Our identifier (also the item name of the block's item).
    pub name: &'static str,
    /// Key in the resource pack's `blocks.json`.
    pub pack_name: &'static str,
    /// Human readable name for tooltips.
    pub display: &'static str,
    pub shape: Shape,
    pub layer: Layer,
    /// Has a collision box.
    pub solid: bool,
    /// Fully occludes neighbouring faces and blocks light.
    pub opaque: bool,
    /// Seconds to break by hand ≈ hardness * 1.5 (Minecraft-like). Negative = unbreakable.
    pub hardness: f32,
    /// Block light emitted, 0..=15.
    pub light: u8,
    pub tint: Tint,
    /// Can be replaced by placing a block into it (air, fluids, tall grass).
    pub replaceable: bool,
    /// Tool that breaks it fastest.
    pub tool: Tool,
    /// If true, breaking without the right tool drops nothing (stone, ores).
    pub needs_tool: bool,
    pub drop: Drop,
    /// Is a fluid (water/lava): entities swim in it.
    pub fluid: bool,
    /// Falls when unsupported (sand, gravel).
    pub gravity: bool,
    /// Needs a solid block below or it breaks (plants, torches, snow layers).
    pub needs_support: bool,
    /// Sound group name (pack `sounds.json` block sound key).
    pub sound: &'static str,
}

impl BlockDef {
    const fn base(name: &'static str, pack_name: &'static str, display: &'static str) -> Self {
        BlockDef {
            name,
            pack_name,
            display,
            shape: Shape::Cube,
            layer: Layer::Opaque,
            solid: true,
            opaque: true,
            hardness: 1.0,
            light: 0,
            tint: Tint::None,
            replaceable: false,
            tool: Tool::None,
            needs_tool: false,
            drop: Drop::Itself,
            fluid: false,
            gravity: false,
            needs_support: false,
            sound: "stone",
        }
    }
    pub const fn hardness(mut self, h: f32) -> Self {
        self.hardness = h;
        self
    }
    pub const fn tool(mut self, t: Tool) -> Self {
        self.tool = t;
        self
    }
    pub const fn needs_tool(mut self) -> Self {
        self.needs_tool = true;
        self
    }
    pub const fn drop(mut self, d: Drop) -> Self {
        self.drop = d;
        self
    }
    pub const fn tint(mut self, t: Tint) -> Self {
        self.tint = t;
        self
    }
    pub const fn light(mut self, l: u8) -> Self {
        self.light = l;
        self
    }
    pub const fn sound(mut self, s: &'static str) -> Self {
        self.sound = s;
        self
    }
    pub const fn gravity(mut self) -> Self {
        self.gravity = true;
        self
    }
    pub const fn cutout(mut self) -> Self {
        self.layer = Layer::Cutout;
        self.opaque = false;
        self
    }
    pub const fn translucent(mut self) -> Self {
        self.layer = Layer::Translucent;
        self.opaque = false;
        self
    }
    pub const fn shape(mut self, s: Shape) -> Self {
        self.shape = s;
        self
    }
    /// Is this a full opaque cube for face culling / AO purposes.
    #[inline]
    pub fn occludes(&self) -> bool {
        self.opaque && matches!(self.shape, Shape::Cube)
    }
}

const fn air() -> BlockDef {
    let mut d = BlockDef::base("air", "air", "Air");
    d.shape = Shape::None;
    d.solid = false;
    d.opaque = false;
    d.hardness = 0.0;
    d.replaceable = true;
    d.drop = Drop::Nothing;
    d
}

/// Plain full cube.
const fn cube(name: &'static str, pack: &'static str, display: &'static str) -> BlockDef {
    BlockDef::base(name, pack, display)
}

const fn stone_like(
    name: &'static str,
    pack: &'static str,
    display: &'static str,
    h: f32,
) -> BlockDef {
    BlockDef::base(name, pack, display)
        .hardness(h)
        .tool(Tool::Pickaxe)
        .needs_tool()
}

const fn ore(
    name: &'static str,
    pack: &'static str,
    display: &'static str,
    drop: Drop,
) -> BlockDef {
    stone_like(name, pack, display, 3.0).drop(drop)
}

const fn wood(name: &'static str, pack: &'static str, display: &'static str) -> BlockDef {
    BlockDef::base(name, pack, display)
        .hardness(2.0)
        .tool(Tool::Axe)
        .sound("wood")
}

const fn dirt_like(
    name: &'static str,
    pack: &'static str,
    display: &'static str,
    h: f32,
) -> BlockDef {
    BlockDef::base(name, pack, display)
        .hardness(h)
        .tool(Tool::Shovel)
        .sound("gravel")
}

const fn leaves(
    name: &'static str,
    pack: &'static str,
    display: &'static str,
    tint: Tint,
) -> BlockDef {
    BlockDef::base(name, pack, display)
        .hardness(0.2)
        .tool(Tool::Shears)
        .tint(tint)
        .cutout()
        .drop(Drop::Nothing)
        .sound("grass")
}

const fn plant(name: &'static str, pack: &'static str, display: &'static str) -> BlockDef {
    let mut d = BlockDef::base(name, pack, display)
        .hardness(0.0)
        .cutout()
        .shape(Shape::Cross)
        .sound("grass");
    d.solid = false;
    d.needs_support = true;
    d
}

const fn grass_plant(name: &'static str, pack: &'static str, display: &'static str) -> BlockDef {
    let mut d = plant(name, pack, display)
        .tint(Tint::Grass)
        .drop(Drop::Nothing);
    d.replaceable = true;
    d
}

const fn liquid(name: &'static str, pack: &'static str, display: &'static str) -> BlockDef {
    let mut d = BlockDef::base(name, pack, display)
        .translucent()
        .shape(Shape::Liquid);
    d.solid = false;
    d.hardness = -1.0;
    d.replaceable = true;
    d.fluid = true;
    d.drop = Drop::Nothing;
    d
}

macro_rules! define_blocks {
    ($($id:ident => $def:expr),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u16)]
        enum BlockIndex { $($id),* }

        /// Block id constants, e.g. `blocks::STONE`.
        pub mod blocks {
            use super::{BlockId, BlockIndex};
            $(pub const $id: BlockId = BlockId(BlockIndex::$id as u16);)*
        }

        /// All block definitions, indexed by `BlockId`.
        pub static BLOCK_DEFS: &[BlockDef] = &[$($def),*];
    };
}

define_blocks! {
    AIR => air(),
    STONE => stone_like("stone", "stone", "Stone", 1.5).drop(Drop::Other("cobblestone", 1)),
    GRANITE => stone_like("granite", "granite", "Granite", 1.5),
    DIORITE => stone_like("diorite", "diorite", "Diorite", 1.5),
    ANDESITE => stone_like("andesite", "andesite", "Andesite", 1.5),
    GRASS_BLOCK => dirt_like("grass_block", "grass", "Grass Block", 0.6).tint(Tint::Grass).drop(Drop::Other("dirt", 1)).sound("grass"),
    DIRT => dirt_like("dirt", "dirt", "Dirt", 0.5),
    COARSE_DIRT => dirt_like("coarse_dirt", "coarse_dirt", "Coarse Dirt", 0.5),
    PODZOL => dirt_like("podzol", "podzol", "Podzol", 0.5).drop(Drop::Other("dirt", 1)),
    COBBLESTONE => stone_like("cobblestone", "cobblestone", "Cobblestone", 2.0),
    MOSSY_COBBLESTONE => stone_like("mossy_cobblestone", "mossy_cobblestone", "Mossy Cobblestone", 2.0),
    BEDROCK => cube("bedrock", "bedrock", "Bedrock").hardness(-1.0),
    SAND => dirt_like("sand", "sand", "Sand", 0.5).gravity().sound("sand"),
    RED_SAND => dirt_like("red_sand", "red_sand", "Red Sand", 0.5).gravity().sound("sand"),
    GRAVEL => dirt_like("gravel", "gravel", "Gravel", 0.6).gravity(),
    CLAY => dirt_like("clay", "clay", "Clay", 0.6).drop(Drop::Other("clay_ball", 4)),
    MUD => dirt_like("mud", "mud", "Mud", 0.5),
    SANDSTONE => stone_like("sandstone", "sandstone", "Sandstone", 0.8),
    RED_SANDSTONE => stone_like("red_sandstone", "red_sandstone", "Red Sandstone", 0.8),
    DEEPSLATE => stone_like("deepslate", "deepslate", "Deepslate", 3.0).drop(Drop::Other("cobbled_deepslate", 1)),
    COBBLED_DEEPSLATE => stone_like("cobbled_deepslate", "cobbled_deepslate", "Cobbled Deepslate", 3.5),
    TUFF => stone_like("tuff", "tuff", "Tuff", 1.5),
    CALCITE => stone_like("calcite", "calcite", "Calcite", 0.75),
    COAL_ORE => ore("coal_ore", "coal_ore", "Coal Ore", Drop::Other("coal", 1)),
    IRON_ORE => ore("iron_ore", "iron_ore", "Iron Ore", Drop::Other("raw_iron", 1)),
    COPPER_ORE => ore("copper_ore", "copper_ore", "Copper Ore", Drop::Other("raw_copper", 3)),
    GOLD_ORE => ore("gold_ore", "gold_ore", "Gold Ore", Drop::Other("raw_gold", 1)),
    REDSTONE_ORE => ore("redstone_ore", "redstone_ore", "Redstone Ore", Drop::Other("redstone", 4)),
    LAPIS_ORE => ore("lapis_ore", "lapis_ore", "Lapis Lazuli Ore", Drop::Other("lapis_lazuli", 5)),
    DIAMOND_ORE => ore("diamond_ore", "diamond_ore", "Diamond Ore", Drop::Other("diamond", 1)),
    EMERALD_ORE => ore("emerald_ore", "emerald_ore", "Emerald Ore", Drop::Other("emerald", 1)),
    DEEPSLATE_COAL_ORE => ore("deepslate_coal_ore", "deepslate_coal_ore", "Deepslate Coal Ore", Drop::Other("coal", 1)),
    DEEPSLATE_IRON_ORE => ore("deepslate_iron_ore", "deepslate_iron_ore", "Deepslate Iron Ore", Drop::Other("raw_iron", 1)),
    DEEPSLATE_COPPER_ORE => ore("deepslate_copper_ore", "deepslate_copper_ore", "Deepslate Copper Ore", Drop::Other("raw_copper", 3)),
    DEEPSLATE_GOLD_ORE => ore("deepslate_gold_ore", "deepslate_gold_ore", "Deepslate Gold Ore", Drop::Other("raw_gold", 1)),
    DEEPSLATE_REDSTONE_ORE => ore("deepslate_redstone_ore", "deepslate_redstone_ore", "Deepslate Redstone Ore", Drop::Other("redstone", 4)),
    DEEPSLATE_LAPIS_ORE => ore("deepslate_lapis_ore", "deepslate_lapis_ore", "Deepslate Lapis Ore", Drop::Other("lapis_lazuli", 5)),
    DEEPSLATE_DIAMOND_ORE => ore("deepslate_diamond_ore", "deepslate_diamond_ore", "Deepslate Diamond Ore", Drop::Other("diamond", 1)),
    OAK_LOG => wood("oak_log", "oak_log", "Oak Log"),
    SPRUCE_LOG => wood("spruce_log", "spruce_log", "Spruce Log"),
    BIRCH_LOG => wood("birch_log", "birch_log", "Birch Log"),
    JUNGLE_LOG => wood("jungle_log", "jungle_log", "Jungle Log"),
    ACACIA_LOG => wood("acacia_log", "acacia_log", "Acacia Log"),
    DARK_OAK_LOG => wood("dark_oak_log", "dark_oak_log", "Dark Oak Log"),
    CHERRY_LOG => wood("cherry_log", "cherry_log", "Cherry Log"),
    MANGROVE_LOG => wood("mangrove_log", "mangrove_log", "Mangrove Log"),
    OAK_LEAVES => leaves("oak_leaves", "oak_leaves", "Oak Leaves", Tint::Foliage),
    SPRUCE_LEAVES => leaves("spruce_leaves", "spruce_leaves", "Spruce Leaves", Tint::Fixed(0x619961)),
    BIRCH_LEAVES => leaves("birch_leaves", "birch_leaves", "Birch Leaves", Tint::Fixed(0x80a755)),
    JUNGLE_LEAVES => leaves("jungle_leaves", "jungle_leaves", "Jungle Leaves", Tint::Foliage),
    ACACIA_LEAVES => leaves("acacia_leaves", "acacia_leaves", "Acacia Leaves", Tint::Foliage),
    DARK_OAK_LEAVES => leaves("dark_oak_leaves", "dark_oak_leaves", "Dark Oak Leaves", Tint::Foliage),
    CHERRY_LEAVES => leaves("cherry_leaves", "cherry_leaves", "Cherry Leaves", Tint::None),
    OAK_PLANKS => wood("oak_planks", "oak_planks", "Oak Planks"),
    SPRUCE_PLANKS => wood("spruce_planks", "spruce_planks", "Spruce Planks"),
    BIRCH_PLANKS => wood("birch_planks", "birch_planks", "Birch Planks"),
    GLASS => cube("glass", "glass", "Glass").hardness(0.3).cutout().drop(Drop::Nothing).sound("glass"),
    SNOW_BLOCK => dirt_like("snow_block", "snow", "Snow Block", 0.2).sound("snow").drop(Drop::Other("snowball", 4)),
    SNOW_LAYER => {
        let mut d = dirt_like("snow_layer", "snow_layer", "Snow", 0.1).shape(Shape::Layer(2)).sound("snow").drop(Drop::Other("snowball", 1));
        d.opaque = false; d.solid = false; d.replaceable = true; d.needs_support = true; d
    },
    ICE => cube("ice", "ice", "Ice").hardness(0.5).tool(Tool::Pickaxe).translucent().drop(Drop::Nothing).sound("glass"),
    PACKED_ICE => cube("packed_ice", "packed_ice", "Packed Ice").hardness(0.5).tool(Tool::Pickaxe).drop(Drop::Nothing).sound("glass"),
    CACTUS => {
        let mut d = cube("cactus", "cactus", "Cactus").hardness(0.4).shape(Shape::Inset).sound("cloth");
        d.opaque = false; d.layer = Layer::Cutout; d.needs_support = true; d
    },
    WATER => liquid("water", "water", "Water").tint(Tint::Water),
    LAVA => liquid("lava", "lava", "Lava").light(15),
    SHORT_GRASS => grass_plant("short_grass", "short_grass", "Short Grass"),
    FERN => grass_plant("fern", "fern", "Fern"),
    DEAD_BUSH => { let mut d = plant("dead_bush", "deadbush", "Dead Bush").drop(Drop::Other("stick", 1)); d.replaceable = true; d },
    DANDELION => plant("dandelion", "dandelion", "Dandelion"),
    POPPY => plant("poppy", "poppy", "Poppy"),
    BLUE_ORCHID => plant("blue_orchid", "blue_orchid", "Blue Orchid"),
    CORNFLOWER => plant("cornflower", "cornflower", "Cornflower"),
    BROWN_MUSHROOM => plant("brown_mushroom", "brown_mushroom", "Brown Mushroom").light(1),
    RED_MUSHROOM => plant("red_mushroom", "red_mushroom", "Red Mushroom"),
    SUGAR_CANE => plant("sugar_cane", "reeds", "Sugar Cane").tint(Tint::Grass),
    PUMPKIN => wood("pumpkin", "pumpkin", "Pumpkin").hardness(1.0).sound("wood"),
    MELON => wood("melon", "melon_block", "Melon").hardness(1.0).drop(Drop::Other("melon_slice", 5)),
    CRAFTING_TABLE => wood("crafting_table", "crafting_table", "Crafting Table").hardness(2.5),
    FURNACE => stone_like("furnace", "furnace", "Furnace", 3.5),
    CHEST => wood("chest", "chest", "Chest").hardness(2.5),
    BOOKSHELF => wood("bookshelf", "bookshelf", "Bookshelf").hardness(1.5).drop(Drop::Other("book", 3)),
    TORCH => {
        let mut d = cube("torch", "torch", "Torch").hardness(0.0).shape(Shape::Torch).light(14).sound("wood");
        d.opaque = false; d.solid = false; d.layer = Layer::Cutout; d.needs_support = true; d
    },
    GLOWSTONE => cube("glowstone", "glowstone", "Glowstone").hardness(0.3).light(15).sound("glass").drop(Drop::Other("glowstone_dust", 3)),
    BRICKS => stone_like("bricks", "brick_block", "Bricks", 2.0),
    STONE_BRICKS => stone_like("stone_bricks", "stone_bricks", "Stone Bricks", 1.5),
    SMOOTH_STONE => stone_like("smooth_stone", "smooth_stone", "Smooth Stone", 2.0),
    OBSIDIAN => stone_like("obsidian", "obsidian", "Obsidian", 50.0),
    WHITE_WOOL => cube("white_wool", "white_wool", "White Wool").hardness(0.8).tool(Tool::Shears).sound("cloth"),
    TERRACOTTA => stone_like("terracotta", "hardened_clay", "Terracotta", 1.25),
    MYCELIUM => dirt_like("mycelium", "mycelium", "Mycelium", 0.6).drop(Drop::Other("dirt", 1)),
    MOSS_BLOCK => dirt_like("moss_block", "moss_block", "Moss Block", 0.1).tool(Tool::Hoe).sound("grass"),
    COAL_BLOCK => stone_like("coal_block", "coal_block", "Block of Coal", 5.0),
    IRON_BLOCK => stone_like("iron_block", "iron_block", "Block of Iron", 5.0).sound("metal"),
    GOLD_BLOCK => stone_like("gold_block", "gold_block", "Block of Gold", 3.0).sound("metal"),
    DIAMOND_BLOCK => stone_like("diamond_block", "diamond_block", "Block of Diamond", 5.0).sound("metal"),
    TNT => cube("tnt", "tnt", "TNT").hardness(0.0).sound("grass"),
    LILY_PAD => {
        let mut d = plant("lily_pad", "waterlily", "Lily Pad").shape(Shape::Layer(1)).tint(Tint::Fixed(0x208030));
        d.solid = true; d
    },
    VINE => { let mut d = plant("vine", "vine", "Vines").tint(Tint::Foliage); d.replaceable = true; d.needs_support = false; d },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_are_consistent() {
        assert_eq!(blocks::AIR.0, 0);
        assert!(blocks::AIR.is_air());
        assert_eq!(blocks::STONE.def().name, "stone");
        assert_eq!(BlockId::by_name("oak_log"), Some(blocks::OAK_LOG));
        let mut names: Vec<_> = BLOCK_DEFS.iter().map(|d| d.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BLOCK_DEFS.len(), "duplicate block names");
    }
}
