//! Dom-fixture: every workstation panel on fixed scenes. In the browser it paints them; natively it
//! prints the risk series as text, which proves the scenes without a screen.
// The native build only prints the risk dump, so most scene fields and builders look unused there.
// The browser build reads all of them, and it is the one that warns on dead fixture code.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

#[cfg(target_arch = "wasm32")]
mod app;
mod chart_scenes;
mod fixture;
#[cfg(target_arch = "wasm32")]
mod hud;
mod monitor_feed;
mod monitor_scenes;
#[cfg(not(target_arch = "wasm32"))]
mod position_dump;
mod position_scenes;
mod scenes;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    position_dump::dump();
}

#[cfg(target_arch = "wasm32")]
fn main() {
    polysim::workstation::browser::start_in_canvas(
        "fixture",
        Box::new(|_creation| Ok(Box::new(app::DomFixture::new()))),
    );
}
