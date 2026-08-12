//! Monitor projection fitness: the pure map from the ordered UI event lane to the center
//! panel's state. The quote summary reads unsigned tick distances from the book mid at both mid
//! parities and blanks each cell independently; the feature store folds latest-per-(instrument,
//! feature) in catalog order, separating a genuine value change from a re-emission and a stale feed
//! from a legitimate zero; the bounded channel histories evict oldest-first and iterate newest-first
//! with a monotonic appended total behind the unseen count; the System channel synthesizes rows from
//! book-state transitions, rotations, gap counts and lifecycle notes. All of it is event-time
//! only, so replay reproduces the state exactly. The formatters render the time, delta and
//! feature-value conventions the panel paints.

use proptest::prelude::*;

use polysim::config::ExecutionMode;
use polysim::desktop::format::{
    MISSING, write_bps_delta, write_feature_value, write_half_tick_delta, write_opt_bps_delta,
    write_time_of_day,
};
use polysim::desktop::model::UiModel;
use polysim::desktop::monitor::{Channel, MonitorUiState};
use polysim::desktop::monitor_model::{FeatureCell, FillRow, OrderEvent, SystemEvent, SystemNote};
use polysim::desktop::monitor_view::{
    FeatureRowView, account, feature_rows, quote_summary, unseen,
};
use polysim::hot::exec::{CloseReason, ExecHalt, HaltReason, OrderState, RejectOrigin};
use polysim::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{Liquidity, RejectClass};
use polysim::msg::inbound::Level;
use polysim::msg::ui::{
    DomQuote, UI_BOOK_LEVELS, UI_ORDER_SNAPSHOT_CAPACITY, UiBookSnapshot, UiBookState, UiCatalog,
    UiEvent, UiInstrument, UiWorkingOrder,
};
use polysim::time::{DurationUs, TsUs};

/// One-mantissa tick, so a price reads directly as its tick index in the assertions.
const TICK: Price = Price(1);

/// 100 ms spin: the DOM liveness threshold is 2.5 spins = 250 ms, the changed window 2 spins =
/// 200 ms, the stale threshold 5 spins = 500 ms.
const SPIN: DurationUs = DurationUs::from_micros(100_000);

fn model(instruments: usize, features: usize) -> UiModel {
    UiModel::with_monitor_capacity(instruments, features, SPIN)
}

fn book(
    instrument: u16,
    seq: u64,
    event_ts: i64,
    state: UiBookState,
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
        state,
        bid_len: bids.len() as u16,
        ask_len: asks.len() as u16,
        bids: bid_levels,
        asks: ask_levels,
    }
}

fn valid_book(instrument: u16, seq: u64, event_ts: i64, bid: i64, ask: i64) -> UiBookSnapshot {
    book(
        instrument,
        seq,
        event_ts,
        UiBookState::Valid,
        &[(bid, 1)],
        &[(ask, 1)],
    )
}

fn quote_event(
    instrument: u16,
    seq: u64,
    event_ts: i64,
    bid: Option<(i64, i64)>,
    ask: Option<(i64, i64)>,
) -> UiEvent {
    let leg = |value: Option<(i64, i64)>| value.map(|(p, q)| (Price(p), Qty(q)));
    UiEvent::Quote {
        instrument: InstrumentId(instrument),
        seq,
        event_ts_us: TsUs::from_micros(event_ts),
        quote: DomQuote::top(leg(bid), leg(ask)),
    }
}

fn feature_event(instrument: u16, seq: u64, event_ts: i64, feature: u16, value: f64) -> UiEvent {
    UiEvent::Feature {
        instrument: InstrumentId(instrument),
        seq,
        event_ts_us: TsUs::from_micros(event_ts),
        feature: polysim::msg::persist::FeatureId(feature),
        value,
    }
}

fn trade_event(instrument: u16, seq: u64, event_ts: i64, qty: i64) -> UiEvent {
    UiEvent::Trade {
        instrument: InstrumentId(instrument),
        seq,
        event_ts_us: TsUs::from_micros(event_ts),
        aggressor: Side::Buy,
        price: Price(100),
        qty: Qty(qty),
    }
}

fn fill_event(
    instrument: u16,
    seq: u64,
    event_ts: i64,
    side: Side,
    price: i64,
    qty: i64,
) -> UiEvent {
    UiEvent::Fill {
        instrument: InstrumentId(instrument),
        seq,
        event_ts_us: TsUs::from_micros(event_ts),
        quote_level: None,
        side,
        price: Price(price),
        qty: Qty(qty),
        commission: 944_000,
        commission_asset: AssetId(1),
        liquidity: Some(Liquidity::Maker),
    }
}

fn feature(model: &UiModel, instrument: u16, feature: u16) -> Option<FeatureCell> {
    model.monitor().feature(
        InstrumentId(instrument),
        polysim::msg::persist::FeatureId(feature),
    )
}

fn rows(model: &UiModel, instrument: u16) -> Vec<FeatureRowView> {
    feature_rows(model, InstrumentId(instrument)).collect()
}

#[test]
fn summary_even_mid_renders_unsigned_whole_tick_deltas() {
    // Book best 100/102 → mid tick 101 (mid_half 202). Strategy quote bid 2 ticks below (tick 99),
    // ask 3 ticks above (tick 104): the spec's reference bid 2 / ask 3.
    let mut model = model(1, 0);
    model.apply_event(quote_event(0, 0, 1_000, Some((99, 5)), Some((104, 5))));
    model.apply_book(valid_book(0, 0, 1_050, 100, 102));

    let summary = quote_summary(&model, InstrumentId(0), TICK);
    assert_eq!(summary.mid_half_ticks, Some(202));
    assert_eq!(
        summary.bid_delta_half_ticks,
        Some(4),
        "bid at tick 99 is 4 half-ticks (2 ticks) from the mid, unsigned"
    );
    assert_eq!(summary.ask_delta_half_ticks, Some(6));

    let mut buffer = String::new();
    write_half_tick_delta(&mut buffer, summary.bid_delta_half_ticks.unwrap());
    assert_eq!(buffer, "2", "the bid below mid carries no minus sign");
    write_half_tick_delta(&mut buffer, summary.ask_delta_half_ticks.unwrap());
    assert_eq!(buffer, "3");
}

