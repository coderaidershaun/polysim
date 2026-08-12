//! DOM projection fitness (chunk 3): the pure map from a book snapshot + simulated quote to a
//! fixed-center tick ladder. Anchors are exact integer math on tick indices at both mid parities;
//! empty ticks keep their rows as `None` (never a fabricated zero); off-grid prices are invalid, not
//! rounded; the quote highlights exactly one row per side or reports a directional off-screen delta.
//! The model half asserts latest-per-instrument retention, sequence-gap counting, and event-time
//! quote liveness; the formatters render the tick-unit convention the spec's `65991` example fixes.
//! The fit half pins the one geometry rule that can fail silently: asking for more rows than the
//! panel holds costs rows, never legibility.

use std::collections::HashMap;

use polysim::desktop::dom_view::{
    DEFAULT_ROWS_PER_SIDE, DomGrouping, DomOverlay, DomRow, DomStatus, DomViewInput, FeedStatus,
    MAX_ROWS_PER_SIDE, MIN_ROWS_PER_SIDE, QuotePlacement, RowFit, ask_anchor, bid_anchor,
    build_dom_view, fit_rows, mid_half_ticks, price_for_row, tick_index,
};
use polysim::desktop::exec_model::{OrderCell, OrderStatus};
use polysim::desktop::format::{
    MISSING, qty_decimals, write_mid, write_opt_qty, write_qty, write_tick_price,
};
use polysim::desktop::model::UiModel;
use polysim::ids::{ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty};
use polysim::msg::inbound::Level;
use polysim::msg::ui::{DomQuote, UI_BOOK_LEVELS, UiBookSnapshot, UiBookState, UiEvent};
use polysim::time::{DurationUs, TsUs};
use proptest::prelude::*;

/// A one-mantissa tick, so a price mantissa reads directly as its tick index and the ladder math is
/// legible in the assertions.
const TICK: Price = Price(1);

fn book(
    instrument: u16,
    seq: u64,
    event_ts: i64,
    bids: &[(i64, i64)],
    asks: &[(i64, i64)],
) -> UiBookSnapshot {
    let empty = Level {
        price: Price(0),
        qty: Qty(0),
    };
    let mut bid_levels = [empty; UI_BOOK_LEVELS];
    let mut ask_levels = [empty; UI_BOOK_LEVELS];
    for (slot, &(price, qty)) in bid_levels.iter_mut().zip(bids) {
        *slot = Level {
            price: Price(price),
            qty: Qty(qty),
        };
    }
    for (slot, &(price, qty)) in ask_levels.iter_mut().zip(asks) {
        *slot = Level {
            price: Price(price),
            qty: Qty(qty),
        };
    }
    UiBookSnapshot {
        instrument: InstrumentId(instrument),
        seq,
        event_ts_us: TsUs::from_micros(event_ts),
        state: UiBookState::Valid,
        bid_len: bids.len() as u16,
        ask_len: asks.len() as u16,
        bids: bid_levels,
        asks: ask_levels,
    }
}

#[test]
fn integer_mid_anchors_skip_the_shared_tick() {
    // bb 100, ba 102 → mid_half 202 (even) → integer mid 101, which is the separator's own tick.
    let mid = mid_half_ticks(Price(100), Price(102), TICK).expect("on-grid");
    assert_eq!(mid, 202);
    assert_eq!(
        ask_anchor(mid),
        102,
        "nearest ask sits one tick above the integer mid"
    );
    assert_eq!(bid_anchor(mid), 100, "nearest bid sits one tick below it");

    let snapshot = book(0, 0, 10, &[(100, 4), (99, 2)], &[(102, 5), (103, 7)]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.mid_half_ticks, Some(202));
    assert_eq!(view.status, DomStatus::Live);

    let asks = view.ask_rows();
    assert_eq!(asks[0].tick_index, 102);
    assert_eq!(asks[0].public_qty, Some(Qty(5)));
    assert_eq!(asks[1].tick_index, 103);
    assert_eq!(asks[1].public_qty, Some(Qty(7)));
    assert_eq!(
        asks[2].public_qty, None,
        "the far ask tick is empty, not zero"
    );

    let bids = view.bid_rows();
    assert_eq!(bids[0].tick_index, 100);
    assert_eq!(bids[0].public_qty, Some(Qty(4)));
    assert_eq!(bids[1].tick_index, 99);
    assert_eq!(bids[1].public_qty, Some(Qty(2)));
    assert_eq!(bids[2].public_qty, None);
}

