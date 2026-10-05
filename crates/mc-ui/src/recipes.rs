//! Crafting recipes: shaped (pattern, matched anywhere in the grid and
//! mirrored) and shapeless (any arrangement), with ingredient tags such as
//! "any planks".

use mc_core::{ItemId, ItemStack};

/// One ingredient: any of these items.
#[derive(Clone, Debug, PartialEq)]
pub struct Ingredient(pub Vec<ItemId>);

impl Ingredient {
    pub fn matches(&self, item: ItemId) -> bool {
        self.0.contains(&item)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecipeKind {
    /// `w`×`h` pattern, row-major, `None` = must be empty.
    Shaped {
        w: usize,
        h: usize,
        pattern: Vec<Option<Ingredient>>,
    },
    Shapeless(Vec<Ingredient>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub kind: RecipeKind,
    pub result: ItemStack,
}

/// A crafting grid view: `w`×`h` slots, row-major.
#[derive(Clone, Copy, Debug)]
pub struct Grid<'a> {
    pub slots: &'a [Option<ItemStack>],
    pub w: usize,
    pub h: usize,
}

impl Grid<'_> {
    fn get(&self, x: usize, y: usize) -> Option<ItemStack> {
        self.slots[y * self.w + x]
    }

    /// Bounding box of the non-empty slots: (x0, y0, w, h).
    fn bounds(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.get(x, y).is_some() {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != usize::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }
}

impl Recipe {
    pub fn matches(&self, grid: Grid) -> bool {
        match &self.kind {
            RecipeKind::Shaped { w, h, pattern } => {
                let Some((bx, by, bw, bh)) = grid.bounds() else {
                    return false;
                };
                if bw != *w || bh != *h {
                    return false;
                }
                let check = |mirror: bool| {
                    (0..bh).all(|y| {
                        (0..bw).all(|x| {
                            let px = if mirror { bw - 1 - x } else { x };
                            let want = &pattern[y * w + px];
                            match (want, grid.get(bx + x, by + y)) {
                                (None, None) => true,
                                (Some(ing), Some(s)) => ing.matches(s.item),
                                _ => false,
                            }
                        })
                    })
                };
                check(false) || check(true)
            }
            RecipeKind::Shapeless(ings) => {
                let items: Vec<ItemId> = grid.slots.iter().flatten().map(|s| s.item).collect();
                if items.len() != ings.len() {
                    return false;
                }
                // Small bipartite match (≤ 9 items) by backtracking.
                fn assign(
                    i: usize,
                    items: &[ItemId],
                    ings: &[Ingredient],
                    used: &mut [bool],
                ) -> bool {
                    if i == items.len() {
                        return true;
                    }
                    for (j, ing) in ings.iter().enumerate() {
                        if !used[j] && ing.matches(items[i]) {
                            used[j] = true;
                            if assign(i + 1, items, ings, used) {
                                return true;
                            }
                            used[j] = false;
                        }
                    }
                    false
                }
                let mut used = vec![false; ings.len()];
                assign(0, &items, ings, &mut used)
            }
        }
    }
}

pub struct RecipeBook {
    pub recipes: Vec<Recipe>,
}

impl RecipeBook {
    pub fn empty() -> Self {
        RecipeBook {
            recipes: Vec::new(),
        }
    }

    /// The first recipe matching the grid.
    pub fn find(&self, grid: Grid) -> Option<&Recipe> {
        if grid.slots.iter().all(Option::is_none) {
            return None;
        }
        self.recipes.iter().find(|r| r.matches(grid))
    }

    /// Result of crafting with the grid's current contents.
    pub fn result(&self, slots: &[Option<ItemStack>], w: usize, h: usize) -> Option<ItemStack> {
        self.find(Grid { slots, w, h }).map(|r| r.result)
    }

    /// Add a shaped recipe. `rows` use single characters; `keys` maps each
    /// character to an item name or `#tag`. Returns false (and skips the
    /// recipe) when a name does not resolve.
    pub fn shaped(
        &mut self,
        rows: &[&str],
        keys: &[(char, &str)],
        result: &str,
        count: u8,
    ) -> bool {
        let h = rows.len();
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut pattern = Vec::with_capacity(w * h);
        for row in rows {
            let chars: Vec<char> = row.chars().collect();
            for x in 0..w {
                let c = chars.get(x).copied().unwrap_or(' ');
                if c == ' ' {
                    pattern.push(None);
                    continue;
                }
                let Some((_, name)) = keys.iter().find(|(k, _)| *k == c) else {
                    log::warn!("recipe for {result}: no key for '{c}'");
                    return false;
                };
                match ingredient(name) {
                    Some(i) => pattern.push(Some(i)),
                    None => return false,
                }
            }
        }
        let Some(item) = ItemId::by_name(result) else {
            log::warn!("recipe result {result} is not a registered item");
            return false;
        };
        self.recipes.push(Recipe {
            kind: RecipeKind::Shaped { w, h, pattern },
            result: ItemStack::new(item, count),
        });
        true
    }

    pub fn shapeless(&mut self, ingredients: &[&str], result: &str, count: u8) -> bool {
        let mut ings = Vec::new();
        for n in ingredients {
            match ingredient(n) {
                Some(i) => ings.push(i),
                None => return false,
            }
        }
        let Some(item) = ItemId::by_name(result) else {
            log::warn!("recipe result {result} is not a registered item");
            return false;
        };
        self.recipes.push(Recipe {
            kind: RecipeKind::Shapeless(ings),
            result: ItemStack::new(item, count),
        });
        true
    }

    /// The game's recipe table.
    pub fn standard() -> Self {
        let mut b = RecipeBook::empty();
        for (log, planks) in WOODS {
            b.shapeless(&[log], planks, 4);
        }
        b.shaped(&["P", "P"], &[('P', "#planks")], "stick", 4);
        b.shaped(&["PP", "PP"], &[('P', "#planks")], "crafting_table", 1);
        b.shaped(
            &["CCC", "C C", "CCC"],
            &[('C', "#stone_tool_materials")],
            "furnace",
            1,
        );
        b.shaped(&["PPP", "P P", "PPP"], &[('P', "#planks")], "chest", 1);
        b.shaped(&["C", "S"], &[('C', "coal"), ('S', "stick")], "torch", 4);

        // Tools.
        for (mat, prefix) in [
            ("#planks", "wooden"),
            ("#stone_tool_materials", "stone"),
            ("iron_ingot", "iron"),
            ("diamond", "diamond"),
        ] {
            let keys = [('X', mat), ('#', "stick")];
            b.shaped(
                &["XXX", " # ", " # "],
                &keys,
                &format!("{prefix}_pickaxe"),
                1,
            );
            b.shaped(&["XX", "X#", " #"], &keys, &format!("{prefix}_axe"), 1);
            b.shaped(&["X", "#", "#"], &keys, &format!("{prefix}_shovel"), 1);
            b.shaped(&["X", "X", "#"], &keys, &format!("{prefix}_sword"), 1);
        }
        b.shaped(&[" I", "I "], &[('I', "iron_ingot")], "shears", 1);

        // Food.
        b.shaped(&["WWW"], &[('W', "wheat")], "bread", 1);

        // Storage blocks and back.
        for (block, item) in [
            ("iron_block", "iron_ingot"),
            ("gold_block", "gold_ingot"),
            ("diamond_block", "diamond"),
            ("coal_block", "coal"),
            ("melon", "melon_slice"),
        ] {
            b.shaped(&["XXX", "XXX", "XXX"], &[('X', item)], block, 1);
            if block != "melon" {
                b.shapeless(&[block], item, 9);
            }
        }
        b.shaped(&["III", "III", "III"], &[('I', "ice")], "packed_ice", 1);

        // Building blocks.
        b.shaped(&["BB", "BB"], &[('B', "brick")], "bricks", 1);
        b.shaped(&["SS", "SS"], &[('S', "stone")], "stone_bricks", 4);
        b.shaped(&["SS", "SS"], &[('S', "sand")], "sandstone", 1);
        b.shaped(&["SS", "SS"], &[('S', "red_sand")], "red_sandstone", 1);
        b.shaped(&["GG", "GG"], &[('G', "glowstone_dust")], "glowstone", 1);
        b.shaped(&["SS", "SS"], &[('S', "snowball")], "snow_block", 1);
        b.shaped(&["CC", "CC"], &[('C', "clay_ball")], "clay", 1);
        b.shaped(&["SS", "SS"], &[('S', "string")], "white_wool", 1);
        b.shaped(
            &["DG", "GD"],
            &[('D', "dirt"), ('G', "gravel")],
            "coarse_dirt",
            4,
        );
        b.shapeless(&["cobblestone", "vine"], "mossy_cobblestone", 1);
        b.shaped(&["SSS"], &[('S', "snow_block")], "snow_layer", 6);

        // Misc.
        b.shaped(
            &["PPP", "BBB", "PPP"],
            &[('P', "#planks"), ('B', "book")],
            "bookshelf",
            1,
        );
        b.shaped(&["SSS"], &[('S', "sugar_cane")], "paper", 3);
        b.shapeless(&["paper", "paper", "paper", "leather"], "book", 1);
        b.shaped(
            &["GSG", "SGS", "GSG"],
            &[('G', "gunpowder"), ('S', "#sand")],
            "tnt",
            1,
        );
        b
    }
}

/// (log, planks) pairs.
const WOODS: [(&str, &str); 8] = [
    ("oak_log", "oak_planks"),
    ("spruce_log", "spruce_planks"),
    ("birch_log", "birch_planks"),
    ("jungle_log", "jungle_planks"),
    ("acacia_log", "acacia_planks"),
    ("dark_oak_log", "dark_oak_planks"),
    ("cherry_log", "cherry_planks"),
    ("mangrove_log", "mangrove_planks"),
];

/// Resolve an item name or `#tag` to an ingredient.
fn ingredient(name: &str) -> Option<Ingredient> {
    let names: Vec<&str> = match name {
        "#planks" => WOODS.iter().map(|(_, p)| *p).collect(),
        "#logs" => WOODS.iter().map(|(l, _)| *l).collect(),
        "#stone_tool_materials" => vec!["cobblestone", "cobbled_deepslate"],
        "#sand" => vec!["sand", "red_sand"],
        n => vec![n],
    };
    let items: Vec<ItemId> = names.iter().filter_map(|n| ItemId::by_name(n)).collect();
    if items.is_empty() {
        log::warn!("recipe ingredient {name} does not resolve to any item");
        return None;
    }
    Some(Ingredient(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> ItemId {
        ItemId::by_name(n).unwrap_or_else(|| panic!("no item {n}"))
    }

    fn s(n: &str) -> Option<ItemStack> {
        Some(ItemStack::new(id(n), 1))
    }

    fn grid3(cells: [Option<ItemStack>; 9]) -> Vec<Option<ItemStack>> {
        cells.to_vec()
    }

    #[test]
    fn standard_table_resolves() {
        let b = RecipeBook::standard();
        assert!(b.recipes.len() >= 40, "only {} recipes", b.recipes.len());
    }

    #[test]
    fn planks_from_log_anywhere() {
        let b = RecipeBook::standard();
        for i in 0..9 {
            let mut g = [None; 9];
            g[i] = s("birch_log");
            let r = b.result(&g, 3, 3).unwrap();
            assert_eq!(r.item, id("birch_planks"));
            assert_eq!(r.count, 4);
        }
        let mut g = [None; 4];
        g[3] = s("oak_log");
        assert_eq!(b.result(&g, 2, 2).unwrap().item, id("oak_planks"));
    }

    #[test]
    fn sticks_match_in_any_column_and_mixed_planks() {
        let b = RecipeBook::standard();
        for x in 0..3 {
            for y in 0..2 {
                let mut g = [None; 9];
                g[y * 3 + x] = s("oak_planks");
                g[(y + 1) * 3 + x] = s("spruce_planks");
                let r = b.result(&g, 3, 3).unwrap();
                assert_eq!((r.item, r.count), (id("stick"), 4));
            }
        }
        // 2x2 grid too.
        let g = [s("oak_planks"), None, s("oak_planks"), None];
        assert_eq!(b.result(&g, 2, 2).unwrap().item, id("stick"));
        // Horizontal planks are not sticks.
        let g = [s("oak_planks"), s("oak_planks"), None, None];
        assert_eq!(b.result(&g, 2, 2), None);
    }

    #[test]
    fn shaped_recipes_mirror() {
        let b = RecipeBook::standard();
        let (i, k) = (s("iron_ingot"), s("stick"));
        let axe = grid3([i, i, None, i, k, None, None, k, None]);
        let mirrored = grid3([None, i, i, None, k, i, None, k, None]);
        assert_eq!(b.result(&axe, 3, 3).unwrap().item, id("iron_axe"));
        assert_eq!(b.result(&mirrored, 3, 3).unwrap().item, id("iron_axe"));
        // Shears both ways.
        let sh = [None, i, i, None];
        assert_eq!(b.result(&sh, 2, 2).unwrap().item, id("shears"));
        let sh = [i, None, None, i];
        assert_eq!(b.result(&sh, 2, 2).unwrap().item, id("shears"));
    }

    #[test]
    fn extra_items_break_the_match() {
        let b = RecipeBook::standard();
        let (c, k) = (s("diamond"), s("stick"));
        let pick = grid3([c, c, c, None, k, None, None, k, None]);
        assert_eq!(b.result(&pick, 3, 3).unwrap().item, id("diamond_pickaxe"));
        let mut extra = pick.clone();
        extra[3] = s("dirt");
        assert_eq!(b.result(&extra, 3, 3), None);
        // A pickaxe does not fit a 2x2 grid.
        assert_eq!(b.result(&[c, c, None, k], 2, 2), None);
    }

    #[test]
    fn shapeless_any_order() {
        let b = RecipeBook::standard();
        let g = grid3([
            None,
            s("leather"),
            None,
            s("paper"),
            None,
            s("paper"),
            None,
            None,
            s("paper"),
        ]);
        assert_eq!(b.result(&g, 3, 3).unwrap().item, id("book"));
        let g = [s("vine"), s("cobblestone"), None, None];
        assert_eq!(b.result(&g, 2, 2).unwrap().item, id("mossy_cobblestone"));
        // Missing one paper → nothing.
        let g = [s("leather"), s("paper"), s("paper"), None];
        assert_eq!(b.result(&g, 2, 2), None);
    }

    #[test]
    fn storage_blocks_round_trip() {
        let b = RecipeBook::standard();
        let i = s("gold_ingot");
        assert_eq!(b.result(&[i; 9], 3, 3).unwrap().item, id("gold_block"));
        let r = b
            .result(&[s("gold_block"), None, None, None], 2, 2)
            .unwrap();
        assert_eq!((r.item, r.count), (id("gold_ingot"), 9));
    }

    #[test]
    fn misc_recipes() {
        let b = RecipeBook::standard();
        let (c, k, p) = (s("coal"), s("stick"), s("oak_planks"));
        let r = b.result(&[None, c, None, k], 2, 2).unwrap();
        assert_eq!((r.item, r.count), (id("torch"), 4));
        assert_eq!(
            b.result(&[p, p, p, p], 2, 2).unwrap().item,
            id("crafting_table")
        );
        let st = s("cobblestone");
        let furnace = grid3([st, st, st, st, None, st, st, st, st]);
        assert_eq!(b.result(&furnace, 3, 3).unwrap().item, id("furnace"));
        let w = s("wheat");
        let bread = grid3([None, None, None, None, None, None, w, w, w]);
        assert_eq!(b.result(&bread, 3, 3).unwrap().item, id("bread"));
        let (g, sa, rs) = (s("gunpowder"), s("sand"), s("red_sand"));
        let tnt = grid3([g, sa, g, rs, g, sa, g, sa, g]);
        assert_eq!(b.result(&tnt, 3, 3).unwrap().item, id("tnt"));
        assert_eq!(b.result(&[None; 9], 3, 3), None);
    }
}
