//! Biome registry. Biomes are stored per (x, z) column in each chunk.
//!
//! `temperature`/`downfall` drive the grass/foliage colormap lookup (the asset
//! crate samples `textures/colormap/*.png` with them). Append new biomes at
//! the end of the list.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(transparent)]
pub struct BiomeId(pub u8);

impl BiomeId {
    pub fn def(self) -> &'static BiomeDef {
        BIOME_DEFS.get(self.0 as usize).unwrap_or(&BIOME_DEFS[0])
    }
    pub fn by_name(name: &str) -> Option<BiomeId> {
        BIOME_DEFS
            .iter()
            .position(|b| b.name == name)
            .map(|i| BiomeId(i as u8))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BiomeDef {
    pub name: &'static str,
    pub temperature: f32,
    pub downfall: f32,
    /// Water colour 0xRRGGBB.
    pub water_color: u32,
    /// Optional explicit grass/foliage colours overriding the colormap.
    pub grass_color: Option<u32>,
    pub foliage_color: Option<u32>,
    /// Sky colour 0xRRGGBB at noon.
    pub sky_color: u32,
    /// Fog colour 0xRRGGBB at noon.
    pub fog_color: u32,
    /// Precipitation falls as snow.
    pub snowy: bool,
}

const fn b(name: &'static str, temperature: f32, downfall: f32) -> BiomeDef {
    BiomeDef {
        name,
        temperature,
        downfall,
        water_color: 0x44AFF5,
        grass_color: None,
        foliage_color: None,
        sky_color: 0x78A7FF,
        fog_color: 0xC0D8FF,
        snowy: temperature < 0.15,
    }
}

const fn water(mut d: BiomeDef, c: u32) -> BiomeDef {
    d.water_color = c;
    d
}

const fn colors(mut d: BiomeDef, grass: u32, foliage: u32) -> BiomeDef {
    d.grass_color = Some(grass);
    d.foliage_color = Some(foliage);
    d
}

macro_rules! define_biomes {
    ($($id:ident => $def:expr),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, dead_code)]
        #[repr(u8)]
        enum BiomeIndex { $($id),* }
        pub mod biomes {
            use super::{BiomeId, BiomeIndex};
            $(pub const $id: BiomeId = BiomeId(BiomeIndex::$id as u8);)*
        }
        pub static BIOME_DEFS: &[BiomeDef] = &[$($def),*];
    };
}

define_biomes! {
    PLAINS => b("plains", 0.8, 0.4),
    SUNFLOWER_PLAINS => b("sunflower_plains", 0.8, 0.4),
    FOREST => b("forest", 0.7, 0.8),
    BIRCH_FOREST => b("birch_forest", 0.6, 0.6),
    DARK_FOREST => b("dark_forest", 0.7, 0.8),
    TAIGA => water(b("taiga", 0.25, 0.8), 0x287082),
    SNOWY_TAIGA => water(b("snowy_taiga", -0.5, 0.4), 0x205E83),
    SNOWY_PLAINS => water(b("snowy_plains", 0.0, 0.5), 0x14559B),
    ICE_SPIKES => water(b("ice_spikes", 0.0, 0.5), 0x14559B),
    DESERT => water(b("desert", 2.0, 0.0), 0x32A598),
    SAVANNA => water(b("savanna", 2.0, 0.0), 0x2C8B9C),
    BADLANDS => colors(water(b("badlands", 2.0, 0.0), 0x4E7F81), 0x90814D, 0x9E814D),
    JUNGLE => water(b("jungle", 0.95, 0.9), 0x14A2C5),
    SWAMP => colors(water(b("swamp", 0.8, 0.9), 0x4C6559), 0x6A7039, 0x6A7039),
    MANGROVE_SWAMP => water(b("mangrove_swamp", 0.8, 0.9), 0x3A7A6A),
    MEADOW => water(b("meadow", 0.5, 0.8), 0x0E4ECF),
    CHERRY_GROVE => colors(water(b("cherry_grove", 0.5, 0.8), 0x5DB7EF), 0xB6DB61, 0xB6DB61),
    GROVE => b("grove", -0.2, 0.8),
    SNOWY_SLOPES => b("snowy_slopes", -0.3, 0.9),
    JAGGED_PEAKS => b("jagged_peaks", -0.7, 0.9),
    FROZEN_PEAKS => b("frozen_peaks", -0.7, 0.9),
    STONY_PEAKS => b("stony_peaks", 1.0, 0.3),
    WINDSWEPT_HILLS => water(b("windswept_hills", 0.2, 0.3), 0x007BF7),
    BEACH => water(b("beach", 0.8, 0.4), 0x157CAB),
    SNOWY_BEACH => water(b("snowy_beach", 0.05, 0.3), 0x1463A5),
    STONY_SHORE => water(b("stony_shore", 0.2, 0.3), 0x0D67BB),
    RIVER => water(b("river", 0.5, 0.5), 0x0084FF),
    FROZEN_RIVER => water(b("frozen_river", 0.0, 0.5), 0x185390),
    OCEAN => water(b("ocean", 0.5, 0.5), 0x1787D4),
    DEEP_OCEAN => water(b("deep_ocean", 0.5, 0.5), 0x1787D4),
    WARM_OCEAN => water(b("warm_ocean", 0.5, 0.5), 0x02B0E5),
    LUKEWARM_OCEAN => water(b("lukewarm_ocean", 0.5, 0.5), 0x0D96DB),
    COLD_OCEAN => water(b("cold_ocean", 0.5, 0.5), 0x2080C9),
    FROZEN_OCEAN => water(b("frozen_ocean", 0.0, 0.5), 0x2570B5),
    MUSHROOM_FIELDS => water(b("mushroom_fields", 0.9, 1.0), 0x8A8997),
    DRIPSTONE_CAVES => b("dripstone_caves", 0.8, 0.4),
    LUSH_CAVES => b("lush_caves", 0.5, 0.5),
    DEEP_DARK => b("deep_dark", 0.8, 0.4),
}
