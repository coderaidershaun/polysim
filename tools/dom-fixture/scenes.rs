//! Deterministic books: ONLY place fabricated data lives.

use polysim::desktop::dom_view::{DomGrouping, FeedStatus};
use polysim::desktop::exec_model::{OrderCell, OrderStatus};
use polysim::desktop::format::price_decimals;
use polysim::ids::{ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty};
use polysim::msg::inbound::Level;
use polysim::msg::ui::{DomQuote, UI_BOOK_LEVELS, UiBookSnapshot, UiBookState};
use polysim::time::DurationUs;
use polysim::time::TsUs;

const TICK: Price = Price(FIXED_SCALE);
const QTY_SCALE: i64 = FIXED_SCALE;
const MILLI: i64 = FIXED_SCALE / 1000;
const QUOTE_QTY_MILLI: i64 = 9_000;

/// Grouped: 0.01 tick, 1 bps = 1180 ticks (11.80/row).
const GROUPED_TICK: Price = Price(FIXED_SCALE / 100);
const GROUPED_MID_TICK: i64 = 11_800_000;
const GROUPED_QUOTE_QTY_MILLI: i64 = 250;

pub struct Variant {
    pub label: &'static str,
    pub snapshot: Option<UiBookSnapshot>,
    pub quote: Option<DomQuote>,
    pub bid_orders: Vec<OrderCell>,
    pub ask_orders: Vec<OrderCell>,
    pub tick: Price,
    pub grouping: DomGrouping,
    pub price_decimals: usize,
    pub qty_scale: i64,
    pub qty_decimals: usize,
    pub feed: FeedStatus,
    pub stale_age: Option<DurationUs>,
}

pub struct Scene {
    pub name: &'static str,
    pub variants: Vec<Variant>,
}

pub fn scenes() -> Vec<Scene> {
    vec![
        scene("dense valid book", vec![dense()]),
        scene("sparse — empty ticks", vec![sparse(), sparse_grouped()]),
        scene(
            "half-tick vs integer mid",
            vec![half_tick_mid(), integer_mid()],
        ),
        scene("quotes off-screen", vec![off_screen()]),
        scene(
            "real orders vs desire",
            vec![working_orders(), order_off_screen()],
        ),
        scene("stale", vec![stale()]),
        scene("awaiting book", vec![awaiting()]),
        scene("disconnected", vec![disconnected()]),
        scene("edge values", vec![one_sided(), edge_values()]),
        scene("locked book", vec![locked()]),
        scene(
            "grouped + bps",
            vec![
                grouped("ticks x1 (0.01/row)", DomGrouping::default()),
                grouped(
                    "ticks x10 (0.10/row)",
                    DomGrouping::Ticks { per_bucket: 10 },
                ),
                grouped(
                    "ticks x100 (1.00/row)",
                    DomGrouping::Ticks { per_bucket: 100 },
                ),
                grouped(
                    "bps 1 (11.80/row)",
                    DomGrouping::Bps {
                        numerator: 1,
                        denominator: 1,
                    },
                ),
                grouped(
                    "bps 0.25 (2.95/row)",
                    DomGrouping::Bps {
                        numerator: 1,
                        denominator: 4,
                    },
                ),
                grouped(
                    "bps 0.05 (0.59/row)",
                    DomGrouping::Bps {
                        numerator: 1,
                        denominator: 20,
                    },
                ),
                grouped(
                    "bps 0.01 (0.11/row)",
                    DomGrouping::Bps {
                        numerator: 1,
                        denominator: 100,
                    },
                ),
            ],
        ),
    ]
}

fn dense() -> Variant {
    let asks = [
        (65992, 512),
        (65993, 1_240),
        (65994, 905),
        (65995, 2_100),
        (65996, 333),
        (65997, 1_010),
        (65998, 4_567),
        (65999, 88),
        (66000, 12_345),
        (66001, 750),
        (66002, 3_200),
        (66003, 410),
    ];
    let bids = [
        (65990, 640),
        (65989, 1_115),
        (65988, 900),
        (65987, 2_480),
        (65986, 170),
        (65985, 5_005),
        (65984, 260),
        (65983, 1_900),
        (65982, 55),
        (65981, 8_800),
        (65980, 330),
        (65979, 1_450),
    ];
    Variant {
        label: "bid delta 2 | ask delta 3 | qty 9",
        quote: Some(both_quotes(65989, 65994)),
        ..valid(&levels(&bids), &levels(&asks))
    }
}

fn sparse() -> Variant {
    let asks = levels(&[(65992, 500), (65994, 1_250), (65997, 800), (66001, 2_000)]);
    let bids = levels(&[(65990, 600), (65988, 1_100), (65985, 900), (65980, 3_300)]);
    Variant {
        label: "gaps keep grid + price",
        // The ask quote rests on an otherwise-empty tick: strategy-only row, blank public cell.
        quote: Some(DomQuote::top(None, Some((px(65996), qm(QUOTE_QTY_MILLI))))),
        ..valid(&bids, &asks)
    }
}

