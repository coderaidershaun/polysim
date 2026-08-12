//! Parquet research output, off the hot path. Files seal (not just flush) so a reader never waits
//! for shutdown, and a truncated file is an error rather than a silently short table.

mod drain;
mod schema;
mod table;

use std::any::Any;
use std::path::PathBuf;

use crate::config::{ExecutionMode, RecordedTables, StrategyId, TradingEngineId};

pub use drain::{PersistHandle, PersistWriter};

#[derive(Debug, Clone)]
pub struct PersistConfig {
    pub dir: PathBuf,
    pub tables: RecordedTables,
}

#[derive(Debug, Clone)]
pub struct RunMeta {
    pub strategy_id: StrategyId,
    pub te_id: TradingEngineId,
    /// Records the execution mode and isolates simulated artifacts.
    pub execution_mode: Option<ExecutionMode>,
    pub fixed_scale: i64,
    pub engine_version: Box<str>,
    pub feature_names: Vec<Box<str>>,
    pub instrument_symbols: Vec<Box<str>>,
    // Wrong reconciliation if not decoded.
    pub asset_symbols: Vec<Box<str>>,
}

#[derive(thiserror::Error, Debug)]
pub enum PersistError {
    #[error("persistence io failed at {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parquet write failed for {table} table")]
    Parquet {
        table: &'static str,
        #[source]
        source: parquet::errors::ParquetError,
    },
    #[error("record batch build failed for {table} table")]
    Batch {
        table: &'static str,
        #[source]
        source: arrow_schema::ArrowError,
    },
    #[error("persistence writer thread panicked: {payload}")]
    WriterPanicked { payload: Box<str> },
    #[error("persistence drain interrupted: writer thread outcome not observed")]
    DrainInterrupted,
}

impl PersistError {
    /// Panic = breach the panic hook already turned fatal; this only reports it, never re-raises.
    fn writer_panicked(payload: &(dyn Any + Send)) -> Self {
        PersistError::WriterPanicked {
            payload: panic_payload_string(payload),
        }
    }
}

fn panic_payload_string(payload: &(dyn Any + Send)) -> Box<str> {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str().into()
    } else {
        "non-string panic payload".into()
    }
}
