//! The shipped recorder's declaration seam: what it RECORDS as its quote and what it actually asks
//! the venue for must be the same number.
//!
//! The recorder snaps its Guéant level to the NEAREST tick because that is the honest figure for a
//! research column, while the engine snaps a bid down and an ask up. The two agree only because the
//! strategy's price is already on the venue's grid, from the same `tick_size` — and nothing about
//! that is enforced by a type. Break it and `gueant_bid_price` describes an order that was never
//! placed, which is a research dataset quietly documenting a different strategy from the one that
//! traded. The size is the same story one layer down: `order_notional` is in QUOTE units and only
//! becomes a base quantity through `qty_at`, so a units slip there prices right and trades wrong.
//!
//! This drives the REAL `MicroRecorder` against the real engine, because both halves of the
//! agreement are only present when the real strategy meets the real reconciler.

use polysim::adapters::exec::open_orders_snapshot_end;
use polysim::config::{IntensitySpec, KlineInterval, TableKind};
use polysim::hot::dispatch::{ExecWiring, HotEngine, HotEngineSetup};
use polysim::hot::exec::{ExecLimits, ExecSettings, FeeModel, OrderBudget};
use polysim::hot::strategy::StrategyConfig;
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{
    AccountChunk, AccountChunkKind, AssetBalance, ExecCommand, ExecEvent, ExecKind, ExecLaneItem,
};
use polysim::msg::inbound::InboundMessage;
use polysim::msg::persist::PersistRecord;
use polysim::registry::InstrumentRow;
use polysim::sink::ExecSink;
use polysim::time::DurationUs;
use rtrb::{Consumer, RingBuffer};

use crate::engine_support::{
    ONE, book_reset, delta_chunk, detached_exposure, engine_view, exec_event, instrument_row,
    kline, metrics_ring, persist_ring, pop, recorder_spec, snapshot_pair, spin, strategy_log_ring,
    tracker_spec_all, trade, ts, ui_book_ring, ui_event_ring,
};
use crate::micro_strategy::features::FEATURE_NAMES;
use crate::micro_strategy::models::qty_at;
use crate::micro_strategy::{MicroRecorder, MicroRecorderParams};

const INSTRUMENT: InstrumentId = InstrumentId(0);
const BASE_ASSET: AssetId = AssetId(0);
const QUOTE_ASSET: AssetId = AssetId(1);

const TICK: i64 = ONE / 100;
const BASE: i64 = 100 * ONE;

/// Coarse enough that the engine's step snap actually bites: `order_notional / price` lands on a
/// quantity this does not divide, so a test that ignored the snap would read a different number.
const STEP: i64 = ONE / 1_000;

/// The shipped default, in QUOTE units.
const ORDER_NOTIONAL: i64 = 10 * ONE;

const SPIN_INTERVAL: DurationUs = DurationUs::from_secs(3);

/// One closed candle per step, so the EGARCH close floor is reached in hundreds of messages rather
/// than the hundreds of seconds a realistic 1m cadence would take. The fit reads a SERIES, not a
/// calendar, so compressing the arrivals changes when it warms and not what it warms into.
const STEPS: i64 = 900;
const STEP_US: i64 = 500_000;

