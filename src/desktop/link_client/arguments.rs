//! The command-line flags both workstation programs share: which engines to attach to and how to
//! reach them. Each program keeps its own usage text and decides what to do with the optional tick.

use std::net::{AddrParseError, SocketAddr};
use std::num::ParseIntError;

use crate::config::{ConfigError, StrategyId};

use super::LinkClientConfig;

/// Deliberately not `Debug`: the link config carries the token.
pub struct AttachArguments {
    pub link: LinkClientConfig,
    /// Left as typed because only some programs have a tick: each decides whether it is allowed
    /// and which range is valid.
    pub tick_ms: Option<u64>,
}

#[derive(thiserror::Error, Debug)]
pub enum ArgumentError {
    #[error("{flag} needs a value")]
    MissingValue { flag: &'static str },
    #[error("--link {raw:?} is not a host:port address")]
    Link {
        raw: String,
        #[source]
        source: AddrParseError,
    },
    #[error("--strategy {raw:?} is not a valid identifier")]
    Strategy {
        raw: String,
        #[source]
        source: ConfigError,
    },
    #[error("--tick-ms {raw:?} is not a whole number")]
    TickMs {
        raw: String,
        #[source]
        source: ParseIntError,
    },
    #[error("unknown argument {argument:?}")]
    Unknown { argument: String },
    #[error("--strategy is required")]
    MissingStrategy,
    #[error("at least one --link address is required")]
    NoLink,
}

impl AttachArguments {
    /// `None` when help was asked for.
    ///
    /// # Errors
    /// [`ArgumentError::MissingValue`] when a flag ends the arguments, [`ArgumentError::Link`],
    /// [`ArgumentError::Strategy`] and [`ArgumentError::TickMs`] for a value that does not parse,
    /// [`ArgumentError::Unknown`] for a flag nobody reads, and [`ArgumentError::MissingStrategy`]
    /// or [`ArgumentError::NoLink`] when the engine is not named.
    pub fn parse(
        mut arguments: impl Iterator<Item = String>,
    ) -> Result<Option<Self>, ArgumentError> {
        let mut peers: Vec<SocketAddr> = Vec::new();
        let mut strategy_id = None;
        let mut token = None;
        let mut tick_ms = None;
        while let Some(flag) = arguments.next() {
            match flag.as_str() {
                "--help" | "-h" => return Ok(None),
                "--link" => {
                    let raw = value(&mut arguments, "--link")?;
                    peers.push(
                        raw.parse()
                            .map_err(|source| ArgumentError::Link { raw, source })?,
                    );
                }
                "--strategy" => {
                    let raw = value(&mut arguments, "--strategy")?;
                    strategy_id = Some(
                        StrategyId::new(&raw)
                            .map_err(|source| ArgumentError::Strategy { raw, source })?,
                    );
                }
                "--token" => token = Some(value(&mut arguments, "--token")?.into_boxed_str()),
                "--tick-ms" => {
                    let raw = value(&mut arguments, "--tick-ms")?;
                    tick_ms = Some(
                        raw.parse()
                            .map_err(|source| ArgumentError::TickMs { raw, source })?,
                    );
                }
                _ => {
                    return Err(ArgumentError::Unknown {
                        argument: without_value(flag),
                    });
                }
            }
        }
        let strategy_id = strategy_id.ok_or(ArgumentError::MissingStrategy)?;
        if peers.is_empty() {
            return Err(ArgumentError::NoLink);
        }
        Ok(Some(Self {
            link: LinkClientConfig {
                peers,
                strategy_id,
                token,
            },
            tick_ms,
        }))
    }
}

fn value(
    arguments: &mut impl Iterator<Item = String>,
    flag: &'static str,
) -> Result<String, ArgumentError> {
    arguments.next().ok_or(ArgumentError::MissingValue { flag })
}

/// An unknown `--name=value` is echoed without its value, since the value may be the link token,
/// which is never printed.
fn without_value(argument: String) -> String {
    match argument.split_once('=') {
        Some((name, _)) => format!("{name}=..."),
        None => argument,
    }
}