#[test]
fn summary_half_tick_mid_renders_half_tick_deltas() {
    // Book best 100/101 → mid 100.5 (mid_half 201). Quote bid tick 100, ask tick 102.
    let mut model = model(1, 0);
    model.apply_event(quote_event(0, 0, 1_000, Some((100, 5)), Some((102, 5))));
    model.apply_book(valid_book(0, 0, 1_050, 100, 101));

    let summary = quote_summary(&model, InstrumentId(0), TICK);
    assert_eq!(summary.mid_half_ticks, Some(201));
    assert_eq!(
        summary.bid_delta_half_ticks,
        Some(1),
        "|200-201| half-ticks"
    );
    assert_eq!(
        summary.ask_delta_half_ticks,
        Some(3),
        "|204-201| half-ticks"
    );

    let mut buffer = String::new();
    write_half_tick_delta(&mut buffer, 1);
    assert_eq!(buffer, "0.5");
    write_half_tick_delta(&mut buffer, 3);
    assert_eq!(buffer, "1.5");
}

#[test]
fn summary_cells_blank_independently() {
    // No book yet: every cell is —.
    let mut model = model(1, 0);
    model.apply_event(quote_event(0, 0, 1_000, Some((99, 5)), Some((104, 5))));
    let none = quote_summary(&model, InstrumentId(0), TICK);
    assert_eq!(none.mid_half_ticks, None);
    assert_eq!(none.bid_delta_half_ticks, None);
    assert_eq!(none.ask_delta_half_ticks, None);

    // A stale quote (book 400 ms past it) keeps the mid but blanks both deltas.
    model.apply_book(valid_book(0, 0, 1_000 + 400_000, 100, 102));
    let stale = quote_summary(&model, InstrumentId(0), TICK);
    assert_eq!(
        stale.mid_half_ticks,
        Some(202),
        "the book still yields a mid"
    );
    assert_eq!(
        stale.bid_delta_half_ticks, None,
        "a stale quote blanks its deltas"
    );
    assert_eq!(stale.ask_delta_half_ticks, None);

    // A live one-legged quote: only the present leg carries a delta.
    model.apply_event(quote_event(0, 1, 1_500_000, Some((99, 5)), None));
    model.apply_book(valid_book(0, 1, 1_550_000, 100, 102));
    let one_legged = quote_summary(&model, InstrumentId(0), TICK);
    assert_eq!(one_legged.bid_delta_half_ticks, Some(4));
    assert_eq!(
        one_legged.ask_delta_half_ticks, None,
        "no ask leg, no delta"
    );
}

#[test]
fn feature_folds_latest_and_separates_a_change_from_a_reemission() {
    let mut model = model(1, 3);
    model.apply_event(feature_event(0, 0, 1_000, 0, 1.0));
    model.apply_event(feature_event(0, 1, 2_000, 0, 2.0));

    let cell = feature(&model, 0, 0).expect("feature seen");
    assert_eq!(cell.value, 2.0, "the later value wins");
    assert_eq!(cell.last_update_ts, TsUs::from_micros(2_000));
    assert_eq!(cell.last_changed_ts, TsUs::from_micros(2_000));

    model.apply_event(feature_event(0, 2, 3_000, 0, 2.0));
    let cell = feature(&model, 0, 0).expect("feature seen");
    assert_eq!(
        cell.last_update_ts,
        TsUs::from_micros(3_000),
        "a re-emission still refreshes the update time"
    );
    assert_eq!(
        cell.last_changed_ts,
        TsUs::from_micros(2_000),
        "but the change time holds — the value did not differ"
    );
}

#[test]
fn feature_rows_hold_catalog_order() {
    let mut model = model(1, 3);
    // Emit out of id order; the rows still read 0,1,2.
    model.apply_event(feature_event(0, 0, 1_000, 2, 3.5));
    model.apply_event(feature_event(0, 1, 1_000, 0, 1.5));
    model.apply_event(feature_event(0, 2, 1_000, 1, 2.5));

    let ids: Vec<u16> = rows(&model, 0).iter().map(|r| r.feature.0).collect();
    assert_eq!(
        ids,
        vec![0, 1, 2],
        "rows never re-sort out of catalog order"
    );
    let values: Vec<Option<f64>> = rows(&model, 0).iter().map(|r| r.value).collect();
    assert_eq!(values, vec![Some(1.5), Some(2.5), Some(3.5)]);
}

#[test]
fn feature_nan_reemission_is_not_a_change() {
    let mut model = model(1, 1);
    model.apply_event(feature_event(0, 0, 1_000, 0, f64::NAN));
    model.apply_event(feature_event(0, 1, 2_000, 0, f64::NAN));

    let cell = feature(&model, 0, 0).expect("seen");
    assert!(cell.value.is_nan(), "the stored value stays NaN");
    assert_eq!(
        cell.last_update_ts,
        TsUs::from_micros(2_000),
        "a re-emitted NaN still refreshes the update time"
    );
    assert_eq!(
        cell.last_changed_ts,
        TsUs::from_micros(1_000),
        "bit-equal NaN re-emission is not a change — the change time holds"
    );
}

#[test]
fn feature_row_flags_change_then_fades() {
    let mut model = model(1, 2);
    model.apply_event(feature_event(0, 0, 1_000_000, 0, 1.0));
    model.apply_event(feature_event(0, 1, 1_100_000, 0, 2.0)); // changed 1 spin ago

    assert!(
        rows(&model, 0)[0].changed,
        "a value change within 2 spins wears the highlight"
    );

    // Advance the freshest feed 3 spins past the change on another feature; the highlight fades.
    model.apply_event(feature_event(0, 2, 1_400_000, 1, 9.0));
    assert!(
        !rows(&model, 0)[0].changed,
        "3 spins past the change is beyond the 2-spin window"
    );
}

