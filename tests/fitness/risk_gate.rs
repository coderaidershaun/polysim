//! Risk-gate fitness: the exposure ceiling must never trap a position it cannot unwind.
//!
//! Exposure became mark-to-market on 2026-07-26, and that quietly broke a property the superseded
//! fixed-step ledger held by construction. `|exposure|` can now pass the ceiling on a price move
//! with NO fill at all — the old ledger moved only in whole Δ steps the gate itself refused to take
//! past the ceiling, so it could not happen. A gate that withdrew both sides there would deadlock:
//! nothing quoted means nothing fills, nothing filling means the position cannot come down, and only
//! a favourable mark would release it.
//!
//! The rest of the suite cannot see this. Every test that drives the recorder lives in the
//! flat-price limit, where mark-to-market exposure and the fixed-step ledger agree exactly — which
//! is why the migration passed every pinned test without one of them being edited. This file is the
//! first thing that moves the mark independently of the prices fills got.
//!
//! The ENGINE grew its own ceiling on 2026-07-27 (`gates::assess_exposure`), and the second half of
//! this file pins it. Two layers now hold the same line for different reasons: the strategy's shapes
//! the quotes it declares, the engine's is the one a strategy bug cannot bypass — so both are here,
//! and the deadlock property above binds both. The engine's is the better number, because it holds
//! the price and size of the order about to be placed rather than a fixed per-fill step.
//!
//! The last case here is the engine's OTHER control a strategy cannot bypass — the consecutive
//! hard-reject kill switch — pinned on the one thing that can silently disarm it: an event the
//! engine synthesised for itself being counted as the venue accepting something.

use polysim::adapters::exec::open_orders_snapshot_end;
use polysim::config::{RecordedTables, TrackerSpec};
use polysim::exposure::InstrumentExposure;
use polysim::hot::dispatch::{ExecWiring, ExposureWiring, HotEngine, HotEngineSetup};
use polysim::hot::exec::{
    ClientIdLayout, DesiredQuote, ExecLimits, ExecSettings, ExposureCheck, FeeModel, OrderBudget,
    QuoteLevel, QuotePermission, assess_exposure, side_base,
};
use polysim::hot::strategy::{Strategy, StrategyCtx};
use polysim::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{
    AccountChunk, AccountChunkKind, AssetBalance, CancelReason, ExecCommand, ExecEvent, ExecKind,
    ExecLaneItem, OrderStyle, RejectClass, VenueOrderStatus,
};
use polysim::msg::inbound::{InboundMessage, SpinTick};
use polysim::msg::ui::UiEvent;
use polysim::registry::InstrumentRow;
use polysim::sink::ExecSink;
use polysim::time::DurationUs;
use proptest::prelude::*;
use rtrb::{Consumer, RingBuffer};

use crate::engine_support::{
    ALL_TABLES, ONE, book_reset, exec_event, exposure_ring, instrument_row, metrics_ring,
    persist_ring_for, pop, snapshot_pair, spin, strategy_log_ring, ts, ui_book_ring, ui_event_ring,
};
use crate::micro_strategy::models::{RiskBudget, signed};

/// Drop the book and restate it whole, so the mark moves with no transient crossing to reason about.
pub(crate) fn reseat_book(bid: i64, ask: i64, when: i64) -> [InboundMessage; 3] {
    let (bids, asks) = snapshot_pair(0, &[(bid, ONE)], &[(ask, ONE)], when);
    [
        InboundMessage::BookReset(book_reset(0, when)),
        InboundMessage::Book(bids),
        InboundMessage::Book(asks),
    ]
}

