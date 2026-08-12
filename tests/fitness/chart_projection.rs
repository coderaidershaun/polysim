//! Chart projection fitness (chunk C1): the pure map from the ordered book and event lanes to the
//! rolling five-minute mid series, and the screen transform over it. Buckets are folded in event time
//! alone, so a spin carrying no trustworthy mid stays a real hole the line splits across rather
//! than an invented sample, and replay reproduces the series exactly. Prices never leave exact
//! half-tick integers until the final `0..=1` fraction. The domain grows left → right before it
//! slides; the bounds cover fills as well as mids, so an off-mid marker can never be invisible.

use polysim::desktop::chart_model::{
    BookContinuity, ChartBucket, ChartFill, ChartModel, bucket_open_ts,
};
use polysim::desktop::chart_view::{
    ChartBounds, ChartDomain, bounds, domain, segment_points, visible_fills, x_fraction, y_fraction,
};
use polysim::ids::{AssetId, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::Liquidity;
use polysim::msg::inbound::Level;
use polysim::msg::ui::{UI_BOOK_LEVELS, UiBookSnapshot, UiBookState, UiEvent};
use polysim::time::{DurationUs, TsUs};
use proptest::prelude::*;

/// One-mantissa tick, so a price mantissa reads directly as its tick index and a mid reads as the
/// plain sum of the two best indices (half-ticks).
const TICK: Price = Price(1);

/// The shipped one-second spin, so the five-minute window is exactly 300 buckets and a bucket index
/// reads as its second.
const SPIN: DurationUs = DurationUs::from_micros(1_000_000);

/// The window at [`SPIN`]: `300_000_000 / 1_000_000`.
const CAPACITY: u64 = 300;

/// Buckets committed past a full window, so eviction and sliding are exercised by an amount rather
/// than by a literal that would silently mean something else if the window span moved.
const OVERRUN: u64 = 100;

fn chart_with_ticks(ticks: &[Option<Price>]) -> ChartModel {
    let mut chart = ChartModel::with_capacity(ticks.len(), SPIN);
    chart.configure(ticks, SPIN);
    chart
}

fn chart(instruments: usize) -> ChartModel {
    chart_with_ticks(&vec![Some(TICK); instruments])
}

fn book(
    instrument: u16,
    bucket: u64,
    state: UiBookState,
    bids: &[i64],
    asks: &[i64],
) -> UiBookSnapshot {
    let empty = Level {
        price: Price(0),
        qty: Qty(0),
    };
    let mut bid_levels = [empty; UI_BOOK_LEVELS];
    let mut ask_levels = [empty; UI_BOOK_LEVELS];
    for (slot, &price) in bid_levels.iter_mut().zip(bids) {
        *slot = Level {
            price: Price(price),
            qty: Qty(1),
        };
    }
    for (slot, &price) in ask_levels.iter_mut().zip(asks) {
        *slot = Level {
            price: Price(price),
            qty: Qty(1),
        };
    }
    UiBookSnapshot {
        instrument: InstrumentId(instrument),
        seq: 0,
        event_ts_us: TsUs::from_micros(bucket as i64 * SPIN.micros()),
        state,
        bid_len: bids.len() as u16,
        ask_len: asks.len() as u16,
        bids: bid_levels,
        asks: ask_levels,
    }
}

fn valid_book(instrument: u16, bucket: u64, bid: i64, ask: i64) -> UiBookSnapshot {
    book(instrument, bucket, UiBookState::Valid, &[bid], &[ask])
}

/// Fold one two-sided commit into `bucket` with no snapshot loss before it — the ordinary case.
fn commit(chart: &mut ChartModel, instrument: u16, bucket: u64, bid: i64, ask: i64) {
    chart.apply_book(
        &valid_book(instrument, bucket, bid, ask),
        BookContinuity::Continuous,
    );
}

fn fill_event(instrument: u16, bucket: u64, side: Side, price: i64) -> UiEvent {
    UiEvent::Fill {
        instrument: InstrumentId(instrument),
        seq: 0,
        event_ts_us: TsUs::from_micros(bucket as i64 * SPIN.micros()),
        quote_level: None,
        side,
        price: Price(price),
        qty: Qty(1),
        commission: 0,
        commission_asset: AssetId(0),
        liquidity: Some(Liquidity::Maker),
    }
}

fn rotation_event(instrument: u16, bucket: u64) -> UiEvent {
    UiEvent::Rotation {
        instrument: InstrumentId(instrument),
        seq: 0,
        event_ts_us: TsUs::from_micros(bucket as i64 * SPIN.micros()),
    }
}

fn buckets(chart: &ChartModel, instrument: u16) -> Vec<ChartBucket> {
    chart.buckets(InstrumentId(instrument)).copied().collect()
}

/// FITNESS: the instant a bucket opened folds back into that same bucket. The crosshair's time label
/// is the inverse of the model's bucketing, and the two are separate functions, so nothing but this
/// keeps them describing one axis — an anchor or offset added to the bucketing alone would leave the
/// hairline naming the wrong instant, silently and over live data, in the one artifact nobody can
/// check by eye. Driven through `apply_book` rather than against the bucketing directly, so what is
/// pinned is the path the painter actually runs.
#[test]
fn a_bucket_open_time_folds_back_into_its_own_bucket() {
    // Zero, the first few, a window edge, and epoch scale — ~1.75e9 at the shipped one-second spin.
    for index in [0u64, 1, 7, CAPACITY - 1, 1_000_000, 1_750_000_000] {
        let at = bucket_open_ts(index, SPIN).expect("the shipped cadence has an instant");
        let mut chart = chart(1);
        let mut snapshot = valid_book(0, 0, 100, 102);
        snapshot.event_ts_us = at;
        chart.apply_book(&snapshot, BookContinuity::Continuous);
        assert_eq!(
            indices(&chart, 0),
            vec![index],
            "bucket {index} opened at {}us and the model bucketed that instant elsewhere",
            at.micros()
        );
    }
    assert_eq!(
        bucket_open_ts(1, DurationUs::ZERO),
        None,
        "a cadence the model never took has no instant to report rather than a fabricated one"
    );
    assert_eq!(
        bucket_open_ts(u64::MAX, SPIN),
        None,
        "and an index whose instant leaves i64 reports none rather than wrapping"
    );
}

fn fills(chart: &ChartModel, instrument: u16) -> Vec<ChartFill> {
    chart.fills(InstrumentId(instrument)).copied().collect()
}

fn indices(chart: &ChartModel, instrument: u16) -> Vec<u64> {
    chart
        .buckets(InstrumentId(instrument))
        .map(|b| b.index)
        .collect()
}

/// The `is_run_start` flag of every visible bucket, in paint order — where the painted line breaks.
fn run_starts(chart: &ChartModel, instrument: u16) -> Vec<bool> {
    let instrument = InstrumentId(instrument);
    let domain = domain(chart, instrument).expect("a series to project");
    segment_points(chart, instrument, domain)
        .map(|point| point.is_run_start)
        .collect()
}

#[test]
fn commits_inside_one_spin_fold_into_one_open_high_low_close() {
    let mut chart = chart(1);
    for (bid, ask) in [(100, 102), (105, 107), (95, 97), (101, 103)] {
        commit(&mut chart, 0, 1, bid, ask);
    }

    assert_eq!(
        buckets(&chart, 0),
        vec![ChartBucket {
            index: 1,
            open_half_ticks: 202,
            high_half_ticks: 212,
            low_half_ticks: 192,
            close_half_ticks: 204,
            has_gap_before: false,
        }],
        "four commits in one spin are one bucket: first mid opens, extremes bound it, last closes"
    );
}

#[test]
fn the_next_spin_pushes_a_new_bucket() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);
    // Still inside spin 1: 1_999_999 µs floors to bucket 1, not 2.
    chart.apply_book(
        &UiBookSnapshot {
            event_ts_us: TsUs::from_micros(1_999_999),
            ..valid_book(0, 1, 124, 126)
        },
        BookContinuity::Continuous,
    );
    commit(&mut chart, 0, 2, 110, 112);

    let banked = buckets(&chart, 0);
    assert_eq!(indices(&chart, 0), vec![1, 2]);
    assert_eq!(
        banked[0].high_half_ticks, 250,
        "the last microsecond of a spin is its own bucket's"
    );
    assert_eq!(banked[0].close_half_ticks, 250);
    assert_eq!(
        (
            banked[1].open_half_ticks,
            banked[1].high_half_ticks,
            banked[1].low_half_ticks,
            banked[1].close_half_ticks
        ),
        (222, 222, 222, 222),
        "a fresh bucket opens flat on its first mid"
    );
}

