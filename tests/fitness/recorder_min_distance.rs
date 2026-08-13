use polysim::adapters::exec::open_orders_snapshot_end;
use polysim::config::{IntensitySpec, KlineInterval, TableKind};
use polysim::hot::dispatch::{ExecWiring, HotEngine, HotEngineSetup};
use polysim::hot::exec::{ExecLimits, ExecSettings, FeeModel, OrderBudget};
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
use crate::micro_strategy::models::MIN_QUOTE_DISTANCE_BPS;
use crate::micro_strategy::{MicroRecorder, MicroRecorderParams};

const INSTRUMENT: InstrumentId = InstrumentId(0);
const BASE_ASSET: AssetId = AssetId(0);
const QUOTE_ASSET: AssetId = AssetId(1);

const TICK: i64 = ONE / 100;
const BASE: i64 = 10_000 * ONE;
const STEP: i64 = ONE / 100_000;

const SPIN_INTERVAL: DurationUs = DurationUs::from_secs(3);
const STEPS: i64 = 900;
const STEP_US: i64 = 500_000;

#[test]
fn a_quote_is_never_tighter_than_the_minimum_distance() {
    let instruments = [recorded_row()];
    let (mut engine, mut persist, mut commands) = recorder_engine(&instruments);
    make_ready(&mut engine, 0);
    seed_book(&mut engine, 0);

    let mut bid_checked = false;
    let mut ask_checked = false;
    for index in 0..STEPS {
        let when = index * STEP_US;
        for message in step_messages(index, when) {
            engine.dispatch(pop(0, 0), &message);
        }
        let columns = spin_columns(&mut persist);
        let issued = drain_places(&mut commands);
        if issued.is_empty() {
            continue;
        }
        let mid = column_value(&columns, "mid").expect("mid was not recorded on a quoting spin");
        let tick_bps = TICK as f64 / mid * 1e4;
        for order in &issued {
            let half_spread_column = match order.side {
                Side::Buy => "gueant_bid_half_spread_bps",
                Side::Sell => "gueant_ask_half_spread_bps",
            };
            let half_spread = column_value(&columns, half_spread_column).unwrap_or_else(|| {
                panic!("{half_spread_column} was not recorded on the spin that placed {order:?}")
            });
            assert!(
                half_spread < MIN_QUOTE_DISTANCE_BPS,
                "the fixture must solve a depth below the floor so the clamp is what places the \
                 quote — got h = {half_spread} bps; quieten the tape, never widen the pin"
            );
            let distance_bps = (order.price.to_f64() - mid).abs() / mid * 1e4;
            assert!(
                distance_bps >= MIN_QUOTE_DISTANCE_BPS - 1e-9,
                "{:?} order at {} sits {distance_bps} bps from mid {mid}, tighter than the \
                 {MIN_QUOTE_DISTANCE_BPS} bps floor",
                order.side,
                order.price.to_f64(),
            );
            assert!(
                distance_bps <= MIN_QUOTE_DISTANCE_BPS + tick_bps + 1e-9,
                "{:?} order at {} sits {distance_bps} bps from mid {mid} — the clamp lands on the \
                 first grid tick past the floor, never further",
                order.side,
                order.price.to_f64(),
            );
            match order.side {
                Side::Buy => bid_checked = true,
                Side::Sell => ask_checked = true,
            }
        }
        if bid_checked && ask_checked {
            break;
        }
    }
    assert!(
        bid_checked && ask_checked,
        "both sides must place within the fixture: bid {bid_checked}, ask {ask_checked}"
    );
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

fn make_ready(engine: &mut HotEngine, when: i64) {
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::StreamReady,
            ..exec_event(INSTRUMENT, ClientOrderId(0), Side::Buy, 0, when)
        }),
    );
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
