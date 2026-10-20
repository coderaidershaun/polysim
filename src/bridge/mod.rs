//! The workstation's headless twin: it attaches to one running engine the way the window does,
//! folds the feed into the same model, and writes what the window would show as JSON lines. Jet
//! paints those lines in a web page, so no second model exists to drift from the one the window uses.

mod command;
mod encode;
mod run;
mod session;

use std::path::PathBuf;
use std::time::Duration;

use crate::desktop::{LinkClientConfig, LinkClientError};
use crate::log::LogConfig;

pub use command::{Command, CommandError, PageFit, ViewCommand};
pub use encode::{Encoder, TickInput, hello_line};

/// One bridge, one log sink. Named for the binary, since the page can move between engines.
const LOG_FILE_STEM: &str = "polysim-bridge";

/// Deliberately not `Debug`: the link config carries the token.
pub struct BridgeConfig {
    pub link: LinkClientConfig,
    pub tick: Duration,
}

#[derive(thiserror::Error, Debug)]
pub enum BridgeError {
    #[error("failed to attach to a trading engine")]
    Link(#[from] LinkClientError),
    #[error("failed to write a protocol line to stdout")]
    Stdout(#[source] std::io::Error),
}

/// Run the bridge on the calling thread until stdin closes, attached to one engine over the link.
/// Stopping it stops nothing on the engine, which is another process.
///
/// # Errors
/// [`BridgeError::Link`] when no link socket can be opened, and [`BridgeError::Stdout`] when a
/// protocol line cannot be written, which ends the run since nothing is reading the page's side.
pub fn run_bridge(config: BridgeConfig) -> Result<(), BridgeError> {
    let logging = crate::log::init(&LogConfig {
        dir: PathBuf::from("logs"),
        file_stem: LOG_FILE_STEM.into(),
    });
    crate::log::register_thread("bridge");
    let outcome = run::serve(config);
    logging.drain();
    outcome
}