/// FITNESS: the recorded quote price and the placed order price are the same number, and the placed
/// size is `order_notional` converted at that price and snapped down onto the venue step.
#[test]
fn the_recorded_quote_is_the_order_that_gets_placed() {
    let instruments = [recorded_row()];
    let (mut engine, mut persist, mut commands) = recorder_engine(&instruments);
    make_ready(&mut engine, 0);
    seed_book(&mut engine, 0);

    let mut placed = Vec::new();
    let mut columns = Vec::new();
    for index in 0..STEPS {
        let when = index * STEP_US;
        for message in step_messages(index, when) {
            engine.dispatch(pop(0, 0), &message);
        }
        let spun = spin_columns(&mut persist);
        let issued = drain_places(&mut commands);
        if !issued.is_empty() {
            placed = issued;
            columns = spun;
            break;
        }
    }

    assert_eq!(
        placed.len(),
        2,
        "the recorder declares both sides every spin, so its first quoting spin places two orders \
         — got {placed:?}"
    );
    for order in &placed {
        let column = match order.side {
            Side::Buy => "gueant_bid_price",
            Side::Sell => "gueant_ask_price",
        };
        let recorded = column_value(&columns, column).unwrap_or_else(|| {
            panic!("{column} was not recorded on the spin that placed {order:?}")
        });
        assert_eq!(
            order.price,
            Price((recorded * FIXED_SCALE as f64).round() as i64),
            "{column} recorded {recorded}, but the order went out at {}: the research column now \
             describes a price nobody quoted",
            order.price.to_f64(),
        );
        let wanted = qty_at(ORDER_NOTIONAL, order.price);
        assert_eq!(
            order.qty,
            Qty(wanted.0 - wanted.0.rem_euclid(STEP)),
            "order_notional {} at {} is {} base, which snaps down to the {STEP} step — the order \
             asked for {} instead",
            ORDER_NOTIONAL,
            order.price.to_f64(),
            wanted.0,
            order.qty.0,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacedOrder {
    side: Side,
    price: Price,
    qty: Qty,
}

fn drain_places(commands: &mut Consumer<ExecLaneItem>) -> Vec<PlacedOrder> {
    let mut placed = Vec::new();
    while let Ok(item) = commands.pop() {
        let ExecLaneItem::Command(stamped) = item else {
            continue;
        };
        let ExecCommand::Place {
            side, price, qty, ..
        } = stamped.command
        else {
            continue;
        };
        placed.push(PlacedOrder { side, price, qty });
    }
    placed
}

fn spin_columns(persist: &mut Consumer<PersistRecord>) -> Vec<(&'static str, f64)> {
    let mut columns = Vec::new();
    while let Ok(record) = persist.pop() {
        if let PersistRecord::Feature(row) = record {
            columns.push((FEATURE_NAMES[usize::from(row.feature.0)], row.value));
        }
    }
    columns
}

fn column_value(columns: &[(&'static str, f64)], name: &str) -> Option<f64> {
    columns
        .iter()
        .find(|(column, _)| *column == name)
        .map(|(_, value)| *value)
}

fn recorder_engine(
    instruments: &[InstrumentRow],
) -> (HotEngine, Consumer<PersistRecord>, Consumer<ExecLaneItem>) {
    let (persistence, persist) = persist_ring(8_192);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (ui_book_sink, _ui_books) = ui_book_ring(64);
    let (ui_event_sink, _ui_events) = ui_event_ring(1_024);
    let (commands_producer, commands) = RingBuffer::<ExecLaneItem>::new(256);
    let spec = recorder_spec::<MicroRecorderParams>(vec![TableKind::Features]);
    let engine = HotEngine::new(HotEngineSetup {
        instruments,
        strategy: Box::new(MicroRecorder::from_spec(&spec, engine_view(SPIN_INTERVAL))),
        persistence: Some(persistence),
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
        exec: Some(ExecWiring {
            sink: ExecSink::new(commands_producer),
            settings: exec_settings(),
            run_nonce: 0,
        }),
        exposure: detached_exposure(),
    });
    (engine, persist, commands)
}

/// Every limit wide open but the grid: a quote refused by the price band, a stale book or the funds
/// floor would read here as a strategy that declared nothing, and this test is about what it DID
/// declare.
fn exec_settings() -> ExecSettings {
    ExecSettings {
        limits: ExecLimits {
            requote_threshold_ticks: 1,
            max_quote_distance_centi_bps: 100_000_000,
            max_book_age: DurationUs::from_secs(3_600),
            max_order_notional_quote: 1_000_000 * ONE,
        },
        max_orders_per_side: 1,
        min_base_balance: 0,
        min_quote_balance: 0,
        max_consecutive_rejects: 5,
        max_session_loss_quote: 1_000_000 * ONE,
        inflight_timeout: DurationUs::from_secs(3_600),
        // No silence sweep: a reconciliation request in the command stream is noise this test would
        // have to filter rather than a behaviour it is about.
        exec_silence_spins: u32::MAX,
        order_reap_window: DurationUs::from_secs(3_600),
        quote_stop_margin: DurationUs::ZERO,
        flatten_slack_ticks: 0,
        order_budget: OrderBudget::NONE,
        fee_model: FeeModel::None,
        taker_fee_rate: 0,
        holds_reservations_until_settled: true,
    }
}

/// Stream up, open orders known, balances known — without all three nothing is ever placed and every
/// assertion below would pass for the wrong reason.
fn make_ready(engine: &mut HotEngine, when: i64) {
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::StreamReady,
            ..exec_event(INSTRUMENT, ClientOrderId(0), Side::Buy, 0, when)
        }),
    );
    // The open-order marker comes from the constructor the Binance actor sends it with, never
    // synthesised here: a hand-built one keeps this test green while production emits nothing.
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(open_orders_snapshot_end(INSTRUMENT, ts(when))),
    );
    let mut balances = [AssetBalance {
        asset: AssetId::UNKNOWN,
        free: 0,
        locked: 0,
    }; polysim::msg::exec::ACCOUNT_CHUNK_ASSETS];
    balances[0] = AssetBalance {
        asset: BASE_ASSET,
        free: 1_000 * ONE,
        locked: 0,
    };
    balances[1] = AssetBalance {
        asset: QUOTE_ASSET,
        free: 1_000_000 * ONE,
        locked: 0,
    };
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Account(AccountChunk {
            kind: AccountChunkKind::Snapshot,
            balances,
            len: 2,
            is_last_chunk: true,
            venue_update_ts_ms: 1,
            exchange_ts_us: ts(when),
            received_ts_us: ts(when),
            queued_ts_us: ts(when),
        }),
    );
}

