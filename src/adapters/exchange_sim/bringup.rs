//! Standing a simulated venue up from a run's configuration: the matching model, the delivery
//! schedule, the wire and the lifecycle core are assembled here rather than by the caller, so the
//! runtime hands over a spec and never learns how the venue is put together.

use std::time::Duration;

use rtrb::Consumer;

use crate::adapters::binance::exec::SymbolTable;
use crate::adapters::exec::{EngineIdentity, ExecCore};
use crate::config::{SimConfig, validated_mantissa};
use crate::hot::exec::MAX_ORDER_INSTRUMENTS;
use crate::hot::spawn::QueueProducer;
use crate::info;
use crate::msg::exec::ExecLaneItem;
use crate::msg::inbound::MarketTapItem;
use crate::registry::{AssetDictionary, InstrumentRow};
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::{DurationUs, EngineClock};

use super::core::latency::{LatencyBudget, StartupBudget};
use super::core::resting::{InstrumentLimits, ORDER_TABLE_CAPACITY};
use super::core::schedule::{DeliveryLimits, DeliverySchedule};
use super::core::wallet::{FeeBps, SimWalletSetup};
use super::core::{SimVenue, SimVenueSetup};
use super::driver::{SimExecDriver, SimExecDriverSetup};
use super::error::SimVenueError;
use super::lanes::SimLane;
use super::tap::MarketTapLane;
use super::wire::VenueWire;

const ANSWER_CAPACITY: usize = 4_096;

pub struct SimVenueSpec<'a> {
    pub sim: &'a SimConfig,
    pub instrument: &'a InstrumentRow,
    /// Proven by the caller from the registry row, so a venue cannot be built without the grid it
    /// quantises orders against.
    pub limits: InstrumentLimits,
    pub assets: AssetDictionary,
    pub symbols: SymbolTable,
    pub commands: rtrb::Consumer<ExecLaneItem>,
    pub producer: QueueProducer,
    pub trades: Consumer<MarketTapItem>,
    pub depth: Consumer<MarketTapItem>,
    pub clock: EngineClock,
    pub identity: EngineIdentity,
    pub run_state: RunStateCell,
    pub fatal: FatalSignal,
    pub settings: SimVenueSettings,
}

pub struct SimVenueSettings {
    pub max_orders_per_side: usize,
    pub verdict_retention: DurationUs,
    pub inflight_timeout: DurationUs,
    pub market_inbox_capacity: usize,
    pub lane_capacity: usize,
    pub spin_interval: DurationUs,
    pub sweep_deadline: Duration,
}

pub struct SimActorSetup {
    pub driver: SimExecDriver,
    pub commands: rtrb::Consumer<ExecLaneItem>,
    pub trades: MarketTapLane,
    pub depth: MarketTapLane,
    pub producer: QueueProducer,
    pub clock: EngineClock,
    pub assets: AssetDictionary,
    pub symbols: SymbolTable,
    pub identity: EngineIdentity,
    pub latency: LatencyBudget,
    pub lane_capacity: usize,
    pub run_state: RunStateCell,
    pub fatal: FatalSignal,
    pub sweep_deadline: Duration,
}

impl SimActorSetup {
    /// # Errors
    /// [`SimVenueError`] when the opening account cannot be spent from, or when the configured
    /// latencies leave no room to answer a command before it times out.
    pub fn assemble(spec: SimVenueSpec<'_>) -> Result<Self, SimVenueError> {
        let SimVenueSpec {
            sim,
            instrument,
            limits,
            assets,
            symbols,
            commands,
            producer,
            trades,
            depth,
            clock,
            identity,
            run_state,
            fatal,
            settings,
        } = spec;
        let SimVenueSettings {
            max_orders_per_side,
            verdict_retention,
            inflight_timeout,
            market_inbox_capacity,
            lane_capacity,
            spin_interval,
            sweep_deadline,
        } = settings;

        let latency = latency_budget(sim);
        let venue = SimVenue::new(SimVenueSetup {
            instrument: instrument.instrument_id,
            book_capacity: instrument.book_capacity,
            market_inbox_capacity,
            verdict_retention,
            limits,
            wallet: wallet_setup(instrument, sim)?,
        })?;
        info!(
            "simulated venue on {}: market inbox {market_inbox_capacity} events, order table \
             {ORDER_TABLE_CAPACITY} rows, verdict retention {}s — every one of them fatal on exhaustion",
            instrument.venue_symbol,
            verdict_retention.to_secs()
        );
        info!(
            "simulated venue latencies: entry {}ms, ack {}ms, hard market-data bound {}ms",
            sim.order_entry_latency_ms, sim.ack_latency_ms, sim.max_market_data_delay_ms
        );
        let worst_case = startup_budget(sim, spin_interval, inflight_timeout).check()?;
        info!(
            "simulated venue worst legal round trip {}ms inside a {}ms in-flight timeout",
            worst_case.micros() / 1_000,
            inflight_timeout.micros() / 1_000
        );

        Ok(Self {
            driver: SimExecDriver::new(SimExecDriverSetup {
                core: ExecCore::with_limits(max_orders_per_side, MAX_ORDER_INSTRUMENTS),
                venue,
                schedule: DeliverySchedule::new(DeliveryLimits {
                    ack_latency: latency.ack,
                    answer_capacity: ANSWER_CAPACITY,
                }),
                wire: VenueWire::new(identity),
                instrument: instrument.instrument_id,
            }),
            commands,
            trades: MarketTapLane::new(trades, SimLane::Trade),
            depth: MarketTapLane::new(depth, SimLane::Depth),
            producer,
            clock,
            assets,
            symbols,
            identity,
            latency,
            lane_capacity,
            run_state,
            fatal,
            sweep_deadline,
        })
    }
}

fn latency_budget(sim: &SimConfig) -> LatencyBudget {
    LatencyBudget {
        order_entry: millis(sim.order_entry_latency_ms),
        ack: millis(sim.ack_latency_ms),
        max_market_data_delay: millis(sim.max_market_data_delay_ms),
    }
}

fn startup_budget(
    sim: &SimConfig,
    spin_interval: DurationUs,
    inflight_timeout: DurationUs,
) -> StartupBudget {
    let slowest_heartbeat = millis(sim.heartbeat_ms).max(spin_interval);
    StartupBudget {
        latency: latency_budget(sim),
        max_heartbeat_interval: slowest_heartbeat,
        inflight_timeout,
    }
}

fn wallet_setup(row: &InstrumentRow, sim: &SimConfig) -> Result<SimWalletSetup, SimVenueError> {
    Ok(SimWalletSetup {
        base_asset: row.base_asset,
        quote_asset: row.quote_asset,
        opening_base: validated_mantissa(sim.opening_base_balance),
        opening_quote: validated_mantissa(sim.opening_quote_balance),
        maker_fee_bps: FeeBps::new(sim.maker_fee_bps as i64)?,
    })
}

fn millis(ms: u64) -> DurationUs {
    DurationUs::from_millis(ms as i64)
}
