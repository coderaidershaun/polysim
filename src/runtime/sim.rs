//! Bring-up for the simulated execution edge: the runtime's own inputs — the config block, the
//! registry row, the tapped market lanes — translated into the spec the venue assembles itself
//! from.

use std::sync::Arc;
use std::time::Duration;

use crate::adapters::binance::exec::SymbolTable;
use crate::adapters::exchange_sim::{
    InstrumentLimits, SimActor, SimActorSetup, SimVenueSettings, SimVenueSpec,
};
use crate::adapters::exec::EngineIdentity;
use crate::config::ExecutionConfig;
use crate::hot::spawn::{QueueProducer, SimTapGate};
use crate::msg::exec::ExecLaneItem;
use crate::registry::InstrumentRow;
use crate::time::DurationUs;

use super::EngineError;
use super::exec::{EdgeSpawn, ExecBringUpSetup};
use super::queues::MarketTaps;

pub(super) struct SimBringUp<'a> {
    pub(super) setup: &'a ExecBringUpSetup<'a>,
    pub(super) execution: &'a ExecutionConfig,
    pub(super) identity: EngineIdentity,
    pub(super) commands: rtrb::Consumer<ExecLaneItem>,
    pub(super) producer: QueueProducer,
    pub(super) taps: Option<MarketTaps>,
    pub(super) lane_capacity: usize,
    pub(super) sweep_deadline: Duration,
}

pub(super) struct SimEdge {
    pub(super) spawn: EdgeSpawn,
    /// Held by the drain: the taps feeding this venue must outlive its forced sweep.
    pub(super) tap_gate: Arc<SimTapGate>,
}

/// # Errors
/// Returns an [`EngineError`] when simulator prerequisites are invalid.
pub(super) fn sim_bring_up(bring_up: SimBringUp<'_>) -> Result<SimEdge, EngineError> {
    let SimBringUp {
        setup,
        execution,
        identity,
        commands,
        producer,
        taps,
        lane_capacity,
        sweep_deadline,
    } = bring_up;
    let sim = execution
        .sim
        .as_ref()
        .ok_or(EngineError::ExecutionNotWired {
            mode: "sim",
            detail: "the sim: block is absent, and config validation should have refused that",
        })?;
    let Some(taps) = taps else {
        return Err(EngineError::ExecutionNotWired {
            mode: "sim",
            detail: "the market taps the venue matches against were never attached",
        });
    };
    let instruments = setup.registry.instruments();
    let [row] = instruments else {
        return Err(EngineError::SimulatedVenueOneInstrument {
            found: instruments.len(),
        });
    };

    let verdict_retention_micros = execution
        .order_reap_secs
        .checked_mul(1_000_000)
        .and_then(|reap| {
            execution
                .inflight_timeout_ms
                .checked_mul(1_000)
                .and_then(|timeout| reap.checked_add(timeout))
        })
        .and_then(|value| i64::try_from(value).ok())
        .expect("sim verdict retention was config-validated to fit i64 microseconds");

    let tap_gate = taps.gate;
    let actor = SimActorSetup::assemble(SimVenueSpec {
        sim,
        instrument: row,
        limits: venue_limits(row)?,
        assets: setup.registry.assets().clone(),
        symbols: SymbolTable::from_registry(setup.registry),
        commands,
        producer,
        trades: taps.trades,
        depth: taps.depth,
        clock: setup.clock.clone(),
        identity,
        run_state: setup.desired_run_state.clone(),
        fatal: setup.fatal.clone(),
        settings: SimVenueSettings {
            max_orders_per_side: execution.max_orders_per_side as usize,
            verdict_retention: DurationUs::from_micros(verdict_retention_micros),
            inflight_timeout: DurationUs::from_millis(execution.inflight_timeout_ms as i64),
            market_inbox_capacity: setup.input_capacity.saturating_mul(2),
            lane_capacity,
            spin_interval: setup.spin_interval,
            sweep_deadline,
        },
    })?;
    Ok(SimEdge {
        spawn: Box::new(move |runtime: &tokio::runtime::Handle| SimActor::spawn(actor, runtime)),
        tap_gate,
    })
}

fn venue_limits(row: &InstrumentRow) -> Result<InstrumentLimits, EngineError> {
    let missing = |detail: &'static str| EngineError::ExecutionNotWired {
        mode: "sim",
        detail,
    };
    Ok(InstrumentLimits {
        tick: row
            .tick_size
            .ok_or(missing("the instrument has no stamped tick size"))?,
        step: row
            .lot_size
            .ok_or(missing("the instrument has no stamped lot size"))?,
        min_qty: row
            .min_qty
            .ok_or(missing("the instrument has no stamped minimum quantity"))?,
        min_notional: row
            .min_notional
            .ok_or(missing("the instrument has no stamped minimum notional"))?,
        max_orders_per_side: row
            .max_num_orders
            .ok_or(missing("the instrument has no stamped order-count limit"))?
            .try_into()
            .unwrap_or(u16::MAX),
        max_amends: row
            .max_num_order_amends
            .ok_or(missing("the instrument has no stamped amend-count limit"))?
            .try_into()
            .unwrap_or(u8::MAX),
    })
}