fn seed_book(engine: &mut HotEngine, when: i64) {
    // A two-tick touch. The Guéant re-anchoring is `A·e^(k·half_spread_ticks)`, so a wide synthetic
    // spread overflows to infinity on a steep fitted k and the whole quote family goes null.
    let (bids, asks) = snapshot_pair(
        0,
        &[(BASE, 2 * ONE), (BASE - 5 * TICK, ONE)],
        &[(BASE + 2 * TICK, ONE), (BASE + 7 * TICK, 2 * ONE)],
        when + 1,
    );
    for message in [
        InboundMessage::BookReset(book_reset(0, when)),
        InboundMessage::Book(bids),
        InboundMessage::Book(asks),
    ] {
        engine.dispatch(pop(0, 0), &message);
    }
}

/// One step: a print walking into its own side of the book, depth churn, a closed candle, and the
/// spin that emits. Both aggressor sides, because a side whose prints never reach a level never
/// identifies an intensity fit and never produces a Guéant quote at all.
fn step_messages(index: i64, when: i64) -> [InboundMessage; 4] {
    let (side, price) = match index % 2 == 0 {
        true => (Side::Buy, BASE + 2 * TICK + (index % 4) * TICK),
        false => (Side::Sell, BASE - (index % 4) * TICK),
    };
    let (level_side, level) = match index % 2 == 0 {
        true => (Side::Sell, BASE + 7 * TICK),
        false => (Side::Buy, BASE - 5 * TICK),
    };
    [
        InboundMessage::Trade(trade(0, price, ONE / 100, side, when)),
        InboundMessage::Book(delta_chunk(
            0,
            level_side,
            &[(level, (1 + index % 5) * ONE)],
            when,
        )),
        InboundMessage::Kline(kline(
            0,
            KlineInterval::OneMinute,
            (
                BASE,
                BASE + 5 * TICK,
                BASE - 5 * TICK,
                BASE + (index % 5) * TICK,
            ),
            true,
            when,
        )),
        InboundMessage::SpinTick(spin(index as u64, when)),
    ]
}

fn recorded_row() -> InstrumentRow {
    let mut row = instrument_row(0, tracker_spec_all(100), 128);
    row.tick_size = Some(Price(TICK));
    row.lot_size = Some(Qty(STEP));
    row.min_qty = Some(Qty(STEP));
    row.min_notional = Some(ONE);
    row.tracker.intensity = Some(IntensitySpec {
        max_depth_ticks: 16,
        half_life_secs: 600.0,
        min_events: 5.0,
    });
    row
}
