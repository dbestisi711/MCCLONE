//! Shared helpers for the worldgen dev tools: colour palettes, simple
//! argument parsing and PNG output.

#![allow(dead_code)]

use mc_core::biome::biomes as b;
use mc_core::{BiomeId, BlockId, blocks};

/// Approximate top-down colour of a block.
pub fn block_color(id: BlockId) -> [u8; 3] {
    match id {
        blocks::AIR => [200, 220, 255],
        blocks::STONE | blocks::COBBLESTONE => [125, 125, 125],
        blocks::GRANITE => [154, 106, 89],
        blocks::DIORITE => [190, 190, 192],
        blocks::ANDESITE => [136, 136, 138],
        blocks::GRASS_BLOCK => [95, 159, 53],
        blocks::DIRT => [134, 96, 67],
        blocks::COARSE_DIRT => [119, 85, 59],
        blocks::PODZOL => [91, 63, 24],
        blocks::BEDROCK => [40, 40, 40],
        blocks::SAND => [219, 207, 163],
        blocks::RED_SAND => [190, 102, 33],
        blocks::GRAVEL => [131, 127, 126],
        blocks::CLAY => [160, 166, 179],
        blocks::MUD => [60, 57, 60],
        blocks::SANDSTONE => [216, 203, 155],
        blocks::RED_SANDSTONE => [186, 99, 29],
        blocks::DEEPSLATE => [80, 80, 86],
        blocks::TUFF => [108, 109, 102],
        blocks::CALCITE => [223, 224, 220],
        blocks::COAL_ORE | blocks::DEEPSLATE_COAL_ORE => [20, 20, 20],
        blocks::IRON_ORE | blocks::DEEPSLATE_IRON_ORE => [216, 175, 147],
        blocks::COPPER_ORE | blocks::DEEPSLATE_COPPER_ORE => [224, 128, 80],
        blocks::GOLD_ORE | blocks::DEEPSLATE_GOLD_ORE => [252, 238, 75],
        blocks::REDSTONE_ORE | blocks::DEEPSLATE_REDSTONE_ORE => [255, 0, 0],
        blocks::LAPIS_ORE | blocks::DEEPSLATE_LAPIS_ORE => [30, 60, 220],
        blocks::DIAMOND_ORE | blocks::DEEPSLATE_DIAMOND_ORE => [90, 245, 230],
        blocks::EMERALD_ORE => [20, 230, 80],
        blocks::RAW_IRON_BLOCK => [166, 135, 107],
        blocks::RAW_COPPER_BLOCK => [154, 105, 79],
        blocks::OAK_LOG | blocks::DARK_OAK_LOG | blocks::SPRUCE_LOG | blocks::JUNGLE_LOG => {
            [102, 81, 51]
        }
        blocks::BIRCH_LOG => [216, 215, 210],
        blocks::ACACIA_LOG => [103, 96, 86],
        blocks::CHERRY_LOG => [54, 33, 44],
        blocks::MANGROVE_LOG => [84, 66, 36],
        blocks::OAK_LEAVES => [60, 130, 30],
        blocks::DARK_OAK_LEAVES => [40, 100, 20],
        blocks::SPRUCE_LEAVES => [55, 90, 55],
        blocks::BIRCH_LEAVES => [110, 150, 70],
        blocks::JUNGLE_LEAVES => [45, 140, 20],
        blocks::ACACIA_LEAVES => [100, 140, 30],
        blocks::CHERRY_LEAVES => [235, 170, 200],
        blocks::MANGROVE_LEAVES => [70, 120, 30],
        blocks::SNOW_BLOCK | blocks::SNOW_LAYER => [248, 252, 252],
        blocks::ICE => [145, 183, 253],
        blocks::PACKED_ICE => [141, 180, 250],
        blocks::CACTUS => [85, 127, 43],
        blocks::WATER => [50, 90, 200],
        blocks::LAVA => [230, 100, 20],
        blocks::SHORT_GRASS | blocks::FERN => [90, 150, 50],
        blocks::DEAD_BUSH => [130, 100, 50],
        blocks::DANDELION => [240, 220, 40],
        blocks::POPPY => [220, 30, 30],
        blocks::BLUE_ORCHID => [40, 160, 220],
        blocks::CORNFLOWER => [70, 100, 230],
        blocks::ALLIUM => [180, 110, 220],
        blocks::AZURE_BLUET => [220, 230, 240],
        blocks::OXEYE_DAISY => [235, 235, 210],
        blocks::LILY_OF_THE_VALLEY => [240, 240, 240],
        blocks::BROWN_MUSHROOM | blocks::BROWN_MUSHROOM_BLOCK => [150, 110, 80],
        blocks::RED_MUSHROOM | blocks::RED_MUSHROOM_BLOCK => [200, 40, 40],
        blocks::MUSHROOM_STEM => [205, 200, 190],
        blocks::SUGAR_CANE => [140, 190, 90],
        blocks::PUMPKIN => [220, 130, 20],
        blocks::MELON => [110, 150, 30],
        blocks::MYCELIUM => [111, 99, 105],
        blocks::MOSS_BLOCK => [89, 109, 45],
        blocks::LILY_PAD => [32, 128, 48],
        blocks::VINE => [60, 120, 30],
        blocks::TERRACOTTA => [152, 94, 67],
        blocks::WHITE_TERRACOTTA => [209, 178, 161],
        blocks::ORANGE_TERRACOTTA => [161, 83, 37],
        blocks::YELLOW_TERRACOTTA => [186, 133, 35],
        blocks::LIGHT_GRAY_TERRACOTTA => [135, 107, 98],
        blocks::BROWN_TERRACOTTA => [77, 51, 35],
        blocks::RED_TERRACOTTA => [143, 61, 46],
        blocks::MANGROVE_ROOTS | blocks::MUDDY_MANGROVE_ROOTS => [74, 59, 38],
        blocks::MOSSY_COBBLESTONE => [100, 120, 90],
        _ => [255, 0, 255],
    }
}

