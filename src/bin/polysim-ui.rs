//! Composition only: desktop workstation as its own process, attached over the UDP link to
//! ONE running trading engine. Closing it stops nothing — a trading engine drains on its own signal.
//!
//! ```text
//! cargo run --release --features ui --bin polysim-ui -- \
//!     --strategy strat-micro-recorder --link 127.0.0.1:9310
//! ```
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use polysim::desktop::{AttachArguments, run_desktop};

const USAGE: &str = "usage: polysim-ui --strategy <strategy-id> --link <addr> [--link <addr>]... \
                     [--token <token>]

  --strategy  the trading engine's strategy id. Not carried on the wire: a receiver must already
              know it to compute the strategy hash its own frame guard checks
  --link      a trading engine's link bind address. Repeatable, and a PICKER — one engine is
              rendered at a time, chosen from the link bar
  --token     the engine's link token, if its config sets one";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("polysim-ui: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let arguments = match AttachArguments::parse(std::env::args().skip(1)) {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(error) => bail!("{:#}\n\n{USAGE}", anyhow::Error::new(error)),
    };
    if arguments.tick_ms.is_some() {
        bail!("--tick-ms applies to polysim-bridge only\n\n{USAGE}");
    }
    run_desktop(arguments.link).context("desktop workstation exited with an error")?;
    Ok(())
}
