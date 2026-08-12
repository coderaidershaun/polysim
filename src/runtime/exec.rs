//! Shared bring-up for live and simulated execution edges.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rtrb::RingBuffer;

use crate::adapters::backoff::BackoffCaps;
use crate::adapters::binance::exec as binance_exec;
use crate::adapters::binance::exec::{
    BinanceExecAdapter, BinanceExecAdapterContext, BinanceExecAdapterSetup, RecvWindow,
};
use crate::adapters::exchange_sim;
use crate::adapters::exec::{EdgeHandle, EngineIdentity, LeaseNamespace, TeTag, VenueCapabilities};
use crate::adapters::polymarket::exec as polymarket_exec;
use crate::adapters::polymarket::exec::handle::{
    PolymarketExecAdapter, PolymarketExecAdapterContext, PolymarketExecAdapterSetup,
};
use crate::adapters::polymarket::rotation::WindowAssignment;
use crate::config::{
    DEFAULT_QUOTE_STOP_MARGIN_MS, ExecutionConfig, ExecutionMode, RunIdentity, validated_mantissa,
};
use crate::hot::dispatch::ExecWiring;
use crate::hot::exec::{
    ExecLaneBudget, ExecLimits, ExecSettings, FeeModel, MAX_ORDER_INSTRUMENTS, exec_lane_capacity,
};
use crate::hot::spawn::{QueueProducer, SimTapGate};
use crate::info;
use crate::msg::exec::ExecLaneItem;
use crate::registry::Registry;
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::sink::ExecSink;
use crate::time::{DurationUs, EngineClock};

use super::EngineError;
use super::exec_identity::ExecutionLease;
use super::prepare::VenuePreflight;
use super::queues::MarketTaps;
use super::rate_limits::order_budget;
use super::sim::{SimBringUp, SimEdge, sim_bring_up};

/// The cancel sweep's share of the drain deadline, as a fraction rather than a fixed
/// reserve: half goes to the sweep, half to the hot-thread join and the file drains.
const SWEEP_SHARE_OF_DRAIN: u32 = 2;

/// The chosen venue's setup, held until the hot thread exists: an edge is decided during bring-up
/// but must not produce into the execution input queue before its consumer is running.
pub(super) type EdgeSpawn = Box<dyn FnOnce(&tokio::runtime::Handle) -> EdgeHandle + Send>;

pub(super) struct ExecBringUp {
    pub(super) wiring: Option<ExecWiring>,
    pub(super) spawn: Option<EdgeSpawn>,
    /// Only the simulated venue feeds itself from market taps, so only it has any to hold open
    /// across its own shutdown sweep.
    pub(super) tap_gate: Option<Arc<SimTapGate>>,
    pub(super) lease: Option<ExecutionLease>,
}

impl ExecBringUp {
    fn disabled() -> Self {
        ExecBringUp {
            wiring: None,
            spawn: None,
            tap_gate: None,
            lease: None,
        }
    }
}

pub(super) struct ExecBringUpSetup<'a> {
    pub(super) execution: Option<&'a ExecutionConfig>,
    /// Proof the startup gate passed. `None` when execution is disarmed or absent, since
    /// the probe never ran.
    pub(super) preflight: Option<VenuePreflight>,
    pub(super) registry: &'a Registry,
    pub(super) identity: &'a RunIdentity,
    /// The write end of the execution input queue; `None` if the registry allocated no queue.
    pub(super) producer: Option<QueueProducer>,
    pub(super) taps: Option<MarketTaps>,
    /// Rotation bindings flowing from the market-data actor to the execution edge. `None`
    /// on a source that mints none.
    pub(super) window_assignments: Option<tokio::sync::mpsc::Receiver<WindowAssignment>>,
    pub(super) clock: &'a EngineClock,
    pub(super) fatal: &'a FatalSignal,
    pub(super) desired_run_state: RunStateCell,
    pub(super) drain_deadline: Duration,
    pub(super) spin_interval: DurationUs,
    pub(super) input_capacity: usize,
    pub(super) lease_dir: &'a Path,
}

