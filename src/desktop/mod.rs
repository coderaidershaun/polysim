//! Native desktop workstation (eframe/wgpu over Metal), and the toolkit-free half it shares with the
//! headless bridge. Only the gated files name egui/eframe types: the painters and the window, which
//! exist under feature `ui`. Everything crossing the engine boundary is framework-free `msg::ui`
//! over UDP (the engine is another process).
//!
//! Flat files prefixed by panel. Suffix is the layer, and that is the seam worth knowing:
//! `<panel>_model` folds the feed into state, `<panel>_view` projects state to geometry — both
//! zero-egui, both testable headless. `text` holds every word and colour role the panels show, so a
//! painter and the bridge say the same thing. Everything else paints.
//!
//! `pub` here means the `dom-fixture` example or the fitness suite names it; the shell stays private.

#[cfg(feature = "ui")]
mod app;
pub(crate) mod attachment;
#[cfg(feature = "ui")]
pub mod chart;
pub mod chart_model;
#[cfg(feature = "ui")]
pub mod chart_stack;
pub mod chart_view;
#[cfg(feature = "ui")]
mod controls;
#[cfg(feature = "ui")]
mod crosshair;
#[cfg(feature = "ui")]
pub mod dom;
#[cfg(feature = "ui")]
mod dom_overlay;
pub mod dom_view;
pub mod exec_model;
pub mod format;
mod history;
#[cfg(feature = "ui")]
mod latency;
#[cfg(feature = "ui")]
mod layout;
#[cfg(feature = "ui")]
mod lifecycle;
#[cfg(feature = "ui")]
mod link_bar;
pub(crate) mod link_client;
pub mod link_model;
pub mod model;
#[cfg(feature = "ui")]
pub mod monitor;
#[cfg(feature = "ui")]
mod monitor_account;
#[cfg(feature = "ui")]
mod monitor_channels;
pub mod monitor_model;
#[cfg(feature = "ui")]
mod monitor_rows;
#[cfg(feature = "ui")]
mod monitor_summary;
pub mod monitor_view;
#[cfg(feature = "ui")]
mod position_chart;
pub mod position_chart_model;
pub mod position_chart_view;
pub(crate) mod text;
#[cfg(feature = "ui")]
mod theme;

#[cfg(feature = "ui")]
use std::path::PathBuf;

#[cfg(feature = "ui")]
use eframe::egui;

#[cfg(feature = "ui")]
use crate::log::LogConfig;
#[cfg(feature = "ui")]
use crate::msg::ui::ui_channel;

#[cfg(feature = "ui")]
use app::DesktopApp;
#[cfg(feature = "ui")]
use link_client::LinkClient;

pub use link_client::{ArgumentError, AttachArguments, LinkClientConfig, LinkClientError};

/// One workstation, one log sink. Named for binary not trading engine (picker moves between engines mid-session).
#[cfg(feature = "ui")]
const LOG_FILE_STEM: &str = "polysim-ui";

#[cfg(feature = "ui")]
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
#[cfg(feature = "ui")]
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
#[cfg(feature = "ui")]
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