/// The odd-mid half of the anchor rule: an odd `mid_half` skips no tick, so both anchors sit one
/// tick off the mid and every tick between them and the best price is a row with no quantity —
/// `None`, never a fabricated zero.
#[test]
fn empty_ticks_between_best_and_mid_stay_none() {
    // Wide spread: bb 100, ba 105 → mid_half 205 (odd) → 102.5; ticks 101-104 have no quantity.
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(105, 5)]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.mid_half_ticks, Some(205));

    let asks = view.ask_rows();
    assert_eq!(asks[0].tick_index, 103);
    assert_eq!(
        asks[0].public_qty, None,
        "an empty tick nearer mid is None, not 0"
    );
    assert_eq!(asks[1].public_qty, None);
    assert_eq!(asks[2].tick_index, 105);
    assert_eq!(asks[2].public_qty, Some(Qty(5)));

    let bids = view.bid_rows();
    assert_eq!(bids[0].tick_index, 102);
    assert_eq!(bids[0].public_qty, None);
    assert_eq!(bids[2].tick_index, 100);
    assert_eq!(bids[2].public_qty, Some(Qty(4)));
}

/// The view's rows live in fixed arrays, so a request beyond them is clamped rather than
/// truncating into memory that is not there.
#[test]
fn rows_per_side_is_capped_at_the_arrays_bound() {
    assert_eq!(
        MAX_ROWS_PER_SIDE, 30,
        "the level control's top end is the array bound; the two move together or not at all"
    );
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(102, 5)]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 200,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.ask_rows().len(), MAX_ROWS_PER_SIDE);
    assert_eq!(view.bid_rows().len(), MAX_ROWS_PER_SIDE);
}

#[test]
fn every_desired_ladder_level_is_projected_and_wide_buckets_accumulate() {
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(102, 5)]);
    let mut quote = DomQuote::default();
    quote.bids[0] = Some((Price(100), Qty(3)));
    quote.bids[1] = Some((Price(99), Qty(4)));
    quote.asks[0] = Some((Price(102), Qty(5)));
    quote.asks[1] = Some((Price(103), Qty(6)));

    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: overlay(quote),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.bid_placement, QuotePlacement::Visible);
    assert_eq!(view.ask_placement, QuotePlacement::Visible);
    assert_eq!(view.bid_rows()[0].strategy_qty, Some(Qty(3)));
    assert_eq!(view.bid_rows()[1].strategy_qty, Some(Qty(4)));
    assert_eq!(view.ask_rows()[0].strategy_qty, Some(Qty(5)));
    assert_eq!(view.ask_rows()[1].strategy_qty, Some(Qty(6)));
    assert_eq!(
        view.ask_rows()[2].strategy_qty,
        None,
        "no row past the quoted ladder carries a strategy qty"
    );
    let quoted: Vec<i64> = view
        .ask_rows()
        .iter()
        .chain(view.bid_rows())
        .filter(|row| row.is_quoted)
        .map(|row| row.tick_index)
        .collect();
    assert_eq!(
        quoted,
        vec![102, 103, 100, 99],
        "each quoted tick flags its own row and no other"
    );

    let grouped = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: overlay(quote),
        tick: TICK,
        grouping: DomGrouping::Ticks { per_bucket: 10 },
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(grouped.ask_rows()[0].strategy_qty, Some(Qty(11)));
}

#[test]
fn off_screen_quotes_report_directional_half_tick_delta() {
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(102, 5)]);
    // mid_half 202. Ask window [102,104]; bid window [98,100] at 3 rows.
    let quote = DomQuote::top(Some((Price(90), Qty(9))), Some((Price(110), Qty(9))));
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: overlay(quote),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(
        view.ask_placement,
        QuotePlacement::OffScreenAbove {
            delta_half_ticks: 18
        },
        "ask at tick 110 is |220-202| half-ticks above mid"
    );
    assert_eq!(
        view.bid_placement,
        QuotePlacement::OffScreenBelow {
            delta_half_ticks: 22
        },
        "bid at tick 90 is |180-202| half-ticks below mid"
    );
    assert!(
        view.ask_rows()
            .iter()
            .chain(view.bid_rows())
            .all(|r| !r.is_quoted),
        "an off-screen quote never clamps its highlight onto a visible row"
    );
}