proptest! {
    /// FITNESS: whatever the exposure, at least one side is always quotable. This is the deadlock
    /// stated directly rather than through one scenario — a gate that can withdraw both sides at
    /// once has no way back to flat, and no fill can arrive to prove it wrong.
    #[test]
    fn some_side_is_always_quotable(
        exposure_units in -500i64..=500,
        // The startup assert in `InstrumentState::new` refuses a run where Δ exceeds the ceiling, so
        // the ranges are disjoint rather than filtered — a rejected case would be one the engine
        // never boots into.
        order_units in 1i64..=50,
        max_units in 50i64..=200,
    ) {
        let budget = RiskBudget {
            order_notional: order_units * ONE,
            max_exposure_quote: max_units * ONE,
        };
        let exposure = exposure_units * ONE;
        prop_assert!(
            !budget.would_breach(exposure, Side::Buy) || !budget.would_breach(exposure, Side::Sell),
            "both sides withdrawn at exposure {} with Δ {} and ceiling {} — the position is trapped",
            exposure,
            budget.order_notional,
            budget.max_exposure_quote,
        );
    }

    /// FITNESS: a side that strictly shrinks the position is never withdrawn, at any exposure. The
    /// ceiling exists to stop the position growing, so it must never be what stops it shrinking.
    #[test]
    fn a_side_that_shrinks_the_position_is_never_withdrawn(
        exposure_units in -500i64..=500,
        order_units in 1i64..=50,
        max_units in 50i64..=200,
    ) {
        let budget = RiskBudget {
            order_notional: order_units * ONE,
            max_exposure_quote: max_units * ONE,
        };
        let exposure = exposure_units * ONE;
        for side in [Side::Buy, Side::Sell] {
            let projected = exposure + signed(side, budget.order_notional);
            if projected.abs() < exposure.abs() {
                prop_assert!(
                    !budget.would_breach(exposure, side),
                    "{:?} takes exposure from {} to {} — strictly smaller, yet withdrawn",
                    side,
                    exposure,
                    projected,
                );
            }
        }
    }
}

pub(crate) const INSTRUMENT: InstrumentId = InstrumentId(0);
const BASE_ASSET: AssetId = AssetId(0);
const QUOTE_ASSET: AssetId = AssetId(1);

/// Small enough that ONE whole base unit marked at the prices below sits exactly on it, so every
/// scenario reaches the boundary rather than a comfortable interior.
pub(crate) const CEILING: i64 = 100 * ONE;

/// The declared size, and the number the "real notional, not a fixed step" pin turns on.
pub(crate) const QUOTE_QTY: Qty = Qty(ONE / 10);

pub(crate) const BID: i64 = 99 * ONE;
pub(crate) const ASK: i64 = 101 * ONE;
/// The mid of the book above, which is what the ledger marks against.
const MARK: i64 = 100 * ONE;

fn ceiling_check(side: Side, position_base: i64, has_mark: bool) -> ExposureCheck {
    ExposureCheck {
        exposure_quote: if has_mark { Price(MARK).notional(Qty(position_base)) } else { 0 },
        position_base: Qty(position_base),
        has_mark,
        side,
        price: Price(match side {
            Side::Buy => BID,
            Side::Sell => ASK,
        }),
        qty: QUOTE_QTY,
        ceiling_quote: CEILING,
    }
}