#[test]
fn feature_stale_is_distinct_from_missing_and_from_a_real_zero() {
    let mut model = model(1, 3);
    // Feature 2 last updates at 1_000_000; feature 0 (a legitimate zero) refreshes 6 spins later,
    // dragging the freshest feed time past feature 2's staleness threshold. Feature 1 never emits.
    model.apply_event(feature_event(0, 0, 1_000_000, 2, 4.0));
    model.apply_event(feature_event(0, 1, 1_600_000, 0, 0.0));

    let rows = rows(&model, 0);
    assert_eq!(
        rows[0].value,
        Some(0.0),
        "a real zero is a value, not missing"
    );
    assert!(!rows[0].stale, "the fresh zero is not stale");
    assert_eq!(
        rows[1].value, None,
        "an unseen feature is missing (—), not 0"
    );
    assert!(
        !rows[1].stale,
        "a missing feature is not stale, it is absent"
    );
    assert_eq!(rows[2].value, Some(4.0));
    assert!(
        rows[2].stale,
        "feature 2 has not updated in 6 spins — stale, though its value is a real 4.0"
    );
}

#[test]
fn trade_tape_evicts_oldest_and_iterates_newest_first() {
    let mut model = model(1, 0);
    for i in 0..300u64 {
        model.apply_event(trade_event(0, i, 10_000 + i as i64, i as i64));
    }
    assert_eq!(
        model.monitor().trades_appended(InstrumentId(0)),
        300,
        "the appended total counts every print, evicted or not"
    );
    let tape: Vec<_> = model.monitor().trades(InstrumentId(0)).collect();
    assert_eq!(tape.len(), 256, "the tape retains its bounded capacity");
    assert_eq!(tape[0].qty, Qty(299), "newest print is first");
    assert_eq!(
        tape[255].qty,
        Qty(44),
        "the oldest 44 prints evicted; 44 is the oldest retained"
    );
}

/// A refusal shares the Orders channel with the transitions, because both answer the same operator
/// question — what happened to the quote I asked for — and a refusal in a separate place is a
/// refusal nobody sees. Eviction and newest-first ordering are the shared history's, pinned on the
/// trade tape above.
#[test]
fn the_orders_channel_carries_transitions_and_refusals() {
    let mut model = model(1, 0);
    model.apply_event(UiEvent::OrderUpdate {
        instrument: InstrumentId(0),
        seq: 0,
        event_ts_us: TsUs::from_micros(5),
        client_id: ClientOrderId(41),
        quote_level: None,
        side: Side::Buy,
        state: OrderState::Closed(CloseReason::Canceled),
        price: Price(100),
        qty: Qty(5),
        filled: Qty(0),
    });
    model.apply_event(UiEvent::Reject {
        instrument: InstrumentId(0),
        seq: 1,
        event_ts_us: TsUs::from_micros(10),
        side: Side::Sell,
        origin: RejectOrigin::Venue {
            class: RejectClass::Gone,
            code: -2010,
        },
    });

    assert_eq!(model.monitor().orders_appended(), 2);
    let orders: Vec<_> = model.monitor().orders().collect();
    assert_eq!(orders.len(), 2, "both land in the one channel");
    assert_eq!(
        orders[0].event,
        OrderEvent::Refused {
            origin: RejectOrigin::Venue {
                class: RejectClass::Gone,
                code: -2010,
            }
        },
        "the refusal carries the venue's own code and class, not a re-derived one"
    );
    let OrderEvent::Transition { client_id, .. } = orders[1].event else {
        panic!("an order update records a transition, got {:?}", orders[1]);
    };
    assert_eq!(client_id, ClientOrderId(41));
}

#[test]
fn fills_record_every_field_newest_first() {
    let mut model = model(1, 0);
    // A seller reaching our bid means we bought (Buy); a buyer reaching our ask means we sold (Sell).
    // Two sides, so a swapped side or a crossed field would fail the exact-struct pin.
    model.apply_event(fill_event(0, 0, 1_000, Side::Buy, 100, 7));
    model.apply_event(fill_event(0, 1, 2_000, Side::Sell, 102, 3));

    assert_eq!(model.monitor().fills_appended(), 2);
    let fills: Vec<_> = model.monitor().fills().copied().collect();
    assert_eq!(
        fills[0],
        FillRow {
            at: TsUs::from_micros(2_000),
            instrument: InstrumentId(0),
            quote_level: None,
            side: Side::Sell,
            price: Price(102),
            qty: Qty(3),
            commission: 944_000,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
        },
        "newest fill first, every field mapped straight from the event — including the fee and the \
         asset it was charged in, which a simulated fill could not carry"
    );
    assert_eq!(
        fills[1],
        FillRow {
            at: TsUs::from_micros(1_000),
            instrument: InstrumentId(0),
            quote_level: None,
            side: Side::Buy,
            price: Price(100),
            qty: Qty(7),
            commission: 944_000,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
        },
    );
}

#[test]
fn unseen_is_appended_minus_watermark_and_saturates() {
    let mut model = model(1, 0);
    for i in 0..10u64 {
        model.apply_event(trade_event(0, i, i as i64, i as i64));
    }
    let total = model.monitor().trades_appended(InstrumentId(0));
    assert_eq!(unseen(total, 0), 10, "nothing seen yet: all ten are unseen");
    assert_eq!(unseen(total, total), 0, "caught up: none unseen");

    model.apply_event(trade_event(0, 10, 10, 10));
    let grown = model.monitor().trades_appended(InstrumentId(0));
    assert_eq!(
        unseen(grown, total),
        1,
        "one appended past the watermark is one unseen (resume-follow basis)"
    );
    assert_eq!(
        unseen(5, 10),
        0,
        "a watermark ahead of the total saturates to 0"
    );
}

#[test]
fn a_book_round_trip_records_one_resync_row() {
    let mut model = model(1, 0);
    // First Valid snapshot is the silent baseline — no transition row.
    model.apply_book(valid_book(0, 0, 1_000, 100, 102));
    assert_eq!(
        model.monitor().system().count(),
        0,
        "the baseline emits no row"
    );

    // The drop alone records nothing — a book that is still down is live state, not history.
    model.apply_book(book(0, 1, 2_000, UiBookState::AwaitingSnapshot, &[], &[]));
    assert_eq!(
        model.monitor().system().count(),
        0,
        "the drop alone emits no row"
    );
    assert_eq!(
        model.monitor().book_state(InstrumentId(0)),
        Some(UiBookState::AwaitingSnapshot),
        "the down book is visible as live state"
    );

    // Coming back Valid closes the round trip: exactly one row, stamped at the rebuild.
    model.apply_book(valid_book(0, 2, 3_000, 100, 102));
    let system: Vec<_> = model.monitor().system().collect();
    assert_eq!(system.len(), 1, "one row for the whole round trip");
    assert_eq!(
        system[0].event,
        SystemEvent::BookResynced {
            instrument: InstrumentId(0)
        }
    );
    assert_eq!(system[0].at, Some(TsUs::from_micros(3_000)));
}