#[test]
fn locked_book_shows_the_price_only_in_the_separator() {
    // bb == ba == 100 → mid_half 200 (even) → mid 100; a0 101, b0 99 skip tick 100 entirely.
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(100, 5)]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.mid_half_ticks, Some(200));
    assert!(
        view.ask_rows().iter().all(|r| r.public_qty.is_none()),
        "the locked ask sits at the mid tick, shown only in the separator"
    );
    assert!(view.bid_rows().iter().all(|r| r.public_qty.is_none()));
    assert_eq!(view.ask_rows()[0].tick_index, 101);
    assert_eq!(view.bid_rows()[0].tick_index, 99);

    let mut label = String::new();
    write_mid(
        &mut label,
        view.mid_half_ticks.expect("a locked book still has a mid"),
    );
    assert_eq!(label, "100");
}

#[test]
fn off_grid_price_is_invalid_never_rounded() {
    assert_eq!(
        tick_index(Price(105), Price(10)),
        None,
        "105 is off a 10 grid"
    );
    assert_eq!(tick_index(Price(100), Price(10)), Some(10));
    assert_eq!(
        tick_index(Price(5), Price(0)),
        None,
        "a non-positive tick has no grid"
    );

    // An off-grid best price yields no trustworthy mid and no ladder rows — a display state.
    let snapshot = book(0, 0, 10, &[(100, 4)], &[(105, 5)]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: Price(10),
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.mid_half_ticks, None);
    assert_eq!(view.ask_rows().len(), 0);
    assert!(view.ask_rows().is_empty());
}

#[test]
fn one_sided_book_has_no_mid_and_no_rows() {
    let snapshot = book(0, 0, 10, &[(100, 4)], &[]);
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(view.mid_half_ticks, None);
    assert_eq!(view.ask_rows().len(), 0);
    assert_eq!(
        view.status,
        DomStatus::Live,
        "one-sided is fresh, just un-mid-able"
    );
}