/// FITNESS: `assess_exposure` verdicts at the ceiling match a named table. From over the ceiling the
/// engine withdraws the side that would ADD to the position and keeps the side that would reduce it —
/// the reducing side admitted by construction rather than by arithmetic, so no rounding or sign error
/// can reach it — and an UNMARKED position is not a flat one: the ledger's mark is `None` until the
/// first two-sided committed book, exposure reads 0 in that window too, and only `has_mark` tells the
/// two zeroes apart, so a restarted engine holding unvalued inventory must refuse to grow it rather
/// than read itself as flat.
#[test]
fn assess_exposure_ceiling_verdicts_match_named_cases() {
    struct Case {
        name: &'static str,
        side: Side,
        position_base: i64,
        has_mark: bool,
        exposure_override: Option<i64>,
        expected: QuotePermission,
    }
    let cases = [
        Case {
            name: "long_at_the_ceiling_buy_is_withdrawn",
            side: Side::Buy,
            position_base: ONE,
            has_mark: true,
            exposure_override: None,
            expected: QuotePermission::ReducingOnly {
                reducing: Side::Sell,
            },
        },
        Case {
            name: "long_at_the_ceiling_sell_is_the_way_back",
            side: Side::Sell,
            position_base: ONE,
            has_mark: true,
            exposure_override: None,
            expected: QuotePermission::Both,
        },
        // Twice the ceiling — the regime this file was written for, since a price move alone can
        // carry exposure there with no fill at all. The unwinding side is not merely permitted, it
        // is unobjectionable: a gate answering "restricted, but not on this side" from deep over the
        // budget is reasoning about the position when it was asked about the way out of it.
        Case {
            name: "twice_the_ceiling_the_unwinding_side_is_unobjectionable",
            side: Side::Sell,
            position_base: 2 * ONE,
            has_mark: true,
            exposure_override: None,
            expected: QuotePermission::Both,
        },
        Case {
            name: "short_at_the_ceiling_sell_is_withdrawn",
            side: Side::Sell,
            position_base: -ONE,
            has_mark: true,
            exposure_override: None,
            expected: QuotePermission::ReducingOnly {
                reducing: Side::Buy,
            },
        },
        Case {
            name: "short_at_the_ceiling_buy_is_the_way_back",
            side: Side::Buy,
            position_base: -ONE,
            has_mark: true,
            exposure_override: None,
            expected: QuotePermission::Both,
        },
        Case {
            name: "an_unmarked_long_refuses_the_growing_side",
            side: Side::Buy,
            position_base: ONE,
            has_mark: false,
            exposure_override: None,
            expected: QuotePermission::ReducingOnly {
                reducing: Side::Sell,
            },
        },
        Case {
            name: "an_unmarked_long_stays_reducible",
            side: Side::Sell,
            position_base: ONE,
            has_mark: false,
            exposure_override: None,
            expected: QuotePermission::Both,
        },
        // Without this case the branch reading has_mark could be deleted and every other assertion
        // in this table would still pass: a mark of zero is an honest valuation, distinct from no
        // mark at all, and threatens the ceiling not at all.
        Case {
            name: "a_mark_of_zero_is_an_honest_valuation",
            side: Side::Buy,
            position_base: ONE,
            has_mark: true,
            exposure_override: Some(0),
            expected: QuotePermission::Both,
        },
    ];
    for case in cases {
        let mut check = ceiling_check(case.side, case.position_base, case.has_mark);
        if let Some(exposure_quote) = case.exposure_override {
            check.exposure_quote = exposure_quote;
        }
        let got = assess_exposure(check);
        assert_eq!(
            got, case.expected,
            "case {}: got {:?}, want {:?}",
            case.name, got, case.expected
        );
    }
}

/// FITNESS: the projection is the order's REAL notional. The strategy must guess with a fixed
/// per-fill step because it has no price for a fill that has not happened; the engine holds the
/// price and size about to be sent, and two quotes that differ only in size must be judged
/// differently. A gate stepping by a constant cannot tell these two apart.
#[test]
fn the_ceiling_projects_the_declared_notional_rather_than_a_fixed_step() {
    // Nine tenths of the way to the ceiling, so the two sizes below straddle it.
    let position = ONE * 9 / 10;
    let small = ExposureCheck {
        qty: Qty(ONE / 10),
        ..ceiling_check(Side::Buy, position, true)
    };
    assert_eq!(
        small.exposure_quote + Price(BID).notional(small.qty),
        CEILING - ONE / 10,
        "the small quote lands just inside the ceiling"
    );
    assert_eq!(
        assess_exposure(small),
        QuotePermission::Both,
        "so it is admitted"
    );

    let large = ExposureCheck {
        qty: Qty(ONE / 5),
        ..small
    };
    assert!(
        large.exposure_quote + Price(BID).notional(large.qty) > CEILING,
        "twice the size clears the ceiling — the scenario needs both sides of it"
    );
    assert_eq!(
        assess_exposure(large),
        QuotePermission::ReducingOnly {
            reducing: Side::Sell
        },
        "the same position and the same ceiling, refused only because the ORDER is bigger"
    );
}

