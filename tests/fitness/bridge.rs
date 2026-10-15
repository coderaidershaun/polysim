//! The bridge's wire protocol, which a web page codes against: the hello, catalog and tick lines for
//! a fixed scene stay exactly as checked in, and a view command changes the next frame and only what
//! it changes.
//!
//! A renamed or reshaped field fails here before it reaches the page, where it would show as a
//! blank panel rather than as an error. To accept an intended change to the protocol, run the suite
//! once with `POLYSIM_BLESS_BRIDGE=1` to rewrite `bridge_scene.json`, then read the diff.

use std::time::Duration;

use polysim::bridge::{Command, Encoder, TickInput, hello_line};
use polysim::config::ExecutionMode;
use polysim::desktop::dom_view::{DomGrouping, DomUnit, FeedStatus};
use polysim::desktop::model::UiModel;
use polysim::desktop::monitor_model::SystemNote;
use polysim::hot::exec::{ExecHalt, OrderState, RejectOrigin, RejectReason};
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::Liquidity;
use polysim::msg::persist::FeatureId;
use polysim::msg::ui::{
    DomQuote, UiBookState, UiCatalog, UiEvent, UiInstrument, UiLatencyCell, UiLatencyRow,
    UiLatencySummary,
};
use polysim::time::TsUs;
use serde_json::Value;

use crate::desktop_support::{BookFixture, SPIN, book_snapshot};

const FIXTURE: &str = include_str!("bridge_scene.json");

/// Where `POLYSIM_BLESS_BRIDGE` writes, relative to the package root the suite runs from.
const FIXTURE_PATH: &str = "tests/fitness/bridge_scene.json";

/// 0.01, so a tick index reads as a price in cents.
const TICK: Price = Price(1_000_000);

/// 0.00001, which gives quantities five decimals.
const LOT: Qty = Qty(1_000);

/// The first bucket of the scene, as seconds since the epoch.
const FIRST_SECOND: i64 = 1_700;

const BUCKETS: i64 = 10;

fn tick_price(index: i64) -> i64 {
    index * TICK.0
}

fn quantity(units: f64) -> i64 {
    (units * FIXED_SCALE as f64).round() as i64
}

fn at_second(second: i64) -> TsUs {
    TsUs::from_micros(second * 1_000_000 + 500_000)
}

fn catalog() -> UiCatalog {
    let instrument = |id: u16, display: &str, base: &str, base_asset: u16| UiInstrument {
        instrument_id: InstrumentId(id),
        display: display.into(),
        base: base.into(),
        quote: "USDT".into(),
        base_asset: AssetId(base_asset),
        quote_asset: AssetId(9),
        tick_size: Some(TICK),
        lot_size: Some(LOT),
        qty_scale: FIXED_SCALE,
    };
    UiCatalog {
        strategy_id: "strat-fitness".into(),
        window_title: "Polysim - strat-fitness @ 127.0.0.1:9310".into(),
        execution_mode: Some(ExecutionMode::Sim),
        spin_interval_us: SPIN.micros() as u64,
        instruments: vec![
            instrument(0, "BTCUSDT", "BTC", 0),
            instrument(1, "ETHUSDT", "ETH", 1),
        ],
        feature_names: vec![
            "obi_half_bp".into(),
            "spread_bps".into(),
            "microprice_gap".into(),
        ],
    }
}