#[test]
fn status_precedence_disconnected_then_awaiting_then_stale() {
    let valid = book(0, 0, 10, &[(100, 4)], &[(102, 5)]);
    let mut awaiting = valid;
    awaiting.state = UiBookState::AwaitingSnapshot;

    let disconnected = build_dom_view(DomViewInput {
        snapshot: Some(&valid),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::Disconnected,
    });
    assert_eq!(disconnected.status, DomStatus::Disconnected);

    let stale = build_dom_view(DomViewInput {
        snapshot: Some(&valid),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::Stale,
    });
    assert_eq!(stale.status, DomStatus::Stale);
    assert_eq!(
        stale.mid_half_ticks,
        Some(202),
        "a stale book still projects its last ladder"
    );

    let awaiting_view = build_dom_view(DomViewInput {
        snapshot: Some(&awaiting),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(awaiting_view.status, DomStatus::AwaitingBook);
    assert_eq!(awaiting_view.ask_rows().len(), 0);

    let no_book = build_dom_view(DomViewInput {
        snapshot: None,
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 3,
        feed: FeedStatus::default(),
    });
    assert_eq!(no_book.status, DomStatus::AwaitingBook);
}

#[test]
fn price_for_row_round_trips_and_guards_overflow() {
    assert_eq!(price_for_row(Price(10), 5), Some(Price(50)));
    assert_eq!(
        price_for_row(Price(i64::MAX), 2),
        None,
        "overflow yields None, not a wrap"
    );
}

proptest! {
    /// FITNESS: a ladder never paints a row under the legibility floor. Too many levels must cost
    /// ROWS — a fit that shrank the row instead would overlap price against quantity, and a DOM
    /// whose numbers overlap is worse than one showing fewer of them. The clamp must also bite only
    /// when it must: a fit that quietly returned a safe small count would be legible and useless.
    #[test]
    fn a_ladder_never_paints_a_row_under_the_legibility_floor(
        side_height in 0.0f32..2_000.0,
        requested in 0usize..200,
        floor_height in 1.0f32..30.0,
    ) {
        let fit = fit_rows(side_height, requested, floor_height);

        prop_assert!(fit.rows <= requested, "never more rows than asked for: {fit:?}");
        prop_assert!(fit.rows <= MAX_ROWS_PER_SIDE, "never past the array bound: {fit:?}");
        prop_assert!(fit.row_height.is_finite(), "no row height is a division by zero: {fit:?}");

        if fit.rows == 0 {
            prop_assert!(fit.row_height >= 0.0);
            return Ok(());
        }
        // The slack is f32 division rounding, not a design allowance.
        prop_assert!(
            fit.row_height >= floor_height - floor_height * 1e-5,
            "{} rows of {} in {side_height} breaches the {floor_height} floor",
            fit.rows, fit.row_height
        );
        prop_assert!(
            fit.rows == requested.min(MAX_ROWS_PER_SIDE)
                || (fit.rows + 1) as f32 * floor_height > side_height,
            "the clamp bit early: {fit:?} of {requested} asked in {side_height} at floor {floor_height}"
        );
    }
}

#[test]
fn a_dense_ladder_clamps_its_row_count_not_its_row_height() {
    assert_eq!(
        fit_rows(180.0, 20, 9.0),
        RowFit {
            rows: 20,
            row_height: 9.0
        },
        "a side that exactly holds the request gives all of it"
    );
    // Stays under MAX_ROWS_PER_SIDE so the HEIGHT is the only thing that can bind here; the array
    // bound gets its own case below, and a request over both would not say which one clamped.
    assert_eq!(
        fit_rows(180.0, 25, 9.0).rows,
        20,
        "asking for more than fits loses the rows, not the row height"
    );
    assert_eq!(fit_rows(180.0, 25, 9.0).row_height, 9.0);
    assert_eq!(
        fit_rows(10_000.0, 200, 9.0).rows,
        MAX_ROWS_PER_SIDE,
        "a tall panel still stops at the array bound"
    );
    assert_eq!(
        fit_rows(0.0, 20, 9.0).rows,
        0,
        "a collapsed panel shows nothing rather than dividing by a row count of zero"
    );
}

/// The level control writes through the model, so the model — not the control — owns the range. A
/// value from anywhere else (a restored preference, a fixture, a future keyboard shortcut) is
/// clamped on the way in rather than reaching the fixed arrays.
#[test]
fn the_model_holds_a_level_count_inside_the_controls_range() {
    // A range whose default sits outside it is a build failure, not a test failure.
    const { assert!(MIN_ROWS_PER_SIDE <= DEFAULT_ROWS_PER_SIDE) };
    const { assert!(DEFAULT_ROWS_PER_SIDE <= MAX_ROWS_PER_SIDE) };

    let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
    assert_eq!(model.dom_levels(), DEFAULT_ROWS_PER_SIDE);
    assert_eq!(model.dom_levels(), 12);

    model.set_dom_levels(20);
    assert_eq!(model.dom_levels(), 20, "a value inside the range is kept");
    model.set_dom_levels(3);
    assert_eq!(model.dom_levels(), MIN_ROWS_PER_SIDE);
    model.set_dom_levels(999);
    assert_eq!(model.dom_levels(), MAX_ROWS_PER_SIDE);
}

#[test]
fn model_keeps_latest_snapshot_per_instrument() {
    let mut model = UiModel::with_capacity(2, DurationUs::from_micros(100_000));
    model.apply_book(book(0, 0, 10, &[(100, 1)], &[(102, 1)]));
    model.apply_book(book(0, 1, 20, &[(100, 9)], &[(102, 1)]));
    model.apply_book(book(1, 0, 30, &[(50, 3)], &[(52, 3)]));

    assert_eq!(
        model.book(InstrumentId(0)).map(|b| b.bids[0].qty),
        Some(Qty(9)),
        "the higher-seq snapshot wins for instrument 0"
    );
    assert_eq!(model.book(InstrumentId(0)).map(|b| b.seq), Some(1));
    assert_eq!(model.book(InstrumentId(1)).map(|b| b.seq), Some(0));
    assert_eq!(model.book_gaps(), 0, "a contiguous sequence has no gaps");
}

#[test]
fn model_counts_event_sequence_gaps() {
    let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
    let quote = |seq: u64| UiEvent::Quote {
        instrument: InstrumentId(0),
        seq,
        event_ts_us: TsUs::from_micros(1_000),
        quote: DomQuote::default(),
    };
    model.apply_event(quote(0));
    model.apply_event(quote(1));
    model.apply_event(quote(3)); // seq 2 was dropped
    assert_eq!(model.event_gaps(), 1, "the missing seq is counted once");
}

#[test]
fn model_judges_quote_liveness_in_event_time() {
    // spin 100 ms → liveness threshold 2.5 spins = 250 ms.
    let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
    let inst = InstrumentId(0);
    assert!(
        !model.is_quote_live(inst),
        "no quote and no book is not live"
    );
    model.apply_book(book(0, 0, 10, &[(100, 1)], &[(102, 1)]));
    assert!(
        !model.is_quote_live(inst),
        "a book without a quote is not live"
    );

    model.apply_event(UiEvent::Quote {
        instrument: inst,
        seq: 0,
        event_ts_us: TsUs::from_micros(1_000_000),
        quote: DomQuote::top(Some((Price(100), Qty(1))), None),
    });

    model.apply_book(book(0, 1, 1_200_000, &[(100, 1)], &[(102, 1)]));
    assert!(
        model.is_quote_live(inst),
        "book 200 ms past the quote is within threshold"
    );

    model.apply_book(book(0, 2, 1_400_000, &[(100, 1)], &[(102, 1)]));
    assert!(
        !model.is_quote_live(inst),
        "book 400 ms past the quote is stale"
    );
}

#[test]
fn formatters_render_the_tick_unit_convention() {
    let mut buffer = String::new();

    write_mid(&mut buffer, 131_982);
    assert_eq!(
        buffer, "65991",
        "an even mid_half renders a whole tick count"
    );
    write_mid(&mut buffer, 131_983);
    assert_eq!(buffer, "65991.5", "an odd mid_half renders the half-tick");

    write_tick_price(&mut buffer, 65_991);
    assert_eq!(buffer, "65991");

    write_qty(&mut buffer, Qty(9 * FIXED_SCALE), FIXED_SCALE, 3);
    assert_eq!(buffer, "9.000", "decimals stay stable for the instrument");
    write_qty(&mut buffer, Qty(FIXED_SCALE / 100), FIXED_SCALE, 3);
    assert_eq!(buffer, "0.010");

    write_opt_qty(&mut buffer, None, FIXED_SCALE, 3);
    assert_eq!(buffer, MISSING, "absent is em-dash");
    write_opt_qty(&mut buffer, Some(Qty(0)), FIXED_SCALE, 3);
    assert_eq!(
        buffer, "0.000",
        "a real zero is distinguishable from missing"
    );

    assert_eq!(
        qty_decimals(Some(Qty(FIXED_SCALE / 1000))),
        3,
        "a 0.001 lot needs three places"
    );
    assert_eq!(
        qty_decimals(Some(Qty(FIXED_SCALE))),
        0,
        "a whole-unit lot needs none"
    );
    assert_eq!(qty_decimals(None), 3, "the fallback is stable");
}

#[test]
fn write_qty_keeps_sign_for_sub_unit_magnitudes() {
    let mut buffer = String::new();
    write_qty(&mut buffer, Qty(-FIXED_SCALE / 2), FIXED_SCALE, 3);
    assert_eq!(
        buffer, "-0.500",
        "a negative magnitude below one unit keeps its sign even though the integer part is zero"
    );
    write_qty(&mut buffer, Qty(-3 * FIXED_SCALE / 2), FIXED_SCALE, 3);
    assert_eq!(
        buffer, "-1.500",
        "a negative magnitude above one unit still signs"
    );
    write_qty(&mut buffer, Qty(FIXED_SCALE / 2), FIXED_SCALE, 3);
    assert_eq!(buffer, "0.500", "a positive sub-unit magnitude has no sign");
}

proptest! {
    /// Every book level whose tick falls inside the visible window lands on exactly its own tick
    /// row; every row without a matching level is `None`; nothing outside the window appears.
    #[test]
    fn levels_land_on_their_tick_rows(
        best_bid in 1_000i64..2_000,
        spread in 1i64..8,
        bid_gaps in prop::collection::vec(1i64..6, 0..8),
        ask_gaps in prop::collection::vec(1i64..6, 0..8),
    ) {
        let best_ask = best_bid + spread;

        let mut bid_levels = vec![(best_bid, 11i64)];
        let mut tick = best_bid;
        for (i, gap) in bid_gaps.iter().enumerate() {
            tick -= gap;
            bid_levels.push((tick, 12 + i as i64));
        }
        let mut ask_levels = vec![(best_ask, 21i64)];
        let mut tick = best_ask;
        for (i, gap) in ask_gaps.iter().enumerate() {
            tick += gap;
            ask_levels.push((tick, 22 + i as i64));
        }

        let snapshot = book(0, 0, 10, &bid_levels, &ask_levels);
        let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay::default(),
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: MAX_ROWS_PER_SIDE,
        feed: FeedStatus::default(),
    });

        let ask_by_tick: HashMap<i64, i64> = ask_levels.iter().copied().collect();
        let bid_by_tick: HashMap<i64, i64> = bid_levels.iter().copied().collect();

        for row in view.ask_rows() {
            let expected = ask_by_tick.get(&row.tick_index).map(|&q| Qty(q));
            prop_assert_eq!(row.public_qty, expected);
        }
        for row in view.bid_rows() {
            let expected = bid_by_tick.get(&row.tick_index).map(|&q| Qty(q));
            prop_assert_eq!(row.public_qty, expected);
        }
    }
}