proptest! {
    /// FITNESS: the engine's ceiling never withdraws both sides, whatever the position, the mark,
    /// the order size or the budget. Stated over the gate itself rather than through one scenario:
    /// a control that can withdraw both sides at once has no way back to flat.
    #[test]
    fn the_engine_ceiling_always_leaves_a_side_quotable(
        position_units in -50i64..=50,
        mark_units in 1i64..=500,
        qty_hundredths in 0i64..=500,
        ceiling_units in 1i64..=500,
        has_mark in any::<bool>(),
    ) {
        let position_base = Qty(position_units * ONE);
        let mark = Price(mark_units * ONE);
        let check = |side: Side| ExposureCheck {
            exposure_quote: if has_mark { mark.notional(position_base) } else { 0 },
            position_base,
            has_mark,
            side,
            price: mark,
            qty: Qty(qty_hundredths * ONE / 100),
            ceiling_quote: ceiling_units * ONE,
        };
        let buy = assess_exposure(check(Side::Buy));
        let sell = assess_exposure(check(Side::Sell));
        prop_assert!(
            buy.admits(Side::Buy) || sell.admits(Side::Sell),
            "both sides withdrawn holding {} at mark {} against ceiling {} — the position is trapped",
            position_base.0,
            mark.0,
            ceiling_units * ONE,
        );
    }

    /// FITNESS: whatever the numbers, the side that shrinks a held position is admitted. The
    /// companion to the property above and the stronger statement — it names WHICH side survives,
    /// so a gate that satisfied "some side quotes" by keeping the wrong one still fails here.
    #[test]
    fn the_engine_ceiling_never_withdraws_the_reducing_side(
        position_units in -50i64..=50,
        mark_units in 1i64..=500,
        qty_hundredths in 0i64..=500,
        ceiling_units in 1i64..=500,
        has_mark in any::<bool>(),
    ) {
        let position_base = Qty(position_units * ONE);
        let mark = Price(mark_units * ONE);
        let reducing = match position_units.signum() {
            1 => Side::Sell,
            -1 => Side::Buy,
            _ => return Ok(()),
        };
        let permission = assess_exposure(ExposureCheck {
            exposure_quote: if has_mark { mark.notional(position_base) } else { 0 },
            position_base,
            has_mark,
            side: reducing,
            price: mark,
            qty: Qty(qty_hundredths * ONE / 100),
            ceiling_quote: ceiling_units * ONE,
        });
        prop_assert!(
            permission.admits(reducing),
            "{:?} shrinks a position of {} yet was withdrawn",
            reducing,
            position_base.0,
        );
    }
}

/// Declares a resting quote on both sides at the touch, every spin — the level-triggered contract
/// `ctx.quote` asks for. What the ENGINE does with those declarations is what the tests below read,
/// and this strategy knows nothing about any ceiling: that is the point, since the gate under test
/// is the one a strategy cannot opt out of.
pub(crate) struct TwoSidedQuoter;

impl Strategy for TwoSidedQuoter {
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        let book = ctx.book(INSTRUMENT);
        let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) else {
            return;
        };
        for (side, price) in [(Side::Buy, bid.price), (Side::Sell, ask.price)] {
            ctx.quote(
                INSTRUMENT,
                side,
                QuoteLevel::ZERO,
                Some(DesiredQuote {
                    price,
                    qty: QUOTE_QTY,
                    style: OrderStyle::PostOnly,
                }),
            );
        }
    }
}

pub(crate) fn row_with_ceiling(ceiling_quote: i64) -> InstrumentRow {
    InstrumentRow {
        max_exposure_quote: ceiling_quote,
        ..instrument_row(0, TrackerSpec::default(), 64)
    }
}

/// Every limit except the ceiling set so wide it cannot be what refuses a quote — a band, a stale
/// book or a funds floor biting instead would read exactly like the gate under test working.
pub(crate) fn exec_settings() -> ExecSettings {
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
        // No silence sweep: a reconciliation request in the command stream would be noise to filter
        // rather than a behaviour these tests are about.
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

/// What the engine builders below vary. A struct because the wrappers differ only in two of these
/// and a positional list of four says nothing at the call site.
pub(crate) struct QuotingSetup<'a> {
    pub(crate) row: InstrumentRow,
    /// What declares. Most callers want [`TwoSidedQuoter`]; the ones testing a declaration this
    /// strategy cannot make bring their own.
    pub(crate) strategy: Box<dyn Strategy>,
    pub(crate) restored: &'a [InstrumentExposure],
    pub(crate) settings: ExecSettings,
    /// Which audit tables are banked. `Orders` is what makes an engine-driven transition — a sweep,
    /// a timeout — observable at all: nothing else in the process records one.
    pub(crate) tables: RecordedTables,
    /// Run nonce encoded in client order IDs.
    pub(crate) run_nonce: u32,
}