/// # Errors
/// [`EngineError::ExecutionNotWired`] when an armed config reaches here without the probe or the
/// input queue that arming implies, [`EngineError::ExecutionTooManyInstruments`] when the order
/// table cannot address every row, and `ExecutionIdentity*` when lease acquisition fails.
pub(super) fn exec_bring_up(mut setup: ExecBringUpSetup<'_>) -> Result<ExecBringUp, EngineError> {
    let Some(execution) = setup.execution.filter(|config| config.mode.is_enabled()) else {
        return Ok(ExecBringUp::disabled());
    };
    let mode = execution.mode.as_str();
    let Some(producer) = setup.producer.take() else {
        return Err(EngineError::ExecutionNotWired {
            mode,
            detail: "the registry allocated no execution input queue",
        });
    };
    let instruments = setup.registry.instruments();
    if instruments.len() > MAX_ORDER_INSTRUMENTS {
        return Err(EngineError::ExecutionTooManyInstruments {
            found: instruments.len(),
            max: MAX_ORDER_INSTRUMENTS,
        });
    }

    let te_tag = TeTag::of(setup.identity);
    let edge_stall = DurationUs::from_micros(BackoffCaps::default().max.as_micros() as i64);
    let lane_capacity = exec_lane_capacity(ExecLaneBudget {
        spin_interval: setup.spin_interval,
        edge_stall,
    });
    let (commands_producer, commands_consumer) = RingBuffer::<ExecLaneItem>::new(lane_capacity);
    info!(
        "execution command lane: {lane_capacity} slots ({} KiB), sized to hold a {}s edge stall at the configured {}ms spin",
        lane_capacity * size_of::<ExecLaneItem>() / 1_024,
        edge_stall.micros() / 1_000_000,
        setup.spin_interval.micros() / 1_000
    );
    let sweep_deadline = setup.drain_deadline / SWEEP_SHARE_OF_DRAIN;

    let lease_dir = setup.lease_dir;
    // Acquiring the lease spends a run nonce that client order ids are minted under, so a config
    // this venue cannot honour is refused before one is burnt on it.
    let refuse_or_acquire_lease = move |capabilities: &VenueCapabilities,
                                        namespace: &LeaseNamespace<'_>|
          -> Result<ExecutionLease, EngineError> {
        check_execution_against_venue(capabilities, execution)?;
        ExecutionLease::acquire(lease_dir, te_tag, namespace)
    };

    // Each venue's physics come from the venue's own module. Read here, where the venue is known,
    // and carried into `settings` — the hot engine stays venue-neutral and never names one.
    let (lease, spawn, tap_gate, venue, capabilities) = match execution.mode {
        ExecutionMode::Sim => {
            let capabilities = exchange_sim::capabilities();
            let lease = refuse_or_acquire_lease(&capabilities, &exchange_sim::lease_namespace())?;
            let identity = EngineIdentity {
                te_tag,
                run_nonce: lease.run_nonce(),
            };
            let taps = setup.taps.take();
            let SimEdge { spawn, tap_gate } = sim_bring_up(SimBringUp {
                setup: &setup,
                execution,
                identity,
                commands: commands_consumer,
                producer,
                taps,
                lane_capacity,
                sweep_deadline,
            })?;
            (
                lease,
                spawn,
                Some(tap_gate),
                "the simulated binance spot venue".to_owned(),
                capabilities,
            )
        }
        ExecutionMode::Live => match setup.preflight.take() {
            None => {
                return Err(EngineError::ExecutionNotWired {
                    mode,
                    detail: "the startup permission probe was never run",
                });
            }
            Some(VenuePreflight::Binance(preflight)) => {
                let env = setup.registry.binance_env().expect(
                    "a binance probe implies a binance source, which stamps its deployment",
                );
                let capabilities =
                    binance_exec::capabilities(order_budget(setup.registry.order_rate_limits())?);
                let lease = refuse_or_acquire_lease(
                    &capabilities,
                    &binance_exec::lease_namespace(
                        env,
                        preflight.credentials.api_key().expose_bytes(),
                    ),
                )?;
                let identity = EngineIdentity {
                    te_tag,
                    run_nonce: lease.run_nonce(),
                };
                let live = BinanceExecAdapterSetup {
                    instruments: instruments.to_vec(),
                    assets: setup.registry.assets().clone(),
                    credentials: preflight.credentials,
                    rest: preflight.rest,
                    commands: commands_consumer,
                    producer,
                    context: BinanceExecAdapterContext {
                        env,
                        clock: setup.clock.clone(),
                        fatal: setup.fatal.clone(),
                        run_state: setup.desired_run_state.clone(),
                        backoff: BackoffCaps::default(),
                        identity,
                        max_orders_per_side: execution.max_orders_per_side as usize,
                        recv_window: RecvWindow::from_millis(execution.recv_window_ms)
                            .map_err(EngineError::BinanceRecvWindow)?,
                        sweep_deadline,
                        disconnect_sweep_after: DurationUs::from_secs(
                            execution.disconnect_sweep_secs as i64,
                        ),
                        loud_clock_skew: DurationUs::from_millis(
                            execution.max_clock_skew_ms as i64,
                        ),
                    },
                };
                (
                    lease,
                    Box::new(move |runtime: &tokio::runtime::Handle| {
                        BinanceExecAdapter::spawn(live, runtime)
                    }) as EdgeSpawn,
                    None,
                    format!("binance {}", env.as_str()),
                    capabilities,
                )
            }
            Some(VenuePreflight::Polymarket(preflight)) => {
                // Before the lease: a refused edge must not burn a run nonce it never places under.
                PolymarketExecAdapter::check_available(&preflight.wallet)
                    .map_err(EngineError::PolymarketExecutionUnavailable)?;
                let capabilities = polymarket_exec::capabilities();
                let signer = preflight.wallet.signer.to_checksum_hex();
                let lease = refuse_or_acquire_lease(
                    &capabilities,
                    &polymarket_exec::lease_namespace(&signer),
                )?;
                let live = PolymarketExecAdapterSetup {
                    instruments: instruments.to_vec(),
                    credentials: preflight.credentials,
                    key: preflight.key,
                    wallet: preflight.wallet,
                    commands: commands_consumer,
                    producer,
                    assignments: setup.window_assignments.take().ok_or(
                        EngineError::ExecutionNotWired {
                            mode,
                            detail: "no rotation binding channel reaches the execution edge",
                        },
                    )?,
                    context: PolymarketExecAdapterContext {
                        clock: setup.clock.clone(),
                        fatal: setup.fatal.clone(),
                        run_state: setup.desired_run_state.clone(),
                        backoff: BackoffCaps::default(),
                        venue_clock_offset: preflight.venue_clock_offset,
                        max_orders_per_side: execution.max_orders_per_side as usize,
                        sweep_deadline,
                        disconnect_sweep_after: DurationUs::from_secs(
                            execution.disconnect_sweep_secs as i64,
                        ),
                    },
                };
                (
                    lease,
                    Box::new(move |runtime: &tokio::runtime::Handle| {
                        PolymarketExecAdapter::spawn(live, runtime)
                    }) as EdgeSpawn,
                    None,
                    "polymarket".to_owned(),
                    capabilities,
                )
            }
        },
        ExecutionMode::Off => unreachable!("disabled execution was filtered above"),
    };

    let run_nonce = lease.run_nonce();
    info!(
        "execution {mode} on {venue}: sweep deadline {}ms inside a {}ms drain, run nonce {run_nonce:08x}",
        sweep_deadline.as_millis(),
        setup.drain_deadline.as_millis()
    );
    Ok(ExecBringUp {
        wiring: Some(ExecWiring {
            sink: ExecSink::new(commands_producer),
            settings: settings(execution, &capabilities),
            run_nonce,
        }),
        spawn: Some(spawn),
        tap_gate,
        lease: Some(lease),
    })
}