/// A ladder overlay carrying only the desired quote — the shape every case below drives, since real
/// working orders are pinned separately.
fn overlay(desired: DomQuote) -> DomOverlay<'static> {
    DomOverlay {
        desired: Some(desired),
        bid_orders: &[],
        ask_orders: &[],
    }
}

/// A REAL working order and the engine's DESIRED quote occupy separate fields on the same row. The
/// ladder must be able to show both at once, because a requote in progress is exactly the moment an
/// operator needs to see that what is resting is not what is wanted.
#[test]
fn a_real_order_and_a_desired_quote_are_separate_row_fields() {
    let snapshot = valid_snapshot(&[(100, 5)], &[(102, 5)]);
    let bid_orders = [working(1, 99, 7, OrderStatus::Confirmed)];
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay {
            desired: Some(DomQuote::top(Some((Price(98), Qty(3))), None)),
            bid_orders: &bid_orders,
            ask_orders: &[],
        },
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 6,
        feed: FeedStatus::default(),
    });

    let order_row = row_at(view.bid_rows(), 99);
    assert_eq!(order_row.order_qty, Some(Qty(7)));
    assert_eq!(order_row.order_status, Some(OrderStatus::Confirmed));
    assert_eq!(
        order_row.strategy_qty, None,
        "the desired quote is on its own row, not merged onto the order's"
    );

    let desired_row = row_at(view.bid_rows(), 98);
    assert_eq!(desired_row.strategy_qty, Some(Qty(3)));
    assert_eq!(
        desired_row.order_qty, None,
        "an intention must never paint into the field that means real exposure"
    );
}