/// The same gappy book at five ticks a row: the blank ticks between levels fold away, the quote's
/// lone tick joins the depth around it, and — the one that needs signing off — both sides' nearest
/// rows are the same mid-straddling bucket, so 65990 labels a row above AND below the separator.
fn sparse_grouped() -> Variant {
    Variant {
        label: "ticks x5 - gaps merge, mid bucket shared",
        grouping: DomGrouping::Ticks { per_bucket: 5 },
        ..sparse()
    }
}

fn half_tick_mid() -> Variant {
    let asks = levels(&[(65992, 700), (65993, 1_400), (65995, 300), (65998, 2_600)]);
    let bids = levels(&[(65991, 800), (65990, 1_200), (65988, 500), (65985, 3_100)]);
    Variant {
        label: "mid ...991.5",
        quote: Some(both_quotes(65989, 65994)),
        ..valid(&bids, &asks)
    }
}

fn integer_mid() -> Variant {
    let asks = levels(&[(65992, 700), (65993, 1_400), (65995, 300), (65998, 2_600)]);
    let bids = levels(&[(65990, 800), (65989, 1_200), (65987, 500), (65984, 3_100)]);
    Variant {
        label: "mid 65991",
        quote: Some(both_quotes(65989, 65994)),
        ..valid(&bids, &asks)
    }
}

fn off_screen() -> Variant {
    let asks = levels(&[
        (65992, 512),
        (65993, 1_240),
        (65994, 905),
        (65995, 2_100),
        (65996, 333),
        (65997, 1_010),
    ]);
    let bids = levels(&[
        (65990, 640),
        (65989, 1_115),
        (65988, 900),
        (65987, 2_480),
        (65986, 170),
        (65985, 5_005),
    ]);
    Variant {
        label: "ask above / bid below the window",
        quote: Some(DomQuote::top(
            Some((px(65950), qm(QUOTE_QTY_MILLI))),
            Some((px(66030), qm(QUOTE_QTY_MILLI))),
        )),
        ..valid(&bids, &asks)
    }
}

fn stale() -> Variant {
    Variant {
        label: "amber, exact age",
        feed: FeedStatus::Stale,
        stale_age: Some(DurationUs::from_millis(1_234)),
        ..dense()
    }
}

fn awaiting() -> Variant {
    let asks = levels(&[(65992, 500), (65993, 400)]);
    let bids = levels(&[(65990, 600), (65989, 700)]);
    Variant {
        label: "book building — no mid",
        quote: None,
        ..variant(Some(book(&bids, &asks, UiBookState::AwaitingSnapshot)))
    }
}

fn disconnected() -> Variant {
    Variant {
        label: "last book dimmed + warning",
        feed: FeedStatus::Disconnected,
        ..dense()
    }
}

fn one_sided() -> Variant {
    let bids = levels(&[(65990, 640), (65989, 1_115), (65988, 900)]);
    Variant {
        label: "bids only -> mid --, no rows",
        ..valid(&bids, &[])
    }
}

fn edge_values() -> Variant {
    let asks = levels(&[(65992, 0), (65994, 99_999_999), (65995, 1_000)]);
    let bids = levels(&[(65990, 12_345_678), (65989, 0), (65987, 1)]);
    Variant {
        label: "0 vs blank | max width",
        ..valid(&bids, &asks)
    }
}

fn locked() -> Variant {
    let asks = levels(&[(65991, 1_000), (65992, 512), (65993, 1_240)]);
    let bids = levels(&[(65991, 1_100), (65990, 640), (65989, 1_115)]);
    Variant {
        label: "bb == ba -> price in separator",
        ..valid(&bids, &asks)
    }
}

/// BTC book: grouping compare alone. Ungrouped: 12 rows 0.12 off-screen. x10 back. 1 bps carries depth.
fn grouped(label: &'static str, grouping: DomGrouping) -> Variant {
    let asks = grouped_levels(&[
        (1, 85),
        (2, 140),
        (3, 62),
        (5, 310),
        (8, 95),
        (12, 480),
        (17, 120),
        (23, 755),
        (30, 210),
        (41, 1_640),
        (55, 305),
        (74, 890),
        (100, 2_400),
        (150, 640),
        (215, 1_180),
        (300, 3_050),
        (420, 720),
        (600, 2_260),
        (850, 1_430),
        (1_200, 4_800),
        (2_000, 2_150),
        (3_500, 6_400),
        (5_000, 3_900),
        (8_000, 11_500),
        (12_000, 7_250),
    ]);
    let bids = grouped_levels(&[
        (-1, 92),
        (-2, 118),
        (-4, 240),
        (-6, 74),
        (-9, 530),
        (-13, 165),
        (-18, 900),
        (-25, 205),
        (-33, 1_450),
        (-46, 380),
        (-62, 1_020),
        (-85, 275),
        (-110, 2_800),
        (-160, 515),
        (-230, 1_340),
        (-320, 2_700),
        (-450, 830),
        (-640, 1_960),
        (-900, 1_275),
        (-1_300, 5_200),
        (-2_100, 2_480),
        (-3_600, 5_900),
        (-5_200, 4_150),
        (-8_500, 10_200),
        (-12_500, 6_800),
    ]);
    Variant {
        label,
        tick: GROUPED_TICK,
        price_decimals: price_decimals(GROUPED_TICK),
        grouping,
        quote: Some(DomQuote::top(
            Some((grouped_px(-6), qm(GROUPED_QUOTE_QTY_MILLI))),
            Some((grouped_px(30), qm(GROUPED_QUOTE_QTY_MILLI))),
        )),
        ..valid(&bids, &asks)
    }
}