#[test]
fn system_rows_synthesize_rotation_and_gap_notes() {
    let mut model = model(1, 0);
    // A rotation event.
    model.apply_event(UiEvent::Rotation {
        instrument: InstrumentId(0),
        seq: 0,
        event_ts_us: TsUs::from_micros(500),
    });
    // An event-lane gap: seq jumps 0 → 2, dropping one.
    model.apply_event(trade_event(0, 2, 900, 1));
    // A book-lane gap: book seq jumps 0 → 2, dropping one (both Valid, so no reset/rebuild row).
    model.apply_book(valid_book(0, 0, 1_000, 100, 102));
    model.apply_book(valid_book(0, 2, 1_100, 100, 102));

    let events: Vec<SystemEvent> = model.monitor().system().map(|r| r.event.clone()).collect();
    assert!(
        events.contains(&SystemEvent::Rotation {
            instrument: InstrumentId(0)
        }),
        "the rotation is recorded"
    );
    assert!(
        events.contains(&SystemEvent::EventsLost { count: 1 }),
        "the one dropped event is noted, saw {events:?}"
    );
    assert!(
        events.contains(&SystemEvent::BooksLost { count: 1 }),
        "the one dropped snapshot is noted"
    );
}

#[test]
fn lifecycle_notes_land_in_the_system_channel_without_a_timestamp() {
    let mut model = model(1, 0);
    model.note_lifecycle(SystemNote::Ready);
    model.note_lifecycle(SystemNote::Draining {
        reason: "shutting down".into(),
    });

    let system: Vec<_> = model.monitor().system().collect();
    assert_eq!(system.len(), 2);
    assert_eq!(
        system[0].at, None,
        "a lifecycle transition carries no event time"
    );
    assert_eq!(
        system[0].event,
        SystemEvent::Lifecycle(SystemNote::Draining {
            reason: "shutting down".into()
        }),
        "newest first, reason carried verbatim"
    );
    assert_eq!(system[1].event, SystemEvent::Lifecycle(SystemNote::Ready));
}

