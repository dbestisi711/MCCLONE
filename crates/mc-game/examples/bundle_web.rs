//! Build `pack.bin` for the web version: the resource-pack files the game
//! reads, in one bundle the browser fetches at start-up.
//!
//! Usage: cargo run --release -p mc-game --example bundle_web -- <out/pack.bin>
//!
//! Files are found two ways: everything the asset loader and UI read while
//! starting up is recorded, and folders the renderer and mobs load lazily
//! (entity textures, sky textures, models, animations) are included whole.
//! PBR companion files (`*_mers`, normal maps, texture sets) in those
//! folders are skipped.

use std::collections::BTreeSet;
use std::path::Path;

use mc_assets::{Assets, Pack};
use mc_core::Inventory;
use mc_core::render_types::UiDrawList;
use mc_ui::{HudInfo, Screen, Ui};

const WHOLE_DIRS: &[&str] = &[
    "textures/entity",
    "textures/environment",
    "textures/misc",
    "textures/colormap",
    "models",
    "entity",
    "animations",
    "animation_controllers",
];

fn skip(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.contains("_mers.")
        || name.contains("_mer.")
        || name.contains("_normal.")
        || name.contains("_heightmap.")
        || name.ends_with(".texture_set.json")
}

fn walk(root: &Path, rel: &str, out: &mut BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = format!("{rel}/{name}");
        if e.path().is_dir() {
            walk(root, &child, out);
        } else if !skip(&child) {
            out.insert(child);
        }
    }
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "web/dist/pack.bin".into());
    let mut pack = Pack::find().expect("resource pack not found");
    let log = pack.record_access();

    // Exercise start-up loading and every UI screen.
    let assets = Assets::load(pack.clone());
    let mut ui = Ui::new(&assets);
    let inv = Inventory::default();
    let hud = HudInfo {
        health: 20.0,
        max_health: 20.0,
        food: 20.0,
        ..Default::default()
    };
    for screen in [
        Screen::None,
        Screen::Inventory,
        Screen::CraftingTable,
        Screen::CreativeInventory,
        Screen::Furnace,
        Screen::Pause,
        Screen::Death,
    ] {
        ui.open(screen);
        let mut dl = UiDrawList::default();
        ui.build(&mut dl, 1280.0, 720.0, &inv, &hud);
        for q in &dl.quads {
            let key = q.texture.as_str();
            if !key.starts_with('@') {
                let _ = pack.load_texture(key);
            }
        }
    }

    let mut files: BTreeSet<String> = log.lock().unwrap().clone();
    // Recorded files are needed as-is (some real textures end in `_normal`,
    // e.g. `sandstone_normal`); the PBR filter only applies to whole folders.
    for dir in WHOLE_DIRS {
        walk(&pack.root, dir, &mut files);
    }
    let bytes = pack.write_bundle(files.iter().map(String::as_str));
    if let Some(dir) = Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("create output dir");
    }
    std::fs::write(&out, &bytes).expect("write bundle");
    println!(
        "{} files, {:.1} MB -> {out}",
        files.len(),
        bytes.len() as f64 / 1048576.0
    );
}
