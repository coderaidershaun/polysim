//! Market-data adapter setup: one venue actor per producer group, spawned last so every ring it
//! writes into already has its consumer.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::adapters::backoff::BackoffCaps;
use crate::adapters::binance::actor::{
    BinanceAdapter, BinanceAdapterContext, BinanceAdapterHandle,
};
use crate::adapters::polymarket::actor::{
    PolymarketAdapter, PolymarketAdapterContext, PolymarketAdapterHandle,
};
use crate::adapters::polymarket::rotation::WindowAssignment;
use crate::adapters::rest_quiet::SharedRestQuiet;
use crate::config::{Config, VenueMarket};
use crate::hot::spawn::QueueProducer;
use crate::msg::persist::RotationRow;
use crate::registry::Registry;
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::EngineClock;

const DEFAULT_TAP_HEARTBEAT: Duration = Duration::from_millis(100);

pub(super) enum SpawnedAdapter {
    Binance(BinanceAdapterHandle),
    Polymarket(PolymarketAdapterHandle),
}

impl SpawnedAdapter {
    pub(super) async fn shutdown(self) {
        match self {
            SpawnedAdapter::Binance(handle) => handle.shutdown().await,
            SpawnedAdapter::Polymarket(handle) => handle.shutdown().await,
        }
    }
}

pub(super) fn tap_heartbeat<P>(config: &Config<P>) -> Duration {
    config
        .execution
        .as_ref()
        .and_then(|execution| execution.sim.as_ref())
        .map_or(DEFAULT_TAP_HEARTBEAT, |sim| {
            Duration::from_millis(sim.heartbeat_ms)
        })
}

pub(super) struct AdapterWiring<'a> {
    pub registry: &'a Registry,
    pub producers: Vec<QueueProducer>,
    pub clock: &'a EngineClock,
    pub fatal: &'a FatalSignal,
    /// Adapters hold their sockets down according to this state. The execution edge reads
    /// it freely, with no restriction of its own.
    pub desired_run_state: &'a RunStateCell,
    pub rotations_tx: &'a mpsc::Sender<RotationRow>,
    /// Rotation bindings for the execution edge; absent when nothing is listening for them.
    pub window_assignments: Option<mpsc::Sender<WindowAssignment>>,
    pub tap_heartbeat: Duration,
    /// One window for the whole Binance deployment: its actors and the signed order client draw on
    /// a single per-IP allowance, so a rate limit any of them earns has to quiet all of them.
    pub binance_rest_quiet: SharedRestQuiet,
    pub tokio_handle: &'a tokio::runtime::Handle,
}

pub(super) fn spawn_adapters(wiring: AdapterWiring<'_>) -> Vec<SpawnedAdapter> {
    let AdapterWiring {
        registry,
        producers,
        clock,
        fatal,
        desired_run_state,
        rotations_tx,
        window_assignments,
        tap_heartbeat,
        binance_rest_quiet,
        tokio_handle,
    } = wiring;
    let mut handles = Vec::with_capacity(producers.len());
    for (producer, group) in producers.into_iter().zip(registry.producer_groups()) {
        match group.market {
            VenueMarket::Binance(market) => {
                let context = BinanceAdapterContext {
                    // A Binance producer group implies the source config chose a Binance
                    // deployment, so these stay aligned.
                    env: registry
                        .binance_env()
                        .expect("a binance producer group implies a binance source"),
                    clock: clock.clone(),
                    fatal: fatal.clone(),
                    run_state: desired_run_state.clone(),
                    backoff: BackoffCaps::default(),
                    tap_heartbeat,
                    rest_quiet: binance_rest_quiet.clone(),
                };
                handles.push(SpawnedAdapter::Binance(BinanceAdapter::spawn(
                    group,
                    market,
                    registry.instruments(),
                    producer,
                    context,
                    tokio_handle,
                )));
            }
            VenueMarket::Polymarket(series) => {
                let context = PolymarketAdapterContext {
                    clock: clock.clone(),
                    fatal: fatal.clone(),
                    run_state: desired_run_state.clone(),
                    backoff: BackoffCaps::default(),
                    rotations_tx: rotations_tx.clone(),
                    window_assignments: window_assignments.clone(),
                };
                handles.push(SpawnedAdapter::Polymarket(PolymarketAdapter::spawn(
                    group,
                    series,
                    registry.instruments(),
                    producer,
                    context,
                    tokio_handle,
                )));
            }
        }
    }
    handles
}