/// Ten seconds of a BTC book around 118000, an order resting on the bid, one in flight on the ask,
/// one far off the ladder, a desired quote either side, prints, a fill, features, balances, a
/// refusal, latency and a position: every panel has something to say.
fn scene() -> UiModel {
    let mut model = UiModel::with_capacity(2, SPIN);
    model.set_catalog(catalog());
    model.set_dom_unit(DomUnit::Ticks);
    model.set_dom_grouping(DomGrouping::Ticks { per_bucket: 5 });
    model.note_lifecycle(SystemNote::Ready);

    let mut events: Vec<UiEvent> = Vec::new();
    for second in FIRST_SECOND..FIRST_SECOND + BUCKETS {
        let drift = second - FIRST_SECOND;
        let bid = 11_799_995 + drift;
        model.apply_book(book_snapshot(BookFixture {
            instrument: 0,
            seq: (second - FIRST_SECOND) as u64,
            event_ts: at_second(second).micros(),
            state: UiBookState::Valid,
            bids: &[
                (tick_price(bid), quantity(1.24)),
                (tick_price(bid - 3), quantity(0.5)),
                (tick_price(bid - 12), quantity(2.0)),
            ],
            asks: &[
                (tick_price(bid + 11), quantity(0.8)),
                (tick_price(bid + 14), quantity(1.1)),
                (tick_price(bid + 27), quantity(3.0)),
            ],
        }));
        for (index, value) in [0.0312, -1.5, 0.25].into_iter().enumerate() {
            events.push(UiEvent::Feature {
                instrument: InstrumentId(0),
                seq: 0,
                event_ts_us: at_second(second),
                feature: FeatureId(index as u16),
                value: value + drift as f64 * 0.001,
            });
        }
        events.push(UiEvent::Position {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: at_second(second),
            exposure_quote: 12.34 + drift as f64 * 100.0,
            pnl_quote: -0.05 + drift as f64 * 0.5,
        });
    }

    let last = at_second(FIRST_SECOND + BUCKETS - 1);
    let bid = 11_799_995 + BUCKETS - 1;
    let want_bid = tick_price(bid - 5);
    let want_ask = tick_price(bid + 20);
    events.push(UiEvent::Quote {
        instrument: InstrumentId(0),
        seq: 0,
        event_ts_us: last,
        quote: DomQuote::top(
            Some((Price(want_bid), Qty(quantity(0.05)))),
            Some((Price(want_ask), Qty(quantity(0.05)))),
        ),
    });
    let order =
        |id: u64, side: Side, state: OrderState, price: i64, units: f64| UiEvent::OrderUpdate {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: last,
            client_id: ClientOrderId(id),
            quote_level: None,
            side,
            state,
            price: Price(price),
            qty: Qty(quantity(units)),
            filled: Qty(0),
        };
    // The far order comes first on its side: a side reports the first placement it produces, so
    // an order that is on screen ahead of it would hide the chevron.
    events.extend([
        order(
            0x1a2d,
            Side::Sell,
            OrderState::Live,
            tick_price(bid + 400),
            0.03,
        ),
        order(0x1a2b, Side::Buy, OrderState::Live, want_bid, 0.05),
        order(
            0x1a2c,
            Side::Sell,
            OrderState::PendingNew,
            tick_price(bid + 30),
            0.02,
        ),
    ]);
    for (units, side) in [(0.01, Side::Sell), (0.02, Side::Buy)] {
        events.push(UiEvent::Trade {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: last,
            aggressor: side,
            price: Price(tick_price(bid + 1)),
            qty: Qty(quantity(units)),
        });
    }
    events.extend([
        UiEvent::Fill {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: at_second(FIRST_SECOND + 6),
            quote_level: None,
            side: Side::Buy,
            price: Price(tick_price(bid - 4)),
            qty: Qty(quantity(0.05)),
            commission: 12_300,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
        },
        UiEvent::Balance {
            asset: AssetId(0),
            seq: 0,
            event_ts_us: last,
            free: quantity(0.12),
            locked: 0,
        },
        UiEvent::Balance {
            asset: AssetId(9),
            seq: 0,
            event_ts_us: last,
            free: quantity(5_000.0),
            locked: quantity(250.0),
        },
        UiEvent::Execution {
            seq: 0,
            event_ts_us: last,
            halt: ExecHalt::Armed,
        },
        UiEvent::Reject {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: last,
            side: Side::Buy,
            origin: RejectOrigin::Local(RejectReason::Underfunded),
        },
        UiEvent::Latency {
            seq: 0,
            event_ts_us: last,
            summary: latency(),
        },
    ]);
    for (seq, mut event) in events.into_iter().enumerate() {
        event.set_seq(seq as u64);
        model.apply_event(event);
    }
    model
}

fn latency() -> UiLatencySummary {
    let cell = |count: u64, sum_us: i64| UiLatencyCell { count, sum_us };
    let row = UiLatencyRow {
        exchange_to_received: cell(2, 24_690),
        received_to_queued: cell(4, 80),
        queue_wait: cell(0, 0),
        processing: cell(10, 1_234),
        end_to_end: cell(3, 36_000),
        order_round_trip: cell(1, 1_234_567),
    };
    UiLatencySummary {
        market_data: row,
        execution: UiLatencyRow::default(),
        hot_path: row,
        backlog_ema: Some(0.4),
    }
}

