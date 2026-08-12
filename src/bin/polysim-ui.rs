//! Composition only: desktop workstation as its own process, attached over the UDP link to
//! ONE running trading engine. Closing it stops nothing — a trading engine drains on its own signal.
//!
//! ```text
//! cargo run --release --features ui --bin polysim-ui -- \
//!     --strategy strat-micro-recorder --link 127.0.0.1:9310
//! ```
use std::net::SocketAddr;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use polysim::config::StrategyId;
use polysim::desktop::{LinkClientConfig, run_desktop};

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
    let Some(config) = parse_arguments(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(());
    };
    run_desktop(config).context("desktop workstation exited with an error")?;
    Ok(())
}

fn parse_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Option<LinkClientConfig>> {
    let mut peers: Vec<SocketAddr> = Vec::new();
    let mut strategy_id = None;
    let mut token = None;
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--link" => {
                let raw = value(&mut arguments, "--link")?;
                peers.push(
                    raw.parse()
                        .with_context(|| format!("--link {raw:?} is not a host:port address"))?,
                );
            }
            "--strategy" => {
                let raw = value(&mut arguments, "--strategy")?;
                strategy_id =
                    Some(StrategyId::new(&raw).with_context(|| {
                        format!("--strategy {raw:?} is not a valid identifier")
                    })?);
            }
            "--token" => token = Some(value(&mut arguments, "--token")?.into_boxed_str()),
            other => bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }
    let Some(strategy_id) = strategy_id else {
        bail!("--strategy is required\n\n{USAGE}");
    };
    if peers.is_empty() {
        bail!("at least one --link address is required\n\n{USAGE}");
    }
    Ok(Some(LinkClientConfig {
        peers,
        strategy_id,
        token,
    }))
}

fn value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    arguments
        .next()
        .with_context(|| format!("{flag} needs a value\n\n{USAGE}"))
}