/// Every consumer the engine writes into. Handed back whole, and a wrapper that does not read one
/// drops it — which is what a run with no workstation and no persistence does anyway.
pub(crate) struct QuotingEngine {
    pub(crate) engine: HotEngine,
    pub(crate) commands: Consumer<ExecLaneItem>,
    pub(crate) ui_events: Consumer<UiEvent>,
}

pub(crate) fn built_quoting_engine(setup: QuotingSetup<'_>) -> QuotingEngine {
    let instruments = [setup.row];
    let (persistence, _records) = persist_ring_for(1_024, setup.tables);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (ui_book_sink, _ui_books) = ui_book_ring(64);
    let (ui_event_sink, ui_events) = ui_event_ring(256);
    let (commands_producer, commands) = RingBuffer::<ExecLaneItem>::new(256);
    let (exposure_sink, _exposure) = exposure_ring(64);
    let engine = HotEngine::new(HotEngineSetup {
        instruments: &instruments,
        strategy: setup.strategy,
        persistence: Some(persistence),
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
        exec: Some(ExecWiring {
            sink: ExecSink::new(commands_producer),
            settings: setup.settings,
            run_nonce: setup.run_nonce,
        }),
        exposure: ExposureWiring {
            restored: setup.restored,
            sink: exposure_sink,
        },
    });
    QuotingEngine {
        engine,
        commands,
        ui_events,
    }
}

pub(crate) fn quoting_engine_with_ui(
    ceiling_quote: i64,
    restored: &[InstrumentExposure],
) -> (HotEngine, Consumer<ExecLaneItem>, Consumer<UiEvent>) {
    let built = built_quoting_engine(QuotingSetup {
        row: row_with_ceiling(ceiling_quote),
        strategy: Box::new(TwoSidedQuoter),
        restored,
        settings: exec_settings(),
        tables: RecordedTables::new(&ALL_TABLES),
        run_nonce: 0,
    });
    (built.engine, built.commands, built.ui_events)
}

/// The same engine for a caller that reads only the command stream. Dropping the UI consumer is
/// what a run with no workstation attached does anyway — the tee then fails and is counted.
pub(crate) fn quoting_engine(
    ceiling_quote: i64,
    restored: &[InstrumentExposure],
) -> (HotEngine, Consumer<ExecLaneItem>) {
    let (engine, commands, _ui_events) = quoting_engine_with_ui(ceiling_quote, restored);
    (engine, commands)
}

/// The three facts that must hold before ONE order may be sent: the stream is up, the open-order
/// snapshot has landed, and balances are known. Without them nothing quotes and every assertion
/// below would pass for the wrong reason.
pub(crate) fn make_ready(engine: &mut HotEngine, when: i64) {
    stream_and_balances(engine, when);
    open_orders_snapshot(engine, when);
}

/// Two of the three legs. Split out so `exec_resync` can withhold the third and prove it is load
/// bearing — an engine that quotes without it would make the marker's absence undetectable, which
/// is exactly how the closed loop this arms went unnoticed.
pub(crate) fn stream_and_balances(engine: &mut HotEngine, when: i64) {
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::StreamReady,
            ..exec_event(INSTRUMENT, ClientOrderId(0), Side::Buy, 0, when)
        }),
    );
    let mut balances = [AssetBalance {
        asset: AssetId::UNKNOWN,
        free: 0,
        locked: 0,
    }; polysim::msg::exec::ACCOUNT_CHUNK_ASSETS];
    balances[0] = AssetBalance {
        asset: BASE_ASSET,
        free: 10 * ONE,
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

/// The third leg, built by the SAME constructor the Binance actor sends it with. Synthesising the
/// marker here instead would let every readiness-dependent test below stay green while production
/// produced no such event at all — which is precisely what happened.
pub(crate) fn open_orders_snapshot(engine: &mut HotEngine, when: i64) {
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(open_orders_snapshot_end(INSTRUMENT, ts(when))),
    );
}