fn parse(line: &str) -> Value {
    assert!(
        line.starts_with(r#"{"kind":""#),
        "every protocol line opens with its kind, got {line}"
    );
    assert!(!line.contains('\n'), "a protocol line is one line");
    serde_json::from_str(line).expect("a protocol line is json")
}

fn tick_input(model: &UiModel) -> TickInput<'_> {
    TickInput {
        model,
        session: 1,
        feed: FeedStatus::Live,
    }
}

fn apply(model: &mut UiModel, encoder: &mut Encoder, command: &str) {
    match Command::parse(command).expect("the test commands are valid") {
        Command::View(view) => view
            .apply(model)
            .expect("the model accepts the view change"),
        Command::Fit(fit) => encoder.fit(fit),
        other => panic!("{command} is not a view or fit command: {other:?}"),
    }
}

/// The scene with the page's room announced, encoded once: the catalog line and the tick line.
fn first_frames(model: &mut UiModel, encoder: &mut Encoder) -> Vec<String> {
    apply(model, encoder, r#"{"cmd":"chart_ticks","mid":9,"risk":5}"#);
    encoder.lines(&tick_input(model))
}

fn section<'a>(tick: &'a Value, name: &str) -> &'a Value {
    tick.get(name)
        .unwrap_or_else(|| panic!("the tick carries a {name} section: {tick}"))
}

fn count(value: &Value) -> usize {
    value.as_array().map_or(0, Vec::len)
}

/// The protocol is the contract between this process and the page, and every drift in it is silent
/// to the compiler: a renamed field still builds, and the page just stops painting a panel.
/// Pinning the frames of one fixed scene makes the drift a failing diff.
#[test]
fn a_fixed_scene_encodes_to_the_checked_in_protocol_frames() {
    let mut model = scene();
    let mut encoder = Encoder::new();
    let lines = first_frames(&mut model, &mut encoder);
    assert_eq!(lines.len(), 2, "a catalog line then a tick line: {lines:?}");

    let hello = hello_line("test", Duration::from_millis(16));
    let frames = Value::Array(vec![parse(&hello), parse(&lines[0]), parse(&lines[1])]);

    let tick = &frames[2];
    for name in [
        "view", "ladder", "summary", "account", "features", "channels", "latency", "chart", "risk",
        "gaps",
    ] {
        assert!(!section(tick, name).is_null(), "{name} is present");
    }
    let ladder = section(tick, "ladder");
    let rows: Vec<&Value> = ladder["bids"]
        .as_array()
        .into_iter()
        .chain(ladder["asks"].as_array())
        .flatten()
        .collect();
    assert!(
        rows.iter().any(|row| !row["order"].is_null()),
        "an order shows"
    );
    assert!(
        rows.iter().any(|row| !row["want"].is_null()),
        "a want shows"
    );
    assert!(
        rows.iter().any(|row| !row["ring"].is_null()),
        "a ring shows"
    );
    assert!(
        !ladder["chevrons"]["up"].is_null(),
        "the far order has a chevron"
    );
    let chart = section(tick, "chart");
    assert!(count(&chart["axis"]) > 0, "the mid axis has ticks");
    assert!(count(&chart["buckets"]) > 0, "the chart has buckets");
    assert!(count(&chart["fills"]) > 0, "the chart has the fill");
    let risk = section(tick, "risk");
    assert!(
        count(&risk["axis"]["exposure"]) > 0,
        "the risk axis has ticks"
    );
    assert!(count(&risk["buckets"]) > 0, "the risk chart has buckets");

    let pretty = format!(
        "{}\n",
        serde_json::to_string_pretty(&frames).expect("frames print")
    );
    if std::env::var_os("POLYSIM_BLESS_BRIDGE").is_some() {
        std::fs::write(FIXTURE_PATH, &pretty).expect("the fixture is writable");
        return;
    }
    let expected: Value = serde_json::from_str(FIXTURE).expect("the checked-in frames are json");
    assert_eq!(
        frames, expected,
        "the protocol frames drifted from bridge_scene.json; if intended, rerun with POLYSIM_BLESS_BRIDGE=1"
    );
}