/// A row is painted from what is REMAINING, not from what was ordered. A half-filled order shown at
/// its original size claims depth at that price that the venue has already taken.
#[test]
fn a_partially_filled_order_shows_only_what_remains() {
    let snapshot = valid_snapshot(&[(100, 5)], &[(102, 5)]);
    let mut order = working(1, 99, 10, OrderStatus::Confirmed);
    order.filled = Qty(4);
    let orders = [order];
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay {
            desired: None,
            bid_orders: &orders,
            ask_orders: &[],
        },
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 6,
        feed: FeedStatus::default(),
    });
    assert_eq!(row_at(view.bid_rows(), 99).order_qty, Some(Qty(6)));
}

/// Two orders sharing a bucket report the LEAST certain of their statuses. A bucket that claimed
/// `Confirmed` while holding an in-flight order would show size at a price the venue may never have
/// accepted.
#[test]
fn a_shared_bucket_reports_the_least_certain_status() {
    let snapshot = valid_snapshot(&[(100, 5)], &[(102, 5)]);
    let orders = [
        working(1, 98, 4, OrderStatus::Confirmed),
        working(2, 99, 6, OrderStatus::InFlight),
    ];
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay {
            desired: None,
            bid_orders: &orders,
            ask_orders: &[],
        },
        tick: TICK,
        grouping: // Five ticks a row, so both orders land in one bucket.
        DomGrouping::Ticks { per_bucket: 5 },
        rows_per_side: 6,
        feed: FeedStatus::default(),
    });
    let bucket = view
        .bid_rows()
        .iter()
        .find(|row| row.order_qty.is_some())
        .expect("both orders fall inside the window");
    assert_eq!(
        bucket.order_qty,
        Some(Qty(10)),
        "the bucket sums its members"
    );
    assert_eq!(bucket.order_status, Some(OrderStatus::InFlight));
}