#[test]
fn projection_is_replay_deterministic() {
    let run = || {
        let mut model = model(2, 2);
        model.apply_event(quote_event(0, 0, 1_000, Some((99, 5)), Some((104, 5))));
        model.apply_event(feature_event(0, 1, 1_000, 0, 1.5));
        model.apply_event(feature_event(0, 2, 1_100, 1, 0.0));
        model.apply_event(trade_event(0, 3, 1_200, 7));
        model.apply_event(fill_event(0, 4, 1_250, Side::Sell, 101, 2));
        model.apply_event(UiEvent::Rotation {
            instrument: InstrumentId(1),
            seq: 5,
            event_ts_us: TsUs::from_micros(1_300),
        });
        model.apply_book(valid_book(0, 0, 1_050, 100, 102));
        (
            quote_summary(&model, InstrumentId(0), TICK),
            rows(&model, 0),
            model.monitor().system().cloned().collect::<Vec<_>>(),
            model
                .monitor()
                .trades(InstrumentId(0))
                .copied()
                .collect::<Vec<_>>(),
            model.monitor().fills().copied().collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        run(),
        run(),
        "the same input sequence yields the same monitor state"
    );
}

#[test]
fn time_of_day_formats_utc_with_padding() {
    let mut buffer = String::new();
    write_time_of_day(&mut buffer, TsUs::from_micros(45_296_789_000));
    assert_eq!(buffer, "12:34:56.789");

    write_time_of_day(&mut buffer, TsUs::from_micros(3_661_002_000));
    assert_eq!(
        buffer, "01:01:01.002",
        "hours/minutes/seconds/millis zero-pad"
    );

    write_time_of_day(
        &mut buffer,
        TsUs::from_micros(86_400_000_000 + 45_296_789_000),
    );
    assert_eq!(
        buffer, "12:34:56.789",
        "the date is dropped; a full day wraps"
    );
}

/// BTC at 118000 on a 0.01 tick: `2 * 118000 / 0.01`. One tick is 0.00085 bp here, which is why the
/// delta cell cannot render at a fixed two places.
const BTC_MID_HALF_TICKS: i64 = 23_600_000;

/// A delta is read at a glance, so it ROUNDS to two significant figures rather than reporting every
/// place it could. Rounding is not the same as truncating the decimals: a fixed two-place rendering
/// would print the first two of these as `0.00`, which says the quote is at the mid.
#[test]
fn bps_delta_rounds_to_two_significant_figures() {
    let mut buffer = String::new();
    write_bps_delta(&mut buffer, 2, BTC_MID_HALF_TICKS);
    assert_eq!(buffer, "0.00085", "one tick, rounded from 0.00084746");
    write_bps_delta(&mut buffer, 4, BTC_MID_HALF_TICKS);
    assert_eq!(
        buffer, "0.0017",
        "a two-tick quote offset, rounded from 0.0016949 — and still distinct from one tick"
    );
    write_bps_delta(&mut buffer, 2_360, BTC_MID_HALF_TICKS);
    assert_eq!(buffer, "1.0", "one basis point exactly");
    write_bps_delta(&mut buffer, 23_600, BTC_MID_HALF_TICKS);
    assert_eq!(buffer, "10", "ten basis points needs no decimal at all");
    write_bps_delta(&mut buffer, 200_000, BTC_MID_HALF_TICKS);
    assert_eq!(buffer, "85", "rounded from 84.746");
}

/// A quote resting at the mid must read the same in either unit, or flipping the DOM's toggle would
/// look like the quote moved.
#[test]
fn a_zero_bps_delta_is_a_bare_zero() {
    let mut buffer = String::new();
    write_bps_delta(&mut buffer, 0, BTC_MID_HALF_TICKS);
    assert_eq!(buffer, "0");
    assert_ne!(buffer, MISSING, "a real zero distance is not an absent one");

    let mut ticks = String::new();
    write_half_tick_delta(&mut ticks, 0);
    assert_eq!(buffer, ticks, "the same quote reads the same in both units");
}

/// The bound is stated rather than approximated: below the rendered floor a run of zeros would read
/// as "at the mid", and past the cell's integer width a clamped number would read as a real distance.
#[test]
fn bps_delta_states_a_bound_rather_than_a_wrong_number() {
    let mut buffer = String::new();
    write_bps_delta(&mut buffer, 1, 100_000_000_000);
    assert_eq!(buffer, "0.0000001", "the floor itself still renders");
    write_bps_delta(&mut buffer, 1, 1_000_000_000_000);
    assert_eq!(buffer, "<1e-7", "below it the bound takes over");

    write_bps_delta(&mut buffer, 999_999_999, 10_000);
    assert_eq!(buffer, "999999999", "the widest integer form still renders");
    write_bps_delta(&mut buffer, 1_000_000_000_000_000, 1);
    assert_eq!(buffer, ">1e9");
}

#[test]
fn a_bps_delta_without_a_usable_ratio_is_missing() {
    let mut buffer = String::new();
    write_bps_delta(&mut buffer, 4, 0);
    assert_eq!(buffer, MISSING, "a non-positive mid forms no ratio");
    write_bps_delta(&mut buffer, 4, -1);
    assert_eq!(buffer, MISSING);
    write_bps_delta(&mut buffer, -4, BTC_MID_HALF_TICKS);
    assert_eq!(
        buffer, MISSING,
        "a distance is unsigned here; a signed bps convention would be an invention"
    );

    write_opt_bps_delta(&mut buffer, None, Some(BTC_MID_HALF_TICKS));
    assert_eq!(buffer, MISSING);
    write_opt_bps_delta(&mut buffer, Some(4), None);
    assert_eq!(buffer, MISSING);
    write_opt_bps_delta(&mut buffer, Some(4), Some(BTC_MID_HALF_TICKS));
    assert_eq!(buffer, "0.0017", "both present renders the value");
}

proptest! {
    /// FITNESS: the delta cell is nine characters wide at the minimum window size. A wider string
    /// does not truncate — it paints over the neighbouring cell, so the number the operator reads
    /// belongs to no column at all.
    #[test]
    fn a_bps_delta_never_exceeds_the_cell_width(
        delta in 0i64..=i64::MAX,
        mid in 1i64..=i64::MAX,
    ) {
        let mut buffer = String::new();
        write_bps_delta(&mut buffer, delta, mid);
        prop_assert!(
            buffer.chars().count() <= 9,
            "{buffer} is wider than the cell"
        );
    }
}

proptest! {
    /// FITNESS: a real distance never renders as a run of zeros. `0.00` on a quote two ticks off the
    /// mid says the quote is AT the mid, which is the one thing the cell exists to disprove — and
    /// the tick sizes that provoke it are ordinary, not extreme.
    #[test]
    fn a_non_zero_bps_delta_never_renders_as_all_zeros(
        delta in 1i64..=i64::MAX,
        mid in 1i64..=i64::MAX,
    ) {
        let mut buffer = String::new();
        write_bps_delta(&mut buffer, delta, mid);
        prop_assert!(
            buffer.bytes().any(|byte| byte.is_ascii_digit() && byte != b'0'),
            "a distance of {delta} half-ticks rendered as {buffer}"
        );
    }
}

#[test]
fn feature_value_renders_stable_decimals_and_literal_non_finites() {
    let mut buffer = String::new();
    write_feature_value(&mut buffer, 1.5);
    assert_eq!(buffer, "1.5000", "fixed decimals keep the column stable");
    write_feature_value(&mut buffer, 0.0);
    assert_eq!(buffer, "0.0000");
    assert_ne!(
        buffer, MISSING,
        "a real zero is distinct from a missing cell"
    );
    write_feature_value(&mut buffer, -0.5);
    assert_eq!(buffer, "-0.5000");

    write_feature_value(&mut buffer, f64::NAN);
    assert_eq!(buffer, "NaN");
    write_feature_value(&mut buffer, f64::INFINITY);
    assert_eq!(buffer, "inf");
    write_feature_value(&mut buffer, f64::NEG_INFINITY);
    assert_eq!(buffer, "-inf");
    assert_ne!("NaN", MISSING, "non-finite is distinguishable from missing");
}

/// The account band's asset rows resolve THROUGH the catalog's `base_asset`/`quote_asset`, never by
/// matching an asset name. A workstation attached over the link never sees a registry, so name
/// matching would be a second identity for the same thing — and the one number it would get wrong is
/// how much money the risk gate may spend.
#[test]
fn account_rows_resolve_the_instrument_to_its_two_assets() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(balance_event(0, AssetId(3), 4_210_000, 8_000));
    model.apply_event(balance_event(1, AssetId(7), 81_233_000_000, 946_000_000));
    // An asset the instrument does not name must not leak into either row.
    model.apply_event(balance_event(2, AssetId(9), 999, 999));

    let view = account(&model, InstrumentId(0), TICK);
    assert_eq!(view.base.label, "BTC");
    assert_eq!(view.quote.label, "USDT");
    assert_eq!(
        view.base
            .balance
            .map(|balance| (balance.free, balance.locked)),
        Some((4_210_000, 8_000))
    );
    assert_eq!(
        view.quote
            .balance
            .map(|balance| (balance.free, balance.locked)),
        Some((81_233_000_000, 946_000_000))
    );
    assert_eq!(
        view.quote.value,
        Some(82_179_000_000),
        "the quote row's value is its own total holding, free plus locked"
    );
    assert_eq!(
        view.base.value, None,
        "no book here, so there is no mid to value the base holding at"
    );
}

/// A 0.01 tick, so the fixtures below read as venue prices on a 118000 book rather than as tick
/// counts, and the valuation arithmetic is exercised at a real scale.
const CENT_TICK: Price = Price(1_000_000);

/// Best 117999.99 / 118000.01, a mid of exactly 118000.00.
fn btc_book(seq: u64, event_ts: i64) -> UiBookSnapshot {
    valid_book(0, seq, event_ts, 11_799_999_000_000, 11_800_001_000_000)
}