#[test]
fn out_of_order_event_time_never_rewrites_a_settled_bucket() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 2, 100, 102);
    commit(&mut chart, 0, 3, 149, 151);
    let settled = buckets(&chart, 0);

    // A snapshot stamped back inside bucket 2, with an extreme mid that would be visible if folded.
    commit(&mut chart, 0, 2, 500, 500);

    assert_eq!(
        buckets(&chart, 0),
        settled,
        "a late snapshot for a settled bucket is dropped, never folded backwards"
    );
}

#[test]
fn a_bucket_past_the_window_is_still_retained_but_leaves_the_domain() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 0, 100, 102);
    commit(&mut chart, 0, 5_000, 110, 112);

    assert_eq!(
        indices(&chart, 0),
        vec![0, 5_000],
        "two buckets is far under capacity, so nothing was evicted"
    );
    let domain = domain(&chart, InstrumentId(0)).expect("a series to project");
    assert_eq!(
        domain,
        ChartDomain {
            first: 5_000 - CAPACITY + 1,
            last: 5_000
        }
    );
    let visible: Vec<u64> = segment_points(&chart, InstrumentId(0), domain)
        .map(|point| point.bucket.index)
        .collect();
    assert_eq!(
        visible,
        vec![5_000],
        "eviction is by capacity, visibility by the five-minute window — the old bucket falls out of \
         the second, not the first"
    );
}