/// One real fill through the inbound path: adopt the order from a reconciliation snapshot, then
/// report it FULLY filled. Closing it matters — a side still holding a working order is blocked by
/// the in-flight rule, and a test reading "no order placed" there would be reading that rule rather
/// than the ceiling.
fn fill_and_close(engine: &mut HotEngine, side: Side, price: i64, qty: i64, when: i64) {
    let client_id = ClientIdLayout { run_nonce: 0 }.encode(side_base(INSTRUMENT, side) + 1, 1);
    let base = ExecEvent {
        qty: Qty(qty),
        ..exec_event(INSTRUMENT, client_id, side, price, when)
    };
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::SnapshotOrder,
            ..base
        }),
    );
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Exec(ExecEvent {
            kind: ExecKind::ReportTrade,
            status: Some(VenueOrderStatus::Filled),
            last_price: Price(price),
            last_qty: Qty(qty),
            cumulative_qty: Qty(qty),
            cumulative_quote: Price(price).notional(Qty(qty)),
            ..base
        }),
    );
}

pub(crate) fn spin_at(engine: &mut HotEngine, seq: u64, when: i64) {
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(seq, when)));
}

pub(crate) fn drain_commands(commands: &mut Consumer<ExecLaneItem>) -> Vec<ExecCommand> {
    let mut drained = Vec::new();
    while let Ok(item) = commands.pop() {
        if let ExecLaneItem::Command(stamped) = item {
            drained.push(stamped.command);
        }
    }
    drained
}

/// Whether the engine asked the venue for a NEW order on this side — a placement either on its own
/// or as the second leg of a replace, since both put a quote on the book.
pub(crate) fn is_placing(commands: &[ExecCommand], wanted: Side) -> bool {
    commands.iter().any(|command| match command {
        ExecCommand::Place { side, .. } => *side == wanted,
        _ => false,
    })
}

/// The message sequence both engine-driven tests below share: come ready, take a long position by
/// the route the caller chose, then see the book that values it at exactly the ceiling.
fn quote_holding_one_unit(
    ceiling_quote: i64,
    restored: &[InstrumentExposure],
    fill: Option<Side>,
) -> Vec<ExecCommand> {
    let (mut engine, mut commands) = quoting_engine(ceiling_quote, restored);
    make_ready(&mut engine, 0);
    if let Some(side) = fill {
        fill_and_close(&mut engine, side, MARK, ONE, 5);
    }
    for message in reseat_book(BID, ASK, 10) {
        engine.dispatch(pop(0, 0), &message);
    }
    spin_at(&mut engine, 1, 20);
    drain_commands(&mut commands)
}

/// FITNESS: a position onto the ceiling withdraws the adding side at the ENGINE, whatever the
/// strategy keeps declaring — and leaves the reducing side quoting — whether the position arrived
/// through a real fill this run or was inherited whole from a restart.
///
/// The fill case is driven twice over one message sequence, differing only in the row's
/// `max_exposure_quote`. The loose run is what makes the tight one mean something: it proves this
/// fixture really does reach the quoting path, so "no order on the buy side" is the ceiling refusing
/// and not the fixture failing to ask. The restored case then proves the same gate reads a
/// cross-session ledger rather than starting the run believing itself flat.
#[test]
fn a_position_at_the_ceiling_stops_the_engine_adding_whether_filled_or_restored() {
    let tight = quote_holding_one_unit(CEILING, &[], Some(Side::Buy));
    assert!(
        !is_placing(&tight, Side::Buy),
        "the position is marked at the ceiling, so nothing may grow it: {tight:?}"
    );
    assert!(
        is_placing(&tight, Side::Sell),
        "and the side that unwinds it must still quote — no quote means no fill, and no fill means \
         no way down: {tight:?}"
    );

    let loose = quote_holding_one_unit(1_000_000 * ONE, &[], Some(Side::Buy));
    assert!(
        is_placing(&loose, Side::Buy),
        "the same sequence under a ceiling far away must quote both sides, or the test above is \
         reading a fixture that never asked for an order: {loose:?}"
    );
    assert!(
        is_placing(&loose, Side::Sell),
        "and the other side: {loose:?}"
    );

    // An engine that BOOTS holding inventory is held to the same ceiling. The ledger is
    // cross-session, so a restart inherits a position no fill this run produced — and the gate must
    // read it off the restored cost basis rather than starting the run believing itself flat.
    let restored = [InstrumentExposure {
        instrument: INSTRUMENT,
        position_base: Qty(ONE),
        cash_quote: -MARK,
        basis_quote: MARK,
    }];
    let inherited = quote_holding_one_unit(CEILING, &restored, None);
    assert!(
        !is_placing(&inherited, Side::Buy),
        "the inherited position is already at the ceiling: {inherited:?}"
    );
    assert!(
        is_placing(&inherited, Side::Sell),
        "and unwinding it is exactly what a restart holding inventory needs to do: {inherited:?}"
    );
}