/// The base holding is valued at the SAME mid the MID cell shows, so the band and the summary can
/// never disagree about what the position is worth. Integers throughout, because a float is never
/// a money accumulator, and the two divisions fold into one so the value cannot double-round.
#[test]
fn base_holding_is_valued_at_the_book_mid() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    // 0.04210000 free + 0.00008000 locked = 0.04218 BTC held.
    model.apply_event(balance_event(0, AssetId(3), 4_210_000, 8_000));
    model.apply_book(btc_book(0, 1_000));

    let view = account(&model, InstrumentId(0), CENT_TICK);
    assert_eq!(
        view.base.value,
        Some(497_724_000_000),
        "0.04218 BTC at 118000 is 4977.24 USDT — free AND locked, because a coin held against a \
         resting order is still a coin you own"
    );
}

/// "I hold 0.04218 BTC" and "that is worth nothing" are different claims. A holding whose mid is
/// absent must blank its VALUE while keeping its BALANCE, or the band reports a flat book as a
/// worthless position.
#[test]
fn a_missing_mid_blanks_the_base_value_but_not_the_balance() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(balance_event(0, AssetId(3), 4_210_000, 8_000));

    let unpriced = account(&model, InstrumentId(0), CENT_TICK);
    assert_eq!(
        unpriced.base.balance.map(|balance| balance.free),
        Some(4_210_000),
        "the balance is known"
    );
    assert_eq!(
        unpriced.base.value, None,
        "its worth is not, and that is not 0"
    );

    // A one-sided book yields no mid either.
    model.apply_book(book(
        0,
        1,
        2_000,
        UiBookState::Valid,
        &[(11_799_999_000_000, 1)],
        &[],
    ));
    assert_eq!(
        account(&model, InstrumentId(0), CENT_TICK).base.value,
        None,
        "half a book is not a mid"
    );

    model.apply_book(btc_book(2, 3_000));
    assert_eq!(
        account(&model, InstrumentId(0), CENT_TICK).base.value,
        Some(497_724_000_000),
        "the value arrives with the mid, and the band's shape never moved"
    );
}

/// A clamped money figure is a WRONG money figure, and the operator has no way to tell it from a
/// real one. An unrepresentable valuation takes the same road as an absent one.
#[test]
fn a_valuation_out_of_i64_range_is_absent_not_clamped() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(balance_event(0, AssetId(3), i64::MAX / 2, i64::MAX / 2));
    model.apply_book(btc_book(0, 1_000));

    assert_eq!(account(&model, InstrumentId(0), CENT_TICK).base.value, None);
}

/// A balance nobody has reported is ABSENT, never zero. "You hold nothing" and "nobody has said" are
/// different claims, and the first is a reason to stop trading while the second is a reason to wait.
#[test]
fn an_unreported_balance_is_absent_not_zero() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_book(btc_book(0, 1_000));

    let view = account(&model, InstrumentId(0), CENT_TICK);
    assert_eq!(view.base.balance, None);
    assert_eq!(view.quote.balance, None);
    assert_eq!(
        view.base.value, None,
        "a priced mid cannot value a holding nobody has reported"
    );
    assert_eq!(view.quote.value, None);
    assert_eq!(
        view.halt, None,
        "no execution frame means no claim about the gate"
    );
}

/// Dust collapses onto one id at the edge, so its per-asset values cannot be told apart. The band
/// counts those balances rather than showing a number that would be the last dust asset's, wearing
/// every other one's name.
#[test]
fn untracked_asset_balances_are_counted_not_valued() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(balance_event(0, AssetId::UNKNOWN, 5, 0));
    model.apply_event(balance_event(1, AssetId::UNKNOWN, 11, 0));

    let view = account(&model, InstrumentId(0), TICK);
    assert_eq!(view.unknown_asset_balances, 2);
    assert_eq!(
        model.exec().balance(AssetId::UNKNOWN),
        None,
        "an untracked balance holds no value, because the value it would hold is not one asset's"
    );
}

/// `Unknown` is counted apart from the in-flight orders. Folding it in would claim a command is
/// outstanding when the truth is that we do not know whether the order exists at all — and those two
/// call for opposite operator responses.
#[test]
fn lost_orders_count_apart_from_in_flight_ones() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(order_event(0, 1, Side::Buy, OrderState::Live));
    model.apply_event(order_event(1, 2, Side::Buy, OrderState::CancelInFlight));
    model.apply_event(order_event(2, 3, Side::Buy, OrderState::Unknown));
    model.apply_event(order_event(3, 4, Side::Sell, OrderState::PendingNew));

    let view = account(&model, InstrumentId(0), TICK);
    assert_eq!(
        (view.bid.open, view.bid.in_flight, view.bid.lost),
        (1, 1, 1)
    );
    assert_eq!(
        (view.ask.open, view.ask.in_flight, view.ask.lost),
        (0, 1, 0)
    );
}

/// ACKs and stream reports can describe the same client order more than once. Identity, not event
/// count, drives both the account band and the DOM: repeated reports replace one cell.
#[test]
fn duplicate_order_reports_do_not_inflate_the_open_count() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(order_event(0, 41, Side::Buy, OrderState::PendingNew));
    model.apply_event(order_event(1, 41, Side::Buy, OrderState::Live));
    model.apply_event(order_event(2, 41, Side::Buy, OrderState::Live));

    let view = account(&model, InstrumentId(0), TICK);
    assert_eq!(
        (view.bid.open, view.bid.in_flight, view.bid.lost),
        (1, 0, 0)
    );
    assert_eq!(
        model.exec().working(InstrumentId(0), Side::Buy).len(),
        1,
        "the same client id must occupy exactly one projection cell"
    );
}

