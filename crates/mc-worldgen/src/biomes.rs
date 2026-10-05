//! Biome selection from climate parameters and terrain shape.
//!
//! Terrain context decides the broad category first (ocean, river, shore,
//! peaks, slopes, highlands, windswept, swamp, ordinary land). Ordinary land
//! then uses a temperature × humidity grid laid out like a Whittaker
//! diagram: cold+dry → snowy plains, cold+wet → (snowy) taiga, temperate →
//! plains/forests, warm+dry → savanna, warm+wet → jungle, hot+dry → desert.
//! The sign and magnitude of weirdness select rarer variants so that they
//! form patches inside their parent biome.

use mc_core::BiomeId;
use mc_core::biome::biomes as b;

use crate::climate::{Climate, Shape};

/// Temperature band 0 (frozen) ..= 4 (hot).
#[inline]
pub fn temperature_band(t: f32) -> usize {
    match t {
        t if t < -0.42 => 0,
        t if t < -0.14 => 1,
        t if t < 0.18 => 2,
        t if t < 0.48 => 3,
        _ => 4,
    }
}

/// Humidity band 0 (arid) ..= 4 (wet).
#[inline]
pub fn humidity_band(h: f32) -> usize {
    match h {
        h if h < -0.33 => 0,
        h if h < -0.08 => 1,
        h if h < 0.12 => 2,
        h if h < 0.32 => 3,
        _ => 4,
    }
}

/// Continentalness below which the land is a mushroom island.
const MUSHROOM_C: f32 = -1.1;

pub fn pick(cl: &Climate, shape: &Shape) -> BiomeId {
    let h = shape.height;
    let c = cl.continentalness;
    let e = cl.erosion;
    let w = cl.weirdness;
    let pv = cl.pv;
    let ti = temperature_band(cl.temperature);
    let hi = humidity_band(cl.humidity);

    if c < MUSHROOM_C && h > 59.0 {
        return b::MUSHROOM_FIELDS;
    }

    // Open water.
    if h < 61.0 && (c < -0.16 || h < 54.0) && shape.river < 0.3 {
        if h < 43.0 {
            return match ti {
                0 => b::FROZEN_OCEAN,
                1 => b::COLD_OCEAN,
                _ => b::DEEP_OCEAN,
            };
        }
        return match ti {
            0 => b::FROZEN_OCEAN,
            1 => b::COLD_OCEAN,
            2 => b::OCEAN,
            3 => b::LUKEWARM_OCEAN,
            _ => b::WARM_OCEAN,
        };
    }
    if shape.river > 0.4 && h < 63.5 {
        return if ti == 0 { b::FROZEN_RIVER } else { b::RIVER };
    }

    // Shores.
    if c < -0.1 && h < 72.0 {
        if e < -0.35 || (h > 69.0 && e < 0.1) {
            return b::STONY_SHORE;
        }
        if h < 66.5 {
            return match ti {
                0 => b::SNOWY_BEACH,
                _ => b::BEACH,
            };
        }
    }

    // Mountain peaks.
    if h > 150.0 && e < -0.42 && pv > 0.1 {
        return match ti {
            0 | 1 => {
                if w > 0.0 {
                    b::JAGGED_PEAKS
                } else {
                    b::FROZEN_PEAKS
                }
            }
            2 => b::JAGGED_PEAKS,
            4 if hi <= 1 => b::BADLANDS,
            _ => b::STONY_PEAKS,
        };
    }

    // Mountain slopes.
    if h > 120.0 && e < -0.28 {
        return match ti {
            0 => b::SNOWY_SLOPES,
            1 => {
                if hi >= 2 {
                    b::GROVE
                } else {
                    b::SNOWY_SLOPES
                }
            }
            2 => {
                if hi >= 3 {
                    b::GROVE
                } else if w > 0.0 && hi >= 1 {
                    b::CHERRY_GROVE
                } else {
                    b::MEADOW
                }
            }
            3 => {
                if hi >= 3 {
                    b::CHERRY_GROVE
                } else {
                    b::SAVANNA
                }
            }
            _ => b::BADLANDS,
        };
    }

    // Highlands and plateaus.
    if h > 96.0 && e < -0.05 {
        match ti {
            0 => return if hi >= 3 { b::GROVE } else { b::SNOWY_SLOPES },
            1 => return if hi >= 3 { b::TAIGA } else { b::MEADOW },
            2 => {
                if hi <= 1 {
                    return b::MEADOW;
                }
                if hi == 2 && w > 0.2 {
                    return b::CHERRY_GROVE;
                }
            }
            4 => return if hi >= 3 { b::SAVANNA } else { b::BADLANDS },
            _ => {}
        }
    }

    // Windswept, craggy hills.
    if (0.3..0.56).contains(&e) && pv > 0.35 && h > 72.0 && c > -0.08 {
        match ti {
            0..=2 => return b::WINDSWEPT_HILLS,
            3 => return b::SAVANNA,
            _ => {}
        }
    }

    // Swamps on flat, wet lowland.
    if e > 0.55 && h < 66.0 && c > -0.12 && hi >= 2 && (2..=4).contains(&ti) {
        return if ti == 4 || (ti == 3 && hi == 4) {
            b::MANGROVE_SWAMP
        } else {
            b::SWAMP
        };
    }

    // Hot and elevated: badlands mesas.
    if ti == 4 && hi <= 2 && (h > 82.0 || (e < -0.2 && h > 74.0)) {
        return b::BADLANDS;
    }

    middle(ti, hi, w)
}

/// Ordinary land biome from temperature/humidity bands.
fn middle(ti: usize, hi: usize, w: f32) -> BiomeId {
    let variant = w > 0.25;
    match (ti, hi) {
        (0, 0) => {
            if w > 0.38 {
                b::ICE_SPIKES
            } else {
                b::SNOWY_PLAINS
            }
        }
        (0, 1) | (0, 2) => b::SNOWY_PLAINS,
        (0, _) => b::SNOWY_TAIGA,
        (1, 0) => b::PLAINS,
        (1, 1) => {
            if variant {
                b::TAIGA
            } else {
                b::PLAINS
            }
        }
        (1, _) => b::TAIGA,
        (2, 0) => {
            if variant {
                b::SUNFLOWER_PLAINS
            } else {
                b::PLAINS
            }
        }
        (2, 1) => b::PLAINS,
        (2, 2) => {
            if w < -0.45 {
                b::BIRCH_FOREST
            } else {
                b::FOREST
            }
        }
        (2, 3) => b::BIRCH_FOREST,
        (2, _) => b::DARK_FOREST,
        (3, 0) | (3, 1) => b::SAVANNA,
        (3, 2) => {
            if variant {
                b::PLAINS
            } else {
                b::FOREST
            }
        }
        (3, _) => b::JUNGLE,
        (_, 0..=2) => b::DESERT,
        (_, 3) => b::SAVANNA,
        _ => b::JUNGLE,
    }
}

/// True for biomes whose surface is under water (used by the map tool and
/// spawn search).
pub fn is_watery(id: BiomeId) -> bool {
    matches!(
        id,
        b::OCEAN
            | b::DEEP_OCEAN
            | b::WARM_OCEAN
            | b::LUKEWARM_OCEAN
            | b::COLD_OCEAN
            | b::FROZEN_OCEAN
            | b::RIVER
            | b::FROZEN_RIVER
    )
}
