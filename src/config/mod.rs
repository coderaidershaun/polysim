//! Turns YAML into a validated config. An unknown-field typo becomes a named startup
//! error rather than silent data loss. Defaults live in code, not in the file.
//! One submodule per block a config file can carry, plus `error` and `identity`; the root schema
//! that names those blocks lives here, so a caller writes `config::LinkConfig`, never the file.

mod error;
mod execution;
mod exposure;
mod identity;
mod link;
mod simulated;
mod sources;
mod tables;
mod tracker;
mod venue;

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use error::ConfigError;
pub use execution::{ExecutionConfig, ExecutionMode};
pub use exposure::ExposureConfig;
pub use identity::{RunIdentity, StrategyId, TradingEngineId};
pub use link::{ControllerLoss, LinkConfig, PeerSubscription};
pub use simulated::SimConfig;
pub use sources::{
    BinanceEnv, BinanceMarket, KlineInterval, PolySeries, PolySubscriptions, SourceSpec,
    Subscriptions,
};
pub use tables::{RecordedTables, TableKind};
pub use tracker::{
    CandlesSpec, EwmaVolSpec, ImbalanceSpec, IntensitySpec, SpinField, SpinSampledSpec,
    TrackerSpec, VolumeBarsSpec, VolumeThreshold, WindowsSpec,
};
pub use venue::VenueMarket;

pub(crate) use execution::{DEFAULT_QUOTE_STOP_MARGIN_MS, validated_mantissa};

const SPIN_INTERVAL_US_RANGE: std::ops::RangeInclusive<u64> = 1_000..=60_000_000;

const WARMUP_SECS_RANGE: std::ops::RangeInclusive<u64> = 0..=3_600;
const MAX_OPERATIONAL_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;

/// `P` is the strategy's own `params:` type, deserialized in the same single pass so
/// `deny_unknown_fields` holds inside it too; it defaults to [`NoParams`] for a knob-free strategy.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    deny_unknown_fields,
    bound(deserialize = "P: serde::Deserialize<'de> + Default")
)]
pub struct Config<P = NoParams> {
    pub engine: EngineConfig,
    pub queues: QueuesConfig,
    pub source: SourceSpec,
    pub strategy: StrategySpec<P>,
    #[serde(default)]
    pub persistence: Option<PersistenceConfig>,
    #[serde(default)]
    pub link: Option<LinkConfig>,
    #[serde(default)]
    pub execution: Option<ExecutionConfig>,
    #[serde(default)]
    pub exposure: ExposureConfig,
    pub logging: LoggingConfig,
}