fn scene(name: &'static str, variants: Vec<Variant>) -> Scene {
    Scene { name, variants }
}

/// Confirmed bid, in-flight ask, desired quote: distinguish live/sent/want.
fn working_orders() -> Variant {
    let asks = levels(&[(65992, 500), (65993, 1_250), (65994, 800), (65996, 2_000)]);
    let bids = levels(&[(65990, 600), (65989, 1_100), (65988, 900), (65986, 3_300)]);
    Variant {
        label: "live bid | sent ask | want beyond",
        quote: Some(DomQuote::top(
            Some((px(65987), qm(QUOTE_QTY_MILLI))),
            Some((px(65995), qm(QUOTE_QTY_MILLI))),
        )),
        bid_orders: vec![order(1, 65989, QUOTE_QTY_MILLI, OrderStatus::Confirmed)],
        ask_orders: vec![order(2, 65993, QUOTE_QTY_MILLI, OrderStatus::InFlight)],
        ..valid(&bids, &asks)
    }
}

/// Lost off-screen: chevron shows ORDER (live exposure not intention).
fn order_off_screen() -> Variant {
    Variant {
        label: "lost order off-screen outranks want",
        bid_orders: vec![order(3, 65950, QUOTE_QTY_MILLI, OrderStatus::Lost)],
        ..working_orders()
    }
}

/// Venue-acknowledged order, distinct from quote.
fn order(id: u64, tick: i64, milli: i64, status: OrderStatus) -> OrderCell {
    OrderCell {
        client_id: ClientOrderId(id),
        status,
        price: px(tick),
        qty: qm(milli),
        filled: Qty(0),
        at: TsUs::from_micros(0),
        quote_level: None,
    }
}

fn both_quotes(bid_tick: i64, ask_tick: i64) -> DomQuote {
    DomQuote::top(
        Some((px(bid_tick), qm(QUOTE_QTY_MILLI))),
        Some((px(ask_tick), qm(QUOTE_QTY_MILLI))),
    )
}

/// Valid book, 3-decimal qty.
fn valid(bids: &[Level], asks: &[Level]) -> Variant {
    variant(Some(book(bids, asks, UiBookState::Valid)))
}

fn variant(snapshot: Option<UiBookSnapshot>) -> Variant {
    Variant {
        label: "",
        snapshot,
        quote: None,
        bid_orders: Vec::new(),
        ask_orders: Vec::new(),
        tick: TICK,
        grouping: DomGrouping::default(),
        price_decimals: price_decimals(TICK),
        qty_scale: QTY_SCALE,
        qty_decimals: 3,
        feed: FeedStatus::default(),
        stale_age: None,
    }
}

fn book(bids: &[Level], asks: &[Level], state: UiBookState) -> UiBookSnapshot {
    let empty = Level {
        price: Price(0),
        qty: Qty(0),
    };
    let mut bid_levels = [empty; UI_BOOK_LEVELS];
    let mut ask_levels = [empty; UI_BOOK_LEVELS];
    for (slot, level) in bid_levels.iter_mut().zip(bids) {
        *slot = *level;
    }
    for (slot, level) in ask_levels.iter_mut().zip(asks) {
        *slot = *level;
    }
    UiBookSnapshot {
        instrument: InstrumentId(0),
        seq: 1,
        event_ts_us: TsUs::from_micros(0),
        state,
        bid_len: bids.len().min(UI_BOOK_LEVELS) as u16,
        ask_len: asks.len().min(UI_BOOK_LEVELS) as u16,
        bids: bid_levels,
        asks: ask_levels,
    }
}

fn levels(rows: &[(i64, i64)]) -> Vec<Level> {
    rows.iter().map(|&(tick, milli)| lvl(tick, milli)).collect()
}

fn lvl(tick_index: i64, milli: i64) -> Level {
    Level {
        price: px(tick_index),
        qty: qm(milli),
    }
}

fn px(tick_index: i64) -> Price {
    Price(tick_index * FIXED_SCALE)
}

/// Levels on the grouped scene's 0.01 grid, written as signed tick offsets from its mid so a row
/// reads as its distance from the touch rather than as an eight-digit tick index.
fn grouped_levels(offsets: &[(i64, i64)]) -> Vec<Level> {
    offsets
        .iter()
        .map(|&(offset, milli)| Level {
            price: grouped_px(offset),
            qty: qm(milli),
        })
        .collect()
}

fn grouped_px(offset_ticks: i64) -> Price {
    Price((GROUPED_MID_TICK + offset_ticks) * GROUPED_TICK.0)
}

fn qm(milli: i64) -> Qty {
    Qty(milli * MILLI)
}
