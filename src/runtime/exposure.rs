//! Exposure writer setup: the position ledger's disk writer, seeded with what the last run left.

use crate::config::{ExecutionMode, ExposureConfig, RunIdentity};
use crate::exposure::{ExposureHandle, ExposureState, ExposureWriter, ExposureWriterConfig};
use crate::registry::Registry;
use crate::sink::ExposureSink;

/// Spawn before hot thread, seeded with restored state so writer's disk view is correct from start.
/// `mode` picks the artifact stem, so it has to be the same value the load was given — the caller
/// derives it once and hands it to both, or this run writes a ledger it did not read.
pub(super) fn exposure_bring_up(
    config: &ExposureConfig,
    identity: &RunIdentity,
    registry: &Registry,
    restored: &ExposureState,
    mode: Option<ExecutionMode>,
) -> (ExposureHandle, ExposureSink) {
    ExposureWriter::spawn(
        ExposureWriterConfig::new(config.dir.clone(), identity.clone(), mode, registry),
        restored,
    )
}