#[test]
fn a_spin_without_a_trustworthy_mid_stays_a_hole() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);
    commit(&mut chart, 0, 2, 101, 103);
    // Spin 3 commits, but the book is awaiting its snapshot: nothing is banked for it.
    chart.apply_book(
        &book(0, 3, UiBookState::AwaitingSnapshot, &[], &[]),
        BookContinuity::Continuous,
    );
    commit(&mut chart, 0, 4, 105, 107);

    assert_eq!(
        indices(&chart, 0),
        vec![1, 2, 4],
        "the missing spin is absent, never interpolated or carried forward"
    );
    assert_eq!(
        run_starts(&chart, 0),
        vec![true, false, true],
        "the line breaks across the hole"
    );
}

#[test]
fn a_lost_snapshot_run_splits_the_line_at_the_next_bucket() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);
    commit(&mut chart, 0, 2, 101, 103);
    // The lane dropped snapshots, revealed on a one-sided commit that banks nothing itself.
    chart.apply_book(
        &book(0, 3, UiBookState::Valid, &[100], &[]),
        BookContinuity::GapBefore,
    );
    commit(&mut chart, 0, 3, 104, 106);

    let banked = buckets(&chart, 0);
    assert!(!banked[1].has_gap_before);
    assert!(
        banked[2].has_gap_before,
        "the loss latches until the next bucket is actually pushed"
    );
    assert_eq!(
        run_starts(&chart, 0),
        vec![true, false, true],
        "bucket 3 follows bucket 2 in index, yet the flag still splits the run"
    );
}

#[test]
fn a_loss_inside_the_open_bucket_splits_the_line_before_that_bucket() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);
    commit(&mut chart, 0, 2, 101, 103);
    // The ordinary case: the book commits many times per spin, so the commit that reveals the loss
    // folds into the already-open bucket instead of pushing a new one.
    chart.apply_book(&valid_book(0, 2, 104, 106), BookContinuity::GapBefore);
    commit(&mut chart, 0, 3, 105, 107);

    let banked = buckets(&chart, 0);
    assert!(
        banked[1].has_gap_before,
        "the samples lost were bucket 2's own, so bucket 2 is the incomplete one"
    );
    assert!(
        !banked[2].has_gap_before,
        "and the loss does not ride onto a bucket that lost nothing"
    );
    assert_eq!(
        run_starts(&chart, 0),
        vec![true, true, false],
        "the break lands at the boundary before the incomplete bucket, not after it"
    );
}