/// Once a side exceeds the projection's fixed detail capacity, the band must still count distinct
/// overflow identities rather than event frames. Otherwise an ACK plus the stream echo for one
/// leaked order says `+2 untracked`, and the alarm remains forever after its terminal update.
#[test]
fn duplicate_overflow_reports_count_one_untracked_order_until_terminal() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    for id in 1..=9 {
        model.apply_event(order_event(id - 1, id, Side::Buy, OrderState::Live));
    }
    assert_eq!(account(&model, InstrumentId(0), TICK).bid.leaked, 1);

    model.apply_event(order_event(9, 9, Side::Buy, OrderState::Live));
    assert_eq!(
        account(&model, InstrumentId(0), TICK).bid.leaked,
        1,
        "the ACK and stream echo name the same overflow identity"
    );

    model.apply_event(order_event(
        10,
        9,
        Side::Buy,
        OrderState::Closed(CloseReason::Filled),
    ));
    assert_eq!(
        account(&model, InstrumentId(0), TICK).bid.leaked,
        0,
        "a terminal report must retire the exact overflow identity"
    );
}

/// A gap in the shared event lane could be the terminal fill for any order already on screen. The
/// model used to log the gap but retain the stale order as confirmed, so the next real order made
/// the account band say `open 2` and the DOM painted both as live. It must keep the possible
/// exposure visible while withdrawing the confirmation claim.
#[test]
fn an_event_gap_demotes_pre_gap_orders_before_counting_a_replacement() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(order_event(0, 11, Side::Buy, OrderState::Live));

    // Sequence 1 is absent. It could have been order 11's terminal fill; sequence 2 is the
    // replacement the engine legitimately admitted after that fill.
    model.apply_event(order_event(2, 12, Side::Buy, OrderState::Live));

    let view = account(&model, InstrumentId(0), TICK);
    assert_eq!(
        (view.bid.open, view.bid.in_flight, view.bid.lost),
        (1, 0, 1),
        "only the post-gap update is confirmed; the stale cell is possible exposure, not `open`"
    );
    let orders = model.exec().working(InstrumentId(0), Side::Buy);
    assert_eq!(orders.len(), 2, "possible exposure stays visible");
    assert_eq!(
        orders
            .iter()
            .find(|order| order.client_id == ClientOrderId(11))
            .map(|order| order.status),
        Some(polysim::desktop::exec_model::OrderStatus::Lost)
    );
    assert_eq!(
        orders
            .iter()
            .find(|order| order.client_id == ClientOrderId(12))
            .map(|order| order.status),
        Some(polysim::desktop::exec_model::OrderStatus::Confirmed)
    );
}

/// The halt latch travels verbatim, reason and all. An operator reading `HALTED` needs to know which
/// halt it was — a reject streak and a realised loss call for entirely different next moves.
#[test]
fn the_halt_latch_travels_with_its_reason() {
    let mut model = model(1, 0);
    model.set_catalog(catalog_with_assets(AssetId(3), AssetId(7)));
    model.apply_event(UiEvent::Execution {
        seq: 0,
        event_ts_us: TsUs::from_micros(10),
        halt: ExecHalt::Armed,
    });
    assert_eq!(
        account(&model, InstrumentId(0), TICK).halt,
        Some(ExecHalt::Armed)
    );

    let halted = ExecHalt::Halted {
        reason: HaltReason::RealisedLoss,
        halted_ts_us: TsUs::from_micros(20),
    };
    model.apply_event(UiEvent::Execution {
        seq: 1,
        event_ts_us: TsUs::from_micros(20),
        halt: halted,
    });
    assert_eq!(account(&model, InstrumentId(0), TICK).halt, Some(halted));
}

/// A one-instrument catalog whose two assets are DISTINCT ids, so a projection that read one where it
/// meant the other would show the same balance twice instead of quietly passing.
fn catalog_with_assets(base_asset: AssetId, quote_asset: AssetId) -> UiCatalog {
    UiCatalog {
        strategy_id: "fitness".into(),
        window_title: "fitness".into(),
        execution_mode: Some(ExecutionMode::Live),
        spin_interval_us: SPIN.micros() as u64,
        instruments: vec![UiInstrument {
            instrument_id: InstrumentId(0),
            display: "BTCUSDT".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            base_asset,
            quote_asset,
            tick_size: Some(Price(1)),
            lot_size: Some(Qty(1)),
            qty_scale: 100_000_000,
        }],
        feature_names: Vec::new(),
    }
}

fn balance_event(seq: u64, asset: AssetId, free: i64, locked: i64) -> UiEvent {
    UiEvent::Balance {
        asset,
        seq,
        event_ts_us: TsUs::from_micros(seq as i64),
        free,
        locked,
    }
}

fn order_event(seq: u64, id: u64, side: Side, state: OrderState) -> UiEvent {
    UiEvent::OrderUpdate {
        instrument: InstrumentId(0),
        seq,
        event_ts_us: TsUs::from_micros(seq as i64),
        client_id: ClientOrderId(id),
        quote_level: None,
        side,
        state,
        price: Price(100),
        qty: Qty(5),
        filled: Qty(0),
    }
}

fn working_order(id: u64, state: OrderState) -> UiWorkingOrder {
    UiWorkingOrder {
        client_id: ClientOrderId(id),
        quote_level: None,
        state,
        price: Price(100 + id as i64),
        qty: Qty(5),
        filled: Qty(0),
    }
}

fn order_snapshot(seq: u64, side: Side, total_working: u16, details: &[UiWorkingOrder]) -> UiEvent {
    let mut orders = [UiWorkingOrder::EMPTY; UI_ORDER_SNAPSHOT_CAPACITY];
    orders[..details.len()].copy_from_slice(details);
    UiEvent::OrderSnapshot {
        instrument: InstrumentId(0),
        seq,
        event_ts_us: TsUs::from_micros(seq as i64),
        side,
        detail_len: details.len() as u8,
        total_working,
        orders,
    }
}

#[test]
fn first_order_snapshot_heals_a_late_attach_at_any_embedded_sequence() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        900,
        Side::Buy,
        1,
        &[working_order(41, OrderState::Live)],
    ));

    assert_eq!(model.event_gaps(), 0);
    let working = model.exec().working(InstrumentId(0), Side::Buy);
    assert_eq!(working.len(), 1);
    assert_eq!(working[0].client_id, ClientOrderId(41));
}

#[test]
fn complete_snapshot_retires_stale_cells_after_same_run_reconnect() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        0,
        Side::Buy,
        1,
        &[working_order(41, OrderState::Live)],
    ));
    model.apply_event(order_snapshot(
        50,
        Side::Buy,
        1,
        &[working_order(42, OrderState::Live)],
    ));

    assert_eq!(model.event_gaps(), 49);
    let working = model.exec().working(InstrumentId(0), Side::Buy);
    assert_eq!(working.len(), 1);
    assert_eq!(working[0].client_id, ClientOrderId(42));
}