/// Map colour for a biome.
pub fn biome_color(id: BiomeId) -> [u8; 3] {
    match id {
        b::PLAINS => [141, 179, 96],
        b::SUNFLOWER_PLAINS => [181, 219, 96],
        b::FOREST => [5, 102, 33],
        b::BIRCH_FOREST => [48, 116, 68],
        b::DARK_FOREST => [64, 81, 26],
        b::TAIGA => [11, 102, 89],
        b::SNOWY_TAIGA => [49, 85, 74],
        b::SNOWY_PLAINS => [255, 255, 255],
        b::ICE_SPIKES => [180, 220, 220],
        b::DESERT => [250, 148, 24],
        b::SAVANNA => [189, 178, 95],
        b::BADLANDS => [217, 69, 21],
        b::JUNGLE => [83, 123, 9],
        b::SWAMP => [7, 249, 178],
        b::MANGROVE_SWAMP => [36, 196, 142],
        b::MEADOW => [131, 187, 109],
        b::CHERRY_GROVE => [255, 145, 200],
        b::GROVE => [71, 114, 108],
        b::SNOWY_SLOPES => [196, 196, 196],
        b::JAGGED_PEAKS => [220, 220, 200],
        b::FROZEN_PEAKS => [176, 179, 206],
        b::STONY_PEAKS => [123, 143, 116],
        b::WINDSWEPT_HILLS => [96, 96, 96],
        b::BEACH => [250, 222, 85],
        b::SNOWY_BEACH => [250, 240, 192],
        b::STONY_SHORE => [162, 162, 132],
        b::RIVER => [0, 0, 255],
        b::FROZEN_RIVER => [160, 160, 255],
        b::OCEAN => [0, 0, 112],
        b::DEEP_OCEAN => [0, 0, 48],
        b::WARM_OCEAN => [0, 0, 172],
        b::LUKEWARM_OCEAN => [0, 0, 144],
        b::COLD_OCEAN => [32, 32, 112],
        b::FROZEN_OCEAN => [112, 112, 214],
        b::MUSHROOM_FIELDS => [255, 0, 255],
        _ => [128, 0, 128],
    }
}

/// Parse `--name value` style arguments with defaults.
pub struct Args(pub Vec<String>);

impl Args {
    pub fn new() -> Self {
        Args(std::env::args().skip(1).collect())
    }
    pub fn get<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        let key = format!("--{name}");
        self.0
            .iter()
            .position(|a| *a == key)
            .and_then(|i| self.0.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    pub fn flag(&self, name: &str) -> bool {
        let key = format!("--{name}");
        self.0.iter().any(|a| *a == key)
    }
}

pub fn save_png(path: &str, w: u32, h: u32, rgb: &[u8]) {
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    image::save_buffer(path, rgb, w, h, image::ExtendedColorType::Rgb8).expect("write png");
    println!("wrote {path} ({w}x{h})");
}

pub fn shade(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
    ]
}

pub fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}
