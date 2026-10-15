//! Composition only: the workstation's view of ONE running trading engine, written as JSON lines on
//! stdout for a web page to paint, with the page's commands read from stdin. Stdout carries protocol
//! lines and nothing else, so usage text and errors go to stderr. Closing stdin ends the bridge and
//! stops nothing on the engine.
//!
//! ```text
//! cargo run --release --bin polysim-bridge -- \
//!     --strategy strat-micro-recorder --link 127.0.0.1:9310
//! ```
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use polysim::bridge::{BridgeConfig, run_bridge};
use polysim::desktop::AttachArguments;

const USAGE: &str =
    "usage: polysim-bridge --strategy <strategy-id> --link <addr> [--link <addr>]... \
                     [--token <token>] [--tick-ms <ms>]

  --strategy  the trading engine's strategy id. Not carried on the wire: a receiver must already
              know it to compute the strategy hash its own frame guard checks
  --link      a trading engine's link bind address. Repeatable, and a PICKER - one engine is
              followed at a time, chosen by the page
  --token     the engine's link token, if its config sets one
  --tick-ms   how often a line is written when something changed, 1 to 1000 (default 16)";

const DEFAULT_TICK_MS: u64 = 16;
const TICK_MS_RANGE: std::ops::RangeInclusive<u64> = 1..=1_000;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("polysim-bridge: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let arguments = match AttachArguments::parse(std::env::args().skip(1)) {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            eprintln!("{USAGE}");
            return Ok(());
        }
        Err(error) => bail!("{:#}\n\n{USAGE}", anyhow::Error::new(error)),
    };
    let tick_ms = arguments.tick_ms.unwrap_or(DEFAULT_TICK_MS);
    if !TICK_MS_RANGE.contains(&tick_ms) {
        bail!(
            "--tick-ms {tick_ms} is not a whole number from {} to {}\n\n{USAGE}",
            TICK_MS_RANGE.start(),
            TICK_MS_RANGE.end()
        );
    }
    run_bridge(BridgeConfig {
        link: arguments.link,
        tick: Duration::from_millis(tick_ms),
    })
    .context("workstation bridge exited with an error")?;
    Ok(())
}