impl<P: serde::de::DeserializeOwned + Default> Config<P> {
    /// # Errors
    /// [`ConfigError::Read`] or errors from [`from_yaml`].
    ///
    /// [`from_yaml`]: Config::from_yaml
    pub fn load(path: &Path) -> Result<Config<P>, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Config::from_yaml(&contents)
    }

    /// # Errors
    /// [`ConfigError::Parse`] for malformed YAML or unknown/misspelt fields.
    pub fn from_yaml(contents: &str) -> Result<Config<P>, ConfigError> {
        let config: Config<P> =
            serde_saphyr::from_str(contents).map_err(|error| ConfigError::Parse {
                detail: error.to_string().into_boxed_str(),
            })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.persistence.is_none() && !self.strategy.tables.is_empty() {
            let named: Vec<&str> = self
                .strategy
                .tables
                .iter()
                .copied()
                .map(TableKind::as_str)
                .collect();
            return Err(ConfigError::TablesWithoutPersistence {
                tables: named.join(", ").into_boxed_str(),
            });
        }
        let spin = self.engine.spin_interval_us;
        if !SPIN_INTERVAL_US_RANGE.contains(&spin) {
            return Err(ConfigError::EngineFieldRange {
                field: "spin_interval_us",
                value: spin,
                expected: "1000..=60000000 (1ms to 60s)",
            });
        }
        let warmup = self.engine.warmup_secs;
        if !WARMUP_SECS_RANGE.contains(&warmup) {
            return Err(ConfigError::EngineFieldRange {
                field: "warmup_secs",
                value: warmup,
                expected: "0..=3600 (0 disables warmup; beyond 1h a run is mostly suppressed)",
            });
        }
        if self.engine.drain_deadline_ms == 0 {
            return Err(ConfigError::EngineFieldZero {
                field: "drain_deadline_ms",
            });
        }
        if self.engine.drain_deadline_ms > MAX_OPERATIONAL_DURATION_MS {
            return Err(ConfigError::EngineFieldRange {
                field: "drain_deadline_ms",
                value: self.engine.drain_deadline_ms,
                expected: "1..=86400000 (24h operational ceiling)",
            });
        }
        if self.queues.input_capacity == 0 {
            return Err(ConfigError::QueueCapacityZero {
                field: "input_capacity",
            });
        }
        if self.queues.persistence_capacity == 0 {
            return Err(ConfigError::QueueCapacityZero {
                field: "persistence_capacity",
            });
        }
        if let Some(link) = &self.link {
            link.validate()?;
        }
        if let Some(execution) = &self.execution {
            execution.validate(self.engine.spin_interval_us)?;
            self.check_execution_source(execution.mode)?;
        }
        Ok(())
    }

    fn check_execution_source(&self, mode: ExecutionMode) -> Result<(), ConfigError> {
        if !mode.is_enabled() {
            return Ok(());
        }
        if !self.source.supports_execution(mode) {
            let supported: Vec<&str> = self
                .source
                .execution_modes()
                .iter()
                .copied()
                .map(ExecutionMode::as_str)
                .collect();
            return Err(ConfigError::ExecutionModeUnsupported {
                venue: self.source.venue(),
                mode: mode.as_str(),
                supported: supported.join(", ").into_boxed_str(),
            });
        }
        let SourceSpec::Binance {
            market,
            subscriptions,
            ..
        } = &self.source
        else {
            return Ok(());
        };
        if !mode.is_simulated() {
            return Ok(());
        }
        if *market != BinanceMarket::Spot {
            return Err(ConfigError::SimulatedExecutionMarket {
                market: market.as_str(),
            });
        }
        let missing: Vec<&str> = [
            ("trades", subscriptions.trades),
            ("book_updates", subscriptions.book_updates),
            ("book_snapshots", subscriptions.book_snapshots),
        ]
        .into_iter()
        .filter(|(_, enabled)| !enabled)
        .map(|(name, _)| name)
        .collect();
        if !missing.is_empty() {
            return Err(ConfigError::SimulatedExecutionSubscriptions {
                missing: missing.join(", ").into_boxed_str(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// This field is required on Linux; it is ignored on macOS.
    #[serde(default)]
    pub hot_core_id: Option<usize>,
    #[serde(default = "default_tokio_workers")]
    pub tokio_workers: usize,
    /// The engine's sampling cadence. Mandatory, with no default.
    pub spin_interval_us: u64,
    #[serde(default = "default_drain_deadline_ms")]
    pub drain_deadline_ms: u64,
    #[serde(default = "default_warmup_secs")]
    pub warmup_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuesConfig {
    #[serde(default = "default_input_capacity")]
    pub input_capacity: usize,
    #[serde(default = "default_persistence_capacity")]
    pub persistence_capacity: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    deny_unknown_fields,
    bound(deserialize = "P: serde::Deserialize<'de> + Default")
)]
pub struct StrategySpec<P = NoParams> {
    #[serde(default)]
    pub instruments: Instruments,
    #[serde(default)]
    pub tables: Vec<TableKind>,
    #[serde(default)]
    pub params: P,
}

/// The default type for a strategy with no knobs, which allows an empty `params: {}` block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceConfig {
    #[serde(default = "default_data_dir")]
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_logs_dir")]
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Instruments {
    #[default]
    All,
    Explicit(Vec<Box<str>>),
}

impl<'de> Deserialize<'de> for Instruments {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct InstrumentsVisitor;

        impl<'de> serde::de::Visitor<'de> for InstrumentsVisitor {
            type Value = Instruments;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("\"all\" or a list of instrument symbols")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Instruments, E> {
                if value == "all" {
                    Ok(Instruments::All)
                } else {
                    Err(E::custom(format!(
                        "expected \"all\" or a list, got {value:?}"
                    )))
                }
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Instruments, A::Error> {
                let mut symbols = Vec::new();
                while let Some(symbol) = seq.next_element::<String>()? {
                    symbols.push(symbol.into_boxed_str());
                }
                Ok(Instruments::Explicit(symbols))
            }
        }

        deserializer.deserialize_any(InstrumentsVisitor)
    }
}

fn default_tokio_workers() -> usize {
    2
}

fn default_drain_deadline_ms() -> u64 {
    5_000
}

fn default_warmup_secs() -> u64 {
    10
}

fn default_input_capacity() -> usize {
    65_536
}

fn default_persistence_capacity() -> usize {
    65_536
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_logs_dir() -> PathBuf {
    PathBuf::from("./logs")
}
