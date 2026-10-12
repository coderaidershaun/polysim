//! The engine, tape and readers the shipped recorder's seam tests share. Each test file owns its
//! price level and lot step, because those are what its claim is about.

use polysim::adapters::exec::open_orders_snapshot_end;
use polysim::config::{IntensitySpec, KlineInterval, TableKind};
use polysim::hot::dispatch::{ExecWiring, HotEngine, HotEngineSetup};
use polysim::hot::strategy::StrategyConfig;
use polysim::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
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
use crate::micro_strategy::{MicroRecorder, MicroRecorderParams};
use crate::risk_gate::exec_settings;

const INSTRUMENT: InstrumentId = InstrumentId(0);
const BASE_ASSET: AssetId = AssetId(0);
const QUOTE_ASSET: AssetId = AssetId(1);

pub(crate) const TICK: i64 = ONE / 100;

pub(crate) const SPIN_INTERVAL: DurationUs = DurationUs::from_secs(3);

/// One closed candle per step, so the EGARCH close floor is reached in hundreds of messages rather
/// than the hundreds of seconds a realistic 1m cadence would take. The fit reads a SERIES, not a
/// calendar, so compressing the arrivals changes when it warms and not what it warms into.
pub(crate) const STEPS: i64 = 900;
pub(crate) const STEP_US: i64 = 500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlacedOrder {
    pub(crate) side: Side,
    pub(crate) price: Price,
    pub(crate) qty: Qty,
}

pub(crate) fn drain_places(commands: &mut Consumer<ExecLaneItem>) -> Vec<PlacedOrder> {
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

pub(crate) fn spin_columns(persist: &mut Consumer<PersistRecord>) -> Vec<(&'static str, f64)> {
    let mut columns = Vec::new();
    while let Ok(record) = persist.pop() {
        if let PersistRecord::Feature(row) = record {
            columns.push((FEATURE_NAMES[usize::from(row.feature.0)], row.value));
        }
    }
    columns
}

pub(crate) fn column_value(columns: &[(&'static str, f64)], name: &str) -> Option<f64> {
    columns
        .iter()
        .find(|(column, _)| *column == name)
        .map(|(_, value)| *value)
}

pub(crate) fn recorder_engine(
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

/// Stream up, open orders known, balances known — without all three nothing is ever placed and every
/// assertion below would pass for the wrong reason.
pub(crate) fn make_ready(engine: &mut HotEngine, when: i64) {
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::StreamReady,
            ..exec_event(INSTRUMENT, ClientOrderId(0), Side::Buy, 0, when)
        }),
    );
    // The open-order marker comes from the constructor the Binance actor sends it with, never
    // synthesised here: a hand-built one keeps these tests green while production emits nothing.
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

pub(crate) fn seed_book(engine: &mut HotEngine, base: i64, when: i64) {
    // A two-tick touch. The Guéant re-anchoring is `A·e^(k·half_spread_ticks)`, so a wide synthetic
    // spread overflows to infinity on a steep fitted k and the whole quote family goes null.
    let (bids, asks) = snapshot_pair(
        0,
        &[(base, 2 * ONE), (base - 5 * TICK, ONE)],
        &[(base + 2 * TICK, ONE), (base + 7 * TICK, 2 * ONE)],
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
pub(crate) fn step_messages(base: i64, index: i64, when: i64) -> [InboundMessage; 4] {
    let (side, price) = match index % 2 == 0 {
        true => (Side::Buy, base + 2 * TICK + (index % 4) * TICK),
        false => (Side::Sell, base - (index % 4) * TICK),
    };
    let (level_side, level) = match index % 2 == 0 {
        true => (Side::Sell, base + 7 * TICK),
        false => (Side::Buy, base - 5 * TICK),
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
                base,
                base + 5 * TICK,
                base - 5 * TICK,
                base + (index % 5) * TICK,
            ),
            true,
            when,
        )),
        InboundMessage::SpinTick(spin(index as u64, when)),
    ]
}

pub(crate) fn recorded_row(step: i64) -> InstrumentRow {
    let mut row = instrument_row(0, tracker_spec_all(100), 128);
    row.tick_size = Some(Price(TICK));
    row.lot_size = Some(Qty(step));
    row.min_qty = Some(Qty(step));
    row.min_notional = Some(ONE);
    row.tracker.intensity = Some(IntensitySpec {
        max_depth_ticks: 16,
        half_life_secs: 600.0,
        min_events: 5.0,
    });
    row
}