/// Settings whose meaning depends on the venue, checked once the venue is known. Config validation
/// bounds each of these on its own; only here is there anything to compare them against.
///
/// # Errors
/// [`EngineError::ExecutionFieldInert`] for a setting this venue never reads, and
/// [`EngineError::ExecutionBaseFloorOnPosition`] for the one that does reach it and does harm.
fn check_execution_against_venue(
    capabilities: &VenueCapabilities,
    execution: &ExecutionConfig,
) -> Result<(), EngineError> {
    // Two settings are deliberately not checked here: recv_window_ms and max_clock_skew_ms bound a
    // request-signing protocol rather than anything a venue does. Each is bounded already — the
    // first where the binance edge turns it into a signed window, the second by config validation
    // for every venue — so on a venue that reads neither they are merely inert, which is accepted.
    // Giving a signing detail a capability of its own would grow VenueCapabilities every time one
    // venue gains a knob.
    if !capabilities.rotates_markets
        && execution.quote_stop_margin_ms != DEFAULT_QUOTE_STOP_MARGIN_MS
    {
        return Err(EngineError::ExecutionFieldInert {
            field: "quote_stop_margin_ms",
            value: execution.quote_stop_margin_ms.to_string().into(),
            venue_fact: "this venue's instruments never expire, so there is no closing window to \
                         stop quoting ahead of",
        });
    }
    if capabilities.fee_model == FeeModel::None && execution.taker_fee_rate > 0.0 {
        return Err(EngineError::ExecutionFieldInert {
            field: "taker_fee_rate",
            value: execution.taker_fee_rate.to_string().into(),
            venue_fact: "this venue takes its cut out of what a trade receives rather than \
                         charging it on top, so a flatten reserves the notional alone",
        });
    }
    if capabilities.base_asset_is_position && execution.min_base_balance > 0.0 {
        return Err(EngineError::ExecutionBaseFloorOnPosition {
            value: execution.min_base_balance.to_string().into(),
        });
    }
    Ok(())
}

