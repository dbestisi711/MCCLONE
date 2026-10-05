//! Desktop entry point. See `app.rs` for the window/event loop and `game.rs`
//! for the game itself.
//!
//! Run:          cargo run --release
//! Screenshot:   cargo run --release -- --screenshot out.png [--size 1280x720] [--seed N]
//!               [--time 6000] [--yaw DEG] [--pitch DEG] [--pos X,Y,Z] [--ui inventory]
//!               [--scene showcase|cave|underwater|entities] [--bench FRAMES]
//! Keys:         + / - change the render distance.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    mc_game::run();
}
