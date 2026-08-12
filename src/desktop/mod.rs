//! Native desktop workstation (eframe/wgpu over Metal). ONLY module naming egui/eframe types.
//! Everything crossing engine boundary is framework-free `msg::ui` over UDP (engine is another process).
//!
//! Flat files prefixed by panel. Suffix is the layer, and that is the seam worth knowing:
//! `<panel>_model` folds the feed into state, `<panel>_view` projects state to geometry — both
//! zero-egui, both testable headless. Everything else paints.
//!
//! `pub` here means the `dom-fixture` example or the fitness suite names it; the shell stays private.

mod app;
pub mod chart;
pub mod chart_model;
pub mod chart_stack;
pub mod chart_view;
mod controls;
mod crosshair;
pub mod dom;
mod dom_overlay;
pub mod dom_view;
pub mod exec_model;
pub mod format;
mod history;
mod latency;
mod layout;
mod lifecycle;
mod link_bar;
mod link_client;
pub mod link_model;
pub mod model;
pub mod monitor;
mod monitor_account;
mod monitor_channels;
pub mod monitor_model;
mod monitor_rows;
mod monitor_summary;
pub mod monitor_view;
mod position_chart;
pub mod position_chart_model;
pub mod position_chart_view;
mod theme;

use std::path::PathBuf;

use eframe::egui;

use crate::log::LogConfig;
use crate::msg::ui::ui_channel;

use app::DesktopApp;
use link_client::LinkClient;

pub use link_client::{LinkClientConfig, LinkClientError};

/// One workstation, one log sink. Named for binary not trading engine (picker moves between engines mid-session).
const LOG_FILE_STEM: &str = "polysim-ui";

#[derive(thiserror::Error, Debug)]
pub enum DesktopError {
    #[error("failed to attach to a trading engine")]
    Link(#[from] LinkClientError),
    #[error("desktop window failed: {0}")]
    Window(Box<str>),
}

/// Run workstation on main thread, attached to engine via link. Close doesn't stop engine (separate process).
/// # Errors
/// Link/socket open or eframe startup failure.
pub fn run_desktop(config: LinkClientConfig) -> Result<(), DesktopError> {
    let logging = crate::log::init(&LogConfig {
        dir: PathBuf::from("logs"),
        file_stem: LOG_FILE_STEM.into(),
        ..LogConfig::default()
    });
    crate::log::register_thread("ui");
    let outcome = attach_and_run(config);
    logging.drain();
    outcome
}

/// Ensures log thread shuts down even on attach failure.
fn attach_and_run(config: LinkClientConfig) -> Result<(), DesktopError> {
    let (wiring, channels) = ui_channel();
    let (client, feed) = LinkClient::start(config, wiring)?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("dev.polysim.ui")
            .with_title("Polysim")
            .with_inner_size(egui::vec2(1_600.0, 1_000.0))
            // Floor set by the summary band: below ~1190 its mid cell is narrower than a BTC mid,
            // so the number would cross a separator. Refuse the size rather than overlap.
            .with_min_inner_size(egui::vec2(1_200.0, 700.0))
            .with_resizable(true)
            .with_decorations(true),
        renderer: eframe::Renderer::Wgpu,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        centered: true,
        ..Default::default()
    };

    let outcome = eframe::run_native(
        "Polysim",
        options,
        Box::new(move |creation| Ok(Box::new(DesktopApp::new(creation, channels, feed)))),
    )
    .map_err(|error| DesktopError::Window(error.to_string().into_boxed_str()));
    client.shutdown();
    outcome
}
