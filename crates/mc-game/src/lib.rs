//! MCCLONE game crate: the [`game::Game`] state, the winit [`app`] shell
//! shared by the desktop binary and the web build, and headless tooling.
//!
//! Desktop:  `cargo run --release` (see `src/main.rs`).
//! Web:      `./web/build.sh` builds `web/dist` (wasm + pack bundle).

pub mod app;
pub mod game;
mod input_map;
#[cfg(not(target_arch = "wasm32"))]
mod scenes;
pub mod touch;

pub use app::run;