#[test]
fn lost_snapshot_cannot_clear_visible_working_exposure() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        0,
        Side::Buy,
        1,
        &[working_order(41, OrderState::Live)],
    ));

    // Sequence 1 was the complete empty snapshot, but its whole datagram was lost.
    model.apply_event(balance_event(2, AssetId(3), 10, 0));
    let retained = model.exec().working(InstrumentId(0), Side::Buy);
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].client_id, ClientOrderId(41));

    model.apply_event(order_snapshot(3, Side::Buy, 0, &[]));
    assert!(model.exec().working(InstrumentId(0), Side::Buy).is_empty());
}

#[test]
fn snapshot_reports_exact_working_overflow_past_detail_capacity() {
    let mut model = model(1, 0);
    let details: [UiWorkingOrder; UI_ORDER_SNAPSHOT_CAPACITY] =
        core::array::from_fn(|index| working_order(index as u64 + 1, OrderState::Live));
    model.apply_event(order_snapshot(0, Side::Buy, 12, &details));

    assert_eq!(
        model.exec().working(InstrumentId(0), Side::Buy).len(),
        UI_ORDER_SNAPSHOT_CAPACITY
    );
    assert_eq!(
        model
            .exec()
            .side(InstrumentId(0), Side::Buy)
            .expect("snapshot created the side")
            .leaked(),
        4
    );
}

#[test]
fn invalid_in_process_snapshot_cannot_erase_a_valid_side() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        0,
        Side::Buy,
        1,
        &[working_order(41, OrderState::Live)],
    ));
    let duplicate = working_order(42, OrderState::Live);
    model.apply_event(order_snapshot(1, Side::Buy, 2, &[duplicate, duplicate]));

    let retained = model.exec().working(InstrumentId(0), Side::Buy);
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].client_id, ClientOrderId(41));
    assert_eq!(
        retained[0].status,
        polysim::desktop::exec_model::OrderStatus::Lost,
        "invalid authoritative input preserves possible exposure but cannot leave it confirmed"
    );
    assert_eq!(
        account(&model, InstrumentId(0), TICK).bid.leaked,
        1,
        "the cut declared two working identities but only one old identity can be retained"
    );
}

#[test]
fn empty_snapshots_clear_only_the_side_they_name() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        0,
        Side::Buy,
        1,
        &[working_order(51, OrderState::Live)],
    ));
    model.apply_event(order_snapshot(
        1,
        Side::Sell,
        1,
        &[working_order(52, OrderState::Live)],
    ));

    model.apply_event(order_snapshot(2, Side::Buy, 0, &[]));
    let halfway = account(&model, InstrumentId(0), TICK);
    assert_eq!(halfway.bid.open, 0);
    assert_eq!(halfway.ask.open, 1, "the BID cut must not clear the ASK");

    model.apply_event(order_snapshot(3, Side::Sell, 0, &[]));
    let cleared = account(&model, InstrumentId(0), TICK);
    assert_eq!((cleared.bid.open, cleared.ask.open), (0, 0));
}

#[test]
fn after_a_gap_only_the_side_with_a_complete_snapshot_heals() {
    let mut model = model(1, 0);
    model.apply_event(order_event(0, 61, Side::Buy, OrderState::Live));
    model.apply_event(order_event(1, 62, Side::Sell, OrderState::Live));

    // Sequence 2 was lost. Gap handling demotes both old sides before this complete BID cut applies.
    model.apply_event(order_snapshot(
        3,
        Side::Buy,
        1,
        &[working_order(63, OrderState::Live)],
    ));
    let halfway = account(&model, InstrumentId(0), TICK);
    assert_eq!(
        (halfway.bid.open, halfway.bid.lost),
        (1, 0),
        "the current complete BID cut heals atomically"
    );
    assert_eq!(
        (halfway.ask.open, halfway.ask.lost),
        (0, 1),
        "the pre-gap ASK cannot remain confirmed until its own cut arrives"
    );

    model.apply_event(order_snapshot(4, Side::Sell, 0, &[]));
    let healed = account(&model, InstrumentId(0), TICK);
    assert_eq!(
        (healed.ask.open, healed.ask.in_flight, healed.ask.lost),
        (0, 0, 0)
    );
}

#[test]
fn snapshots_and_transitions_follow_the_shared_lane_order() {
    let mut model = model(1, 0);
    model.apply_event(order_snapshot(
        0,
        Side::Buy,
        1,
        &[working_order(71, OrderState::Live)],
    ));
    model.apply_event(order_event(1, 71, Side::Buy, OrderState::CancelInFlight));
    let transitioned = account(&model, InstrumentId(0), TICK).bid;
    assert_eq!((transitioned.open, transitioned.in_flight), (0, 1));

    // A later absolute cut wins over that transition.
    model.apply_event(order_snapshot(
        2,
        Side::Buy,
        1,
        &[working_order(71, OrderState::Live)],
    ));
    let snapped = account(&model, InstrumentId(0), TICK).bid;
    assert_eq!((snapped.open, snapped.in_flight), (1, 0));

    // A still-later terminal transition clears immediately; it need not wait for another spin.
    model.apply_event(order_event(
        3,
        71,
        Side::Buy,
        OrderState::Closed(CloseReason::Filled),
    ));
    assert_eq!(account(&model, InstrumentId(0), TICK).bid.open, 0);
    assert!(model.exec().working(InstrumentId(0), Side::Buy).is_empty());
}

/// Every declared channel owns a scroll slot. The scroll state is a fixed array, and only two of the
/// three places that size it are compiler-checked when a channel is added — the third would index
/// past the end and panic on the UI thread the first time an operator clicked the new tab.
#[test]
fn every_channel_has_its_own_scroll_slot() {
    let mut state = MonitorUiState::new();
    for (index, channel) in Channel::ALL.iter().enumerate() {
        state.set_scrolled_away(*channel, index as u64, index as f32);
        state.active_tab = *channel;
    }
    assert_eq!(state.active_tab, *Channel::ALL.last().expect("no channels"));
}