fn settings(execution: &ExecutionConfig, capabilities: &VenueCapabilities) -> ExecSettings {
    ExecSettings {
        limits: ExecLimits {
            requote_threshold_ticks: execution.requote_threshold_ticks,
            // Expressed in hundredths of a basis point, so the band compares as integers —
            // comparing as floats would risk both a spurious violation and a determinism hazard.
            max_quote_distance_centi_bps: (execution.max_quote_distance_bps * 100.0).round() as i64,
            max_book_age: DurationUs::from_millis(execution.max_book_age_ms as i64),
            max_order_notional_quote: validated_mantissa(execution.max_order_notional_quote),
        },
        max_orders_per_side: execution.max_orders_per_side as usize,
        min_base_balance: validated_mantissa(execution.min_base_balance),
        min_quote_balance: validated_mantissa(execution.min_quote_balance),
        max_consecutive_rejects: execution.max_consecutive_rejects,
        max_session_loss_quote: validated_mantissa(execution.max_session_loss_quote),
        inflight_timeout: DurationUs::from_millis(execution.inflight_timeout_ms as i64),
        exec_silence_spins: execution.exec_silence_spins,
        order_reap_window: DurationUs::from_secs(execution.order_reap_secs as i64),
        quote_stop_margin: DurationUs::from_millis(execution.quote_stop_margin_ms as i64),
        flatten_slack_ticks: execution.flatten_slack_ticks,
        order_budget: capabilities.order_budget,
        fee_model: capabilities.fee_model,
        taker_fee_rate: validated_mantissa(execution.taker_fee_rate),
        holds_reservations_until_settled: capabilities.holds_reservations_until_settled,
    }
}
