//! Furnace state, smelting recipes and fuel values.

use mc_core::{ItemId, ItemStack};

/// Seconds to smelt one item.
pub const COOK_TIME: f32 = 10.0;

/// One furnace's contents and progress.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Furnace {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    /// Seconds of burn time left from the current fuel item.
    pub burn_left: f32,
    /// Total burn time of the current fuel item (for the flame gauge).
    pub burn_total: f32,
    /// Seconds spent smelting the current input item.
    pub cook: f32,
}

/// (input, output, count) smelting table.
const SMELTING: &[(&str, &str, u8)] = &[
    ("raw_iron", "iron_ingot", 1),
    ("iron_ore", "iron_ingot", 1),
    ("deepslate_iron_ore", "iron_ingot", 1),
    ("raw_gold", "gold_ingot", 1),
    ("gold_ore", "gold_ingot", 1),
    ("deepslate_gold_ore", "gold_ingot", 1),
    ("raw_copper", "copper_ingot", 1),
    ("copper_ore", "copper_ingot", 1),
    ("deepslate_copper_ore", "copper_ingot", 1),
    ("coal_ore", "coal", 1),
    ("deepslate_coal_ore", "coal", 1),
    ("diamond_ore", "diamond", 1),
    ("deepslate_diamond_ore", "diamond", 1),
    ("emerald_ore", "emerald", 1),
    ("lapis_ore", "lapis_lazuli", 1),
    ("deepslate_lapis_ore", "lapis_lazuli", 1),
    ("redstone_ore", "redstone", 1),
    ("deepslate_redstone_ore", "redstone", 1),
    ("sand", "glass", 1),
    ("red_sand", "glass", 1),
    ("cobblestone", "stone", 1),
    ("stone", "smooth_stone", 1),
    ("cobbled_deepslate", "deepslate", 1),
    ("clay_ball", "brick", 1),
    ("clay", "terracotta", 1),
    ("porkchop", "cooked_porkchop", 1),
    ("beef", "cooked_beef", 1),
    ("chicken", "cooked_chicken", 1),
    ("mutton", "cooked_mutton", 1),
];

/// What smelting `item` produces.
pub fn smelt_result(item: ItemId) -> Option<ItemStack> {
    let name = item.name();
    SMELTING
        .iter()
        .find(|(i, _, _)| *i == name)
        .and_then(|(_, o, n)| ItemId::by_name(o).map(|o| ItemStack::new(o, *n)))
}

/// Burn time of a fuel item in seconds.
pub fn fuel_time(item: ItemId) -> Option<f32> {
    let n = item.name();
    let t = match n {
        "coal" => 80.0,
        "coal_block" => 800.0,
        "stick" => 5.0,
        "crafting_table" | "chest" | "bookshelf" => 15.0,
        _ if n.ends_with("_planks") || n.ends_with("_log") => 15.0,
        _ if n.starts_with("wooden_") => 10.0,
        _ => return None,
    };
    Some(t)
}

impl Furnace {
    fn can_smelt(&self) -> Option<ItemStack> {
        let input = self.input?;
        let result = smelt_result(input.item)?;
        match self.output {
            None => Some(result),
            Some(o) if o.item == result.item && o.count + result.count <= o.item.max_stack() => {
                Some(result)
            }
            _ => None,
        }
    }

    pub fn burning(&self) -> bool {
        self.burn_left > 0.0
    }

    /// 0..1 flame gauge.
    pub fn burn_fraction(&self) -> f32 {
        if self.burn_total > 0.0 {
            (self.burn_left / self.burn_total).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// 0..1 progress arrow.
    pub fn cook_fraction(&self) -> f32 {
        (self.cook / COOK_TIME).clamp(0.0, 1.0)
    }

    /// Advance by `dt` seconds (in steps so long frames stay accurate).
    pub fn tick(&mut self, mut dt: f32) {
        while dt > 0.0 {
            let step = dt.min(0.25);
            dt -= step;
            self.step(step);
        }
    }

    fn step(&mut self, dt: f32) {
        let smeltable = self.can_smelt().is_some();
        if self.burn_left <= 0.0
            && smeltable
            && let Some(f) = self.fuel
            && let Some(t) = fuel_time(f.item)
        {
            self.burn_left = t;
            self.burn_total = t;
            self.fuel = (f.count > 1).then_some(ItemStack {
                count: f.count - 1,
                ..f
            });
        }
        if self.burn_left > 0.0 {
            self.burn_left = (self.burn_left - dt).max(0.0);
            if smeltable {
                self.cook += dt;
                if self.cook >= COOK_TIME {
                    self.cook = 0.0;
                    if let (Some(result), Some(input)) = (self.can_smelt(), self.input) {
                        self.output = Some(match self.output {
                            Some(o) => ItemStack {
                                count: o.count + result.count,
                                ..o
                            },
                            None => result,
                        });
                        self.input = (input.count > 1).then_some(ItemStack {
                            count: input.count - 1,
                            ..input
                        });
                    }
                }
            } else {
                self.cook = 0.0;
            }
        } else {
            // Out of fuel: progress cools down.
            self.cook = (self.cook - 2.0 * dt).max(0.0);
        }
    }

    /// Everything inside (for dropping when the furnace is broken).
    pub fn contents(&self) -> Vec<ItemStack> {
        [self.input, self.fuel, self.output]
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(n: &str, c: u8) -> Option<ItemStack> {
        Some(ItemStack::new(ItemId::by_name(n).unwrap(), c))
    }

    #[test]
    fn tables_resolve() {
        for (i, o, _) in SMELTING {
            assert!(ItemId::by_name(i).is_some(), "{i}");
            assert!(ItemId::by_name(o).is_some(), "{o}");
        }
        assert!(fuel_time(ItemId::by_name("coal").unwrap()).is_some());
        assert!(fuel_time(ItemId::by_name("birch_planks").unwrap()).is_some());
        assert!(fuel_time(ItemId::by_name("dirt").unwrap()).is_none());
    }

    #[test]
    fn smelts_sand_into_glass() {
        let mut f = Furnace {
            input: st("sand", 3),
            fuel: st("coal", 1),
            ..Default::default()
        };
        f.tick(COOK_TIME + 0.1);
        assert_eq!(f.output, st("glass", 1));
        assert_eq!(f.input, st("sand", 2));
        assert_eq!(f.fuel, None);
        assert!(f.burning());
        f.tick(COOK_TIME * 2.0 + 0.5);
        assert_eq!(f.output, st("glass", 3));
        assert_eq!(f.input, None);
        // Nothing to smelt: progress stays at zero, fuel keeps burning down.
        assert_eq!(f.cook, 0.0);
    }

    #[test]
    fn no_fuel_no_progress() {
        let mut f = Furnace {
            input: st("raw_iron", 1),
            ..Default::default()
        };
        f.tick(30.0);
        assert_eq!(f.output, None);
        // Not smeltable: fuel is not consumed.
        let mut f = Furnace {
            input: st("dirt", 1),
            fuel: st("coal", 2),
            ..Default::default()
        };
        f.tick(5.0);
        assert_eq!(f.fuel, st("coal", 2));
        assert!(!f.burning());
    }

    #[test]
    fn output_must_match() {
        let mut f = Furnace {
            input: st("sand", 1),
            fuel: st("coal", 1),
            output: st("stone", 5),
            ..Default::default()
        };
        f.tick(20.0);
        assert_eq!(f.output, st("stone", 5));
        assert_eq!(f.input, st("sand", 1));
    }
}
