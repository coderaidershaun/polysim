//! Composition only: the agent's MCP server as its own process, speaking to one host over stdio.
//! Its stdout carries the protocol, so nothing here prints to it.
//!
//! ```text
//! cargo run --release --features mcp --bin polysim-mcp -- [--root <repo-root>]
//! ```
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use polysim::mcp::{Fleet, FleetTiming, StopOutcome, serve_stdio};

const USAGE: &str = "usage: polysim-mcp [--root <path>]

  --root  the repo checkout whose strategies/ folder lists the engines. Defaults to the current
          directory. Engine binaries are looked up beside this executable";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("polysim-mcp: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let Some(root) = parse_root(std::env::args().skip(1))? else {
        eprintln!("{USAGE}");
        return Ok(());
    };
    let root = std::fs::canonicalize(&root)
        .with_context(|| format!("cannot resolve the root {}", root.display()))?;
    let bin_dir = std::env::current_exe()
        .context("cannot locate this executable")?
        .parent()
        .context("this executable has no parent directory")?
        .to_path_buf();
    let fleet = Fleet::new(root, bin_dir, FleetTiming::default());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot build the tokio runtime")?;
    let report = runtime.block_on(serve_stdio(fleet));
    // Reading stdin blocks a thread that cannot be cancelled, so a plain drop of the runtime would
    // wait until the host closes stdin.
    runtime.shutdown_background();
    for outcome in report.stopped {
        eprintln!("polysim-mcp: {}", describe(outcome));
    }
    report.session.context("mcp session failed")
}

fn parse_root(mut arguments: impl Iterator<Item = String>) -> Result<Option<PathBuf>> {
    let mut root = PathBuf::from(".");
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(None),
            "--root" => {
                root = arguments
                    .next()
                    .map(PathBuf::from)
                    .context("--root needs a path")?;
            }
            other => match other.strip_prefix("--root=") {
                Some(path) => root = PathBuf::from(path),
                None => bail!("unknown argument {other:?}\n\n{USAGE}"),
            },
        }
    }
    Ok(Some(root))
}

fn describe(outcome: StopOutcome) -> String {
    match outcome.result {
        Ok(stopped) => format!(
            "stopped {} (exit code {}, forced {})",
            outcome.identity,
            stopped
                .exit_code
                .map_or_else(|| "none".to_owned(), |code| code.to_string()),
            stopped.forced
        ),
        Err(error) => format!(
            "could not stop {}: {:#}",
            outcome.identity,
            anyhow::Error::from(error)
        ),
    }
}
