//! Startup steps before threads spawn: build registry, validate core, build tokio runtime, run preflight.
//! Everything that can refuse a run happens here, so a failure returns with nothing to tear down.

use tokio::runtime::Runtime;

use crate::adapters::binance::exec::{ExecutionPreflight, preflight_execution};
use crate::adapters::polymarket::exec::handle::{PolymarketPreflight, preflight_polymarket};
use crate::adapters::rest_quiet::SharedRestQuiet;
use crate::config::{Config, ExecutionMode, RunIdentity, SourceSpec};
use crate::exposure::{self, ExposureState};
use crate::info;
use crate::registry::Registry;

use super::EngineError;
use super::preflight::{check_execution_order_capacity, preflight_poly, preflight_scales};

/// Startup results grouped as struct to avoid tuple-member mistakes.
pub(super) struct Prepared {
    pub(super) registry: Registry,
    pub(super) runtime: Runtime,
    /// Position from previous run; empty on cold start.
    pub(super) exposure: ExposureState,
    /// None unless execution.mode needs credentials.
    pub(super) execution: Option<VenuePreflight>,
    /// Binance charges market-data reads and signed order traffic against one per-IP allowance, so
    /// the window is built here — where the signed client is — and handed on to the adapters.
    pub(super) binance_rest_quiet: SharedRestQuiet,
}

/// Which venue's startup gate was passed, carrying what that venue's edge needs. The variant IS the
/// venue: bring-up dispatches on it rather than re-deriving the venue from the source. Both boxed —
/// a probed REST client and a wallet key are different sizes, and this is moved, never hot.
pub(super) enum VenuePreflight {
    Binance(Box<ExecutionPreflight>),
    Polymarket(Box<PolymarketPreflight>),
}

/// `mode` is the caller's, never re-derived here: it picks which position file this run owns, and a
/// second derivation could load one ledger while the writer keeps another.
pub(super) fn prepare<P>(
    config: &Config<P>,
    identity: &RunIdentity,
    mode: Option<ExecutionMode>,
) -> Result<Prepared, EngineError> {
    let mut registry = Registry::build(config)?;
    info!(
        "registry: {} instruments, {} input queues",
        registry.instruments().len(),
        registry.input_queue_count()
    );
    if let Some(core_id) = config.engine.hot_core_id {
        validate_core_id(core_id)?;
    }
    let runtime = build_runtime(config.engine.tokio_workers)?;
    // Validate before threads start. Deployment from registry not constant -> preflight production grid.
    if let Some(env) = registry.binance_env() {
        runtime.block_on(preflight_scales(&mut registry, env))?;
        info!(
            "scale preflight ok on binance {} — every instrument fits the i64 1e-8 range",
            env.as_str()
        );
    }
    runtime.block_on(preflight_poly(&mut registry))?;
    if let Some(execution) = &config.execution {
        check_execution_order_capacity(&registry, execution.max_orders_per_side)?;
    }
    // Last gate before threads: unreadable position must stop here, not after engine begins.
    let exposure = exposure::load(&config.exposure.dir, identity, &registry, mode)?;
    report_restored(&exposure, &registry);
    let binance_rest_quiet = SharedRestQuiet::new();
    // Last: one signed call that proves the key can trade, before strategy quotes with no permission.
    let execution = match live_execution_source(config) {
        Some(SourceSpec::Binance { .. }) => {
            // Deployment read from the REGISTRY, not config -> testnet data can never front prod orders.
            let env = registry
                .binance_env()
                .expect("a binance source stamps its deployment on the registry");
            let probed = runtime
                .block_on(preflight_execution(env, binance_rest_quiet.clone()))
                .map_err(EngineError::BinanceExecutionPreflight)?;
            Some(VenuePreflight::Binance(Box::new(probed)))
        }
        Some(SourceSpec::Polymarket { .. }) => {
            let probed = runtime
                .block_on(preflight_polymarket())
                .map_err(EngineError::PolymarketExecutionPreflight)?;
            Some(VenuePreflight::Polymarket(Box::new(probed)))
        }
        None => None,
    };
    Ok(Prepared {
        registry,
        runtime,
        exposure,
        execution,
        binance_rest_quiet,
    })
}

/// The source whose venue this run will place orders on, or None when nothing is armed for live.
fn live_execution_source<P>(config: &Config<P>) -> Option<&SourceSpec> {
    config
        .execution
        .as_ref()
        .filter(|execution| execution.mode.needs_credentials())
        .map(|_| &config.source)
}

/// Report what was carried in per asset. Cold start says so explicitly: silence = failed load.
fn report_restored(exposure: &ExposureState, registry: &Registry) {
    if exposure.is_empty() {
        info!("exposure: cold start — no position carried in from a previous run");
        return;
    }
    for asset in exposure.assets() {
        info!(
            "exposure restored: {} {}",
            registry.assets().name(asset.asset).unwrap_or("<unknown>"),
            crate::ids::Qty(asset.amount).to_f64()
        );
    }
}

/// Reject unavailable core so mis-set config fails at startup, not silently. Best-effort platforms skip.
fn validate_core_id(core_id: usize) -> Result<(), EngineError> {
    match core_affinity::get_core_ids() {
        Some(ids) if !ids.iter().any(|core| core.id == core_id) => Err(EngineError::CoreId {
            core_id,
            available: ids.len(),
        }),
        _ => Ok(()),
    }
}

fn build_runtime(workers: usize) -> Result<Runtime, EngineError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers.max(1))
        .enable_all()
        .thread_name("polysim-tokio")
        .build()
        .map_err(EngineError::Runtime)
}