/// FITNESS: only the VENUE can end a hard-reject streak, so the engine's own refusals never postpone
/// the kill switch.
///
/// The engine synthesises events for requests that never left the process, and one of them leaves
/// its order reading `Live` — an amend the edge would not send, which changes nothing about an order
/// still resting. Nobody at the venue accepted anything there. Reading it as acceptance resets the
/// streak, and the arrangement that produces those refusals is a dead socket, which is exactly when
/// the venue is most likely to be rejecting everything else too.
#[test]
fn an_edge_refusal_never_ends_a_venue_reject_streak() {
    let mut halting = engine_holding_one_live_order();
    for reject in 0..REJECT_STREAK {
        dispatch_exec(&mut halting.engine, hard_reject(100 + i64::from(reject)));
    }
    spin_at(&mut halting.engine, 1, 200);
    assert!(
        has_halted(&mut halting.commands),
        "the streak the kill switch is configured for did not halt, so the case below proves nothing"
    );

    let mut interrupted = engine_holding_one_live_order();
    for reject in 0..REJECT_STREAK - 1 {
        dispatch_exec(
            &mut interrupted.engine,
            hard_reject(100 + i64::from(reject)),
        );
    }
    dispatch_exec(
        &mut interrupted.engine,
        ExecEvent {
            kind: ExecKind::AmendNotSent,
            ..live_order_event(150)
        },
    );
    dispatch_exec(&mut interrupted.engine, hard_reject(160));
    spin_at(&mut interrupted.engine, 1, 200);
    assert!(
        has_halted(&mut interrupted.commands),
        "an amend the engine refused itself reset the venue's reject count, and the kill switch \
         never fired"
    );
}

/// The configured streak in [`exec_settings`], named so the case above cannot drift from it.
const REJECT_STREAK: u32 = 5;

/// A ready engine holding one order the venue's answers can land on. Rejects fold onto a slot or
/// nowhere at all, so without a seated order every answer below would be counted as an orphan.
fn engine_holding_one_live_order() -> QuotingEngine {
    let mut built = built_quoting_engine(QuotingSetup {
        row: row_with_ceiling(1_000_000 * ONE),
        strategy: Box::new(TwoSidedQuoter),
        restored: &[],
        settings: exec_settings(),
        tables: RecordedTables::new(&ALL_TABLES),
        run_nonce: 0,
    });
    make_ready(&mut built.engine, 0);
    dispatch_exec(
        &mut built.engine,
        ExecEvent {
            kind: ExecKind::SnapshotOrder,
            ..live_order_event(10)
        },
    );
    dispatch_exec(
        &mut built.engine,
        ExecEvent {
            kind: ExecKind::ReportNew,
            ..live_order_event(20)
        },
    );
    built
}

fn live_order_event(when: i64) -> ExecEvent {
    let client_id = ClientIdLayout { run_nonce: 0 }.encode(side_base(INSTRUMENT, Side::Buy) + 1, 1);
    ExecEvent {
        qty: QUOTE_QTY,
        ..exec_event(INSTRUMENT, client_id, Side::Buy, BID, when)
    }
}

/// The venue refusing a request against an order it is still holding — the class that counts toward
/// the streak, as opposed to the post-only refusals a maker earns all day.
fn hard_reject(when: i64) -> ExecEvent {
    ExecEvent {
        kind: ExecKind::AckFailed,
        reject: Some(RejectClass::StillLive),
        ..live_order_event(when)
    }
}

fn dispatch_exec(engine: &mut HotEngine, event: ExecEvent) {
    engine.dispatch(pop(0, 0), &InboundMessage::Exec(event));
}

/// Whether the engine pulled every order it owns because a kill switch tripped. That command is what
/// a halt DOES, so it is the honest way to observe one from outside.
fn has_halted(commands: &mut Consumer<ExecLaneItem>) -> bool {
    drain_commands(commands).iter().any(|command| {
        matches!(
            command,
            ExecCommand::CancelOurs {
                reason: CancelReason::Halt,
                ..
            }
        )
    })
}