#[test]
fn a_rotation_clears_that_instrument_and_leaves_its_neighbour() {
    let mut chart = chart(2);
    commit(&mut chart, 0, 1, 100, 102);
    commit(&mut chart, 1, 1, 300, 302);
    chart.apply_event(&fill_event(0, 1, Side::Buy, 101));
    chart.apply_event(&fill_event(1, 1, Side::Buy, 301));

    chart.apply_event(&rotation_event(0, 2));

    assert!(
        buckets(&chart, 0).is_empty(),
        "a new window is a new distribution"
    );
    assert!(fills(&chart, 0).is_empty(), "its markers go with it");
    assert_eq!(indices(&chart, 1), vec![1], "the neighbour is untouched");
    assert_eq!(fills(&chart, 1).len(), 1);
}

#[test]
fn only_a_valid_two_sided_on_grid_book_banks_a_mid() {
    let mut chart = chart(1);
    chart.apply_book(
        &book(0, 1, UiBookState::AwaitingSnapshot, &[100], &[102]),
        BookContinuity::Continuous,
    );
    chart.apply_book(
        &book(0, 2, UiBookState::Valid, &[100], &[]),
        BookContinuity::Continuous,
    );
    chart.apply_book(
        &book(0, 3, UiBookState::Valid, &[], &[102]),
        BookContinuity::Continuous,
    );
    assert!(
        buckets(&chart, 0).is_empty(),
        "an awaiting or one-sided book has no mid to bank"
    );

    // A ten-mantissa grid: 105 is not a multiple of the tick, so the mid does not exist.
    let mut off_grid = chart_with_ticks(&[Some(Price(10))]);
    commit(&mut off_grid, 0, 1, 100, 105);
    assert!(
        buckets(&off_grid, 0).is_empty(),
        "an off-grid best price is invalid, never rounded onto the grid"
    );
    commit(&mut off_grid, 0, 2, 100, 110);
    assert_eq!(
        buckets(&off_grid, 0)[0].close_half_ticks,
        21,
        "on-grid, the mid is the exact sum of the two tick indices"
    );
}

#[test]
fn a_fill_marks_its_own_price_never_the_mid() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);
    chart.apply_event(&fill_event(0, 1, Side::Sell, 150));

    assert_eq!(
        fills(&chart, 0),
        vec![ChartFill {
            index: 1,
            half_ticks: 300,
            side: Side::Sell,
        }],
        "the marker sits at the fill's own price in half-ticks, in its own bucket"
    );
    assert_eq!(
        buckets(&chart, 0)[0].close_half_ticks,
        202,
        "and the fill leaves the mid series alone"
    );
}

#[test]
fn a_whole_window_of_fills_keeps_the_marker_under_the_oldest_drawn_bucket() {
    // The 100 ms spin `MAX_BUCKETS_PER_INSTRUMENT` is sized for — the cadence at which a fill ring
    // unrelated to the bucket ring starts evicting markers from under a line that is still drawn.
    const FAST_SPIN: DurationUs = DurationUs::from_micros(100_000);
    let mut chart = ChartModel::with_capacity(1, FAST_SPIN);
    chart.configure(&[Some(TICK)], FAST_SPIN);
    let at = |bucket: u64| TsUs::from_micros(bucket as i64 * FAST_SPIN.micros());

    // Two fills per bucket is the run's real ceiling: the recorder arms one quote per side per spin
    // and a markout side disarms itself on fill.
    let window = chart.capacity() as u64;
    for bucket in 0..window {
        chart.apply_book(
            &UiBookSnapshot {
                event_ts_us: at(bucket),
                ..valid_book(0, 0, 100, 102)
            },
            BookContinuity::Continuous,
        );
        for side in [Side::Buy, Side::Sell] {
            chart.apply_event(&UiEvent::Fill {
                instrument: InstrumentId(0),
                seq: 0,
                event_ts_us: at(bucket),
                quote_level: None,
                side,
                price: Price(101),
                qty: Qty(1),
                commission: 0,
                commission_asset: AssetId(0),
                liquidity: Some(Liquidity::Maker),
            });
        }
    }

    let domain = domain(&chart, InstrumentId(0)).expect("a series to project");
    let visible: Vec<ChartFill> = visible_fills(&chart, InstrumentId(0), domain)
        .copied()
        .collect();
    assert_eq!(
        visible.len() as u64,
        2 * window,
        "a whole window of fills at the run's real rate, none of them evicted"
    );
    assert_eq!(
        visible[0].index,
        buckets(&chart, 0)[0].index,
        "so no bucket is ever drawn with its own markers already gone — which would read as a \
         strategy that did not trade"
    );
}