/// An order beyond the window reports its own placement, independently of the desired quote's. Real
/// exposure the operator cannot see is the fact the chevron exists for.
#[test]
fn an_off_window_order_reports_its_own_placement() {
    let snapshot = valid_snapshot(&[(100, 5)], &[(102, 5)]);
    let orders = [working(1, 40, 6, OrderStatus::Lost)];
    let view = build_dom_view(DomViewInput {
        snapshot: Some(&snapshot),
        overlay: DomOverlay {
            desired: None,
            bid_orders: &orders,
            ask_orders: &[],
        },
        tick: TICK,
        grouping: DomGrouping::default(),
        rows_per_side: 6,
        feed: FeedStatus::default(),
    });
    assert!(
        matches!(
            view.bid_order_placement,
            QuotePlacement::OffScreenBelow { .. }
        ),
        "an order below the window reports below, got {:?}",
        view.bid_order_placement
    );
    assert_eq!(
        view.bid_placement,
        QuotePlacement::None,
        "the desired quote's placement is its own and stays absent"
    );
}

/// A two-sided book at the tick grid the cases above use.
fn valid_snapshot(bids: &[(i64, i64)], asks: &[(i64, i64)]) -> UiBookSnapshot {
    book(0, 0, 10, bids, asks)
}

/// One of this engine's working orders for the cases above.
fn working(id: u64, tick: i64, qty: i64, status: OrderStatus) -> OrderCell {
    OrderCell {
        client_id: ClientOrderId(id),
        quote_level: None,
        status,
        price: Price(tick * TICK.0),
        qty: Qty(qty),
        filled: Qty(0),
        at: TsUs::from_micros(0),
    }
}

fn row_at(rows: &[DomRow], tick_index: i64) -> DomRow {
    *rows
        .iter()
        .find(|row| row.tick_index == tick_index)
        .unwrap_or_else(|| panic!("tick {tick_index} is inside the window"))
}

/// The stale badge's age is a distance between two ENGINE stamps. The workstation is a separate
/// process, reachable over UDP at any address, so its own clock is not comparable with the engine's:
/// a few seconds of skew the wrong way would make a frozen ladder read as a live one, which is the
/// single reading this indicator exists to prevent. The reference is the freshest stamp seen on
/// EITHER lane, so an engine whose other instruments keep reporting still ages the frozen one.
#[test]
fn a_books_age_is_measured_against_the_engines_own_newest_stamp() {
    let mut model = UiModel::with_capacity(2, DurationUs::from_micros(100_000));
    assert_eq!(
        model.book_lag(InstrumentId(0)),
        None,
        "an instrument with no book has no age, not an age of zero"
    );

    model.apply_book(book(0, 0, 1_000_000, &[(100, 1)], &[(102, 1)]));
    model.apply_book(book(1, 0, 1_000_000, &[(100, 1)], &[(102, 1)]));
    assert_eq!(model.book_lag(InstrumentId(0)), Some(DurationUs::ZERO));

    // Instrument 0's book freezes while instrument 1 keeps arriving: the engine is plainly alive,
    // and the frozen ladder must say how far behind it is.
    model.apply_book(book(1, 1, 6_000_000, &[(100, 1)], &[(102, 1)]));
    assert_eq!(
        model.book_lag(InstrumentId(0)),
        Some(DurationUs::from_micros(5_000_000)),
    );
    assert_eq!(model.book_lag(InstrumentId(1)), Some(DurationUs::ZERO));

    // The event lane carries the same clock, so it advances the reference too — an engine emitting
    // quotes with no book updates at all still ages the ladder.
    model.apply_event(UiEvent::Quote {
        instrument: InstrumentId(1),
        seq: 0,
        event_ts_us: TsUs::from_micros(9_000_000),
        quote: DomQuote::default(),
    });
    assert_eq!(
        model.book_lag(InstrumentId(0)),
        Some(DurationUs::from_micros(8_000_000)),
    );

    // Reference stamps are absolute engine time, not an offset from this run's start: the same fold
    // a decade of wall clock later reports the same lag.
    let mut decade_later = UiModel::with_capacity(2, DurationUs::from_micros(100_000));
    decade_later.apply_book(book(0, 0, 1_315_000_000_000_000, &[(100, 1)], &[(102, 1)]));
    decade_later.apply_book(book(1, 0, 1_315_000_005_000_000, &[(100, 1)], &[(102, 1)]));
    assert_eq!(
        decade_later.book_lag(InstrumentId(0)),
        Some(DurationUs::from_micros(5_000_000)),
    );
}