/// A command that parses but never reaches the model, a section resent every tick, a selection that
/// keeps the old instrument's buckets, or an axis left empty until the page sizes its plots would
/// each leave the page showing the wrong thing with no error anywhere.
#[test]
fn a_view_command_changes_the_next_frame() {
    let mut model = scene();
    let unsized_page = Encoder::new().lines(&tick_input(&model));
    let tick = parse(unsized_page.last().expect("the scene sends a tick"));
    assert!(
        count(&section(&tick, "chart")["axis"]) > 0,
        "the mid axis has ticks before the page sends chart_ticks"
    );
    assert!(
        count(&section(&tick, "risk")["axis"]["exposure"]) > 0,
        "the risk axis has ticks before the page sends chart_ticks"
    );

    let mut encoder = Encoder::new();
    let first = first_frames(&mut model, &mut encoder);
    let generation = parse(&first[1])["chart"]["generation"].as_u64();

    assert!(
        encoder.lines(&tick_input(&model)).is_empty(),
        "nothing changed, so nothing is sent"
    );

    apply(
        &mut model,
        &mut encoder,
        r#"{"cmd":"dom_levels","levels":24}"#,
    );
    let lines = encoder.lines(&tick_input(&model));
    assert_eq!(lines.len(), 1, "one tick and no catalog: {lines:?}");
    let tick = parse(&lines[0]);
    assert_eq!(section(&tick, "view")["domLevels"], 24);
    assert_eq!(section(&tick, "ladder")["rows"], 24);
    for unchanged in ["account", "latency", "features", "channels"] {
        assert!(tick.get(unchanged).is_none(), "{unchanged} did not change");
    }

    apply(
        &mut model,
        &mut encoder,
        r#"{"cmd":"select","instrument":1}"#,
    );
    let lines = encoder.lines(&tick_input(&model));
    assert_eq!(lines.len(), 1, "one tick and no catalog: {lines:?}");
    let tick = parse(&lines[0]);
    assert_eq!(section(&tick, "view")["instrument"], 1);
    assert_eq!(
        section(&tick, "chart")["generation"].as_u64(),
        generation.map(|generation| generation + 1),
        "a new instrument starts a new generation"
    );
    assert_eq!(
        section(&tick, "channels")["system"]["full"],
        true,
        "the page replaces its rings instead of patching them"
    );
}

/// A fill can reach the bridge before the book that opens its bucket. Once the chart window is full
/// that bucket lies past the window's right edge, and a fill skipped then would never reach the
/// page, while the window would draw it a moment later.
#[test]
fn a_fill_ahead_of_its_bucket_reaches_the_page() {
    let mut model = UiModel::with_capacity(2, SPIN);
    model.set_catalog(catalog());
    let book = |second: i64| {
        let bid = tick_price(11_800_000);
        book_snapshot(BookFixture {
            instrument: 0,
            seq: second as u64,
            event_ts: at_second(second).micros(),
            state: UiBookState::Valid,
            bids: &[(bid, quantity(1.0))],
            asks: &[(bid + tick_price(2), quantity(1.0))],
        })
    };
    let capacity = model.chart().capacity() as i64;
    for second in 0..capacity {
        model.apply_book(book(second));
    }
    let mut encoder = Encoder::new();
    encoder.lines(&tick_input(&model));

    model.apply_event(UiEvent::Fill {
        instrument: InstrumentId(0),
        seq: 0,
        event_ts_us: at_second(capacity),
        quote_level: None,
        side: Side::Buy,
        price: Price(tick_price(11_800_000)),
        qty: Qty(quantity(0.05)),
        commission: 0,
        commission_asset: AssetId(9),
        liquidity: None,
    });
    encoder.lines(&tick_input(&model));
    model.apply_book(book(capacity));
    let lines = encoder.lines(&tick_input(&model));

    let tick = parse(lines.last().expect("the new bucket sends a tick"));
    let fills = section(&tick, "chart")["fills"].clone();
    assert_eq!(
        fills.as_array().map(|fills| fills.len()),
        Some(1),
        "the fill arrives with its bucket: {fills}"
    );
    assert_eq!(fills[0]["i"], capacity);
}