#[test]
fn an_instrument_without_a_tick_grid_charts_nothing() {
    let mut chart = chart_with_ticks(&[None]);
    commit(&mut chart, 0, 1, 100, 102);
    chart.apply_event(&fill_event(0, 1, Side::Buy, 101));

    assert!(buckets(&chart, 0).is_empty());
    assert!(fills(&chart, 0).is_empty());
    assert_eq!(
        domain(&chart, InstrumentId(0)),
        None,
        "no grid, no series — an honest empty, never a fabricated one"
    );
}

#[test]
fn the_domain_grows_from_the_left_then_slides_as_the_series_evicts() {
    let mut chart = chart(1);
    assert_eq!(chart.capacity() as u64, CAPACITY);
    for bucket in 1..=3 {
        commit(&mut chart, 0, bucket, 100, 102);
    }
    let growing = domain(&chart, InstrumentId(0)).expect("a series to project");
    assert_eq!(
        growing,
        ChartDomain {
            first: 1,
            last: CAPACITY
        },
        "the window is anchored at run start while it fills, so the line grows left → right"
    );
    assert_eq!(growing.width(), CAPACITY);

    for bucket in 4..=CAPACITY + OVERRUN {
        commit(&mut chart, 0, bucket, 100, 102);
    }
    let sliding = domain(&chart, InstrumentId(0)).expect("a series to project");
    assert_eq!(
        sliding,
        ChartDomain {
            first: OVERRUN + 1,
            last: CAPACITY + OVERRUN
        },
        "once full, the window slides so the newest bucket holds the right edge"
    );
    assert_eq!(sliding.width(), CAPACITY);

    let banked = buckets(&chart, 0);
    assert_eq!(banked.len(), CAPACITY as usize);
    assert_eq!(
        (banked[0].index, banked[banked.len() - 1].index),
        (OVERRUN + 1, CAPACITY + OVERRUN),
        "the series retains exactly its capacity, oldest evicted first, iterated oldest → newest"
    );
}

#[test]
fn bounds_span_the_extremes_and_swallow_the_fills() {
    let mut chart = chart(1);
    for (bid, ask) in [(100, 102), (109, 111), (89, 91), (99, 101)] {
        commit(&mut chart, 0, 1, bid, ask);
    }
    chart.apply_event(&fill_event(0, 1, Side::Sell, 150));

    let domain = domain(&chart, InstrumentId(0)).expect("a series to project");
    let bounds = bounds(&chart, InstrumentId(0), domain).expect("visible data to bound");
    assert_eq!(
        bounds,
        ChartBounds {
            low: 174,
            high: 306
        },
        "low 180 / high 300 (the fill above every mid), then 5% air each side"
    );
    assert!(
        bounds.low < 200 && bounds.high > 220,
        "extremes bound the axis, never the closes — so switching Line ↔ Candles cannot rescale y"
    );
}

#[test]
fn a_flat_series_keeps_a_finite_non_zero_range() {
    let mut chart = chart(1);
    commit(&mut chart, 0, 1, 100, 102);

    let domain = domain(&chart, InstrumentId(0)).expect("a series to project");
    let bounds = bounds(&chart, InstrumentId(0), domain).expect("visible data to bound");
    assert_eq!(
        bounds,
        ChartBounds {
            low: 201,
            high: 203
        },
        "an equal min and max expand by one whole tick, so the transform is never zero-height"
    );
    assert_eq!(
        y_fraction(202, bounds),
        0.5,
        "the constant line sits mid-plot"
    );
}

#[test]
fn fractions_stay_in_range_and_move_monotonically() {
    let domain = ChartDomain {
        first: 100,
        last: 700,
    };
    assert_eq!(x_fraction(100, domain), 0.0);
    assert_eq!(x_fraction(700, domain), 1.0);
    assert_eq!(x_fraction(400, domain), 0.5);
    assert_eq!(
        x_fraction(50, domain),
        0.0,
        "an index below the window clamps"
    );
    assert_eq!(x_fraction(9_999, domain), 1.0, "and above it clamps too");

    let bounds = ChartBounds {
        low: 100,
        high: 200,
    };
    assert_eq!(y_fraction(100, bounds), 0.0, "0.0 is the low bound");
    assert_eq!(y_fraction(200, bounds), 1.0);
    assert_eq!(y_fraction(150, bounds), 0.5);
    assert_eq!(y_fraction(-5_000, bounds), 0.0);
    assert_eq!(y_fraction(5_000, bounds), 1.0);
    assert_eq!(
        y_fraction(5, ChartBounds { low: 5, high: 5 }),
        0.5,
        "a zero-height window reads as its own centre rather than dividing by zero"
    );
}

#[test]
fn the_projection_is_replay_deterministic() {
    let run = || {
        let mut chart = chart(2);
        commit(&mut chart, 0, 1, 100, 102);
        commit(&mut chart, 1, 1, 300, 302);
        commit(&mut chart, 0, 1, 105, 107);
        chart.apply_book(
            &book(0, 2, UiBookState::AwaitingSnapshot, &[], &[]),
            BookContinuity::GapBefore,
        );
        commit(&mut chart, 0, 3, 98, 100);
        chart.apply_event(&fill_event(0, 3, Side::Buy, 99));
        chart.apply_event(&rotation_event(1, 3));
        let instrument = InstrumentId(0);
        let domain = domain(&chart, instrument);
        (
            buckets(&chart, 0),
            fills(&chart, 0),
            buckets(&chart, 1),
            domain,
            domain.and_then(|domain| bounds(&chart, instrument, domain)),
            domain.map(|domain| {
                visible_fills(&chart, instrument, domain)
                    .copied()
                    .collect::<Vec<_>>()
            }),
        )
    };
    assert_eq!(
        run(),
        run(),
        "the same input sequence yields the same series, domain and bounds"
    );
}

proptest! {
    /// A series that projects always bounds. The painter reads the two together, so a domain without
    /// bounds paints "no mid samples yet" over live data — the chart's own honest-empty state turned
    /// into a lie.
    #[test]
    fn a_series_with_a_domain_always_has_bounds(
        steps in prop::collection::vec(0u64..2_000, 1..40),
    ) {
        let mut chart = chart(1);
        let mut bucket = 0u64;
        for step in steps {
            bucket += step;
            commit(&mut chart, 0, bucket, 100, 102);
        }

        let domain = domain(&chart, InstrumentId(0)).expect("a commit banks a bucket");
        prop_assert!(bounds(&chart, InstrumentId(0), domain).is_some());
    }

    /// Prices round-trip through the y transform, monotonically and inside `0..=1`.
    #[test]
    fn prices_round_trip_through_the_y_transform(
        low in -1_000_000i64..1_000_000,
        span in 1i64..10_000,
        offset in 0i64..10_000,
    ) {
        let bounds = ChartBounds { low, high: low + span };
        let price = low + offset.min(span);
        let fraction = y_fraction(price, bounds);
        prop_assert!((0.0..=1.0).contains(&fraction));
        prop_assert!(fraction >= y_fraction(price - 1, bounds));
        prop_assert_eq!(low + (f64::from(fraction) * span as f64).round() as i64, price);
    }
}
