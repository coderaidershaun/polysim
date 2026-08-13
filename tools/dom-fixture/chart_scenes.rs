//! Deterministic chart scenes: ONLY place fabricated data lives. No RNG -> reproducible.

use polysim::desktop::chart_model::{BookContinuity, ChartModel};
use polysim::ids::{AssetId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::Liquidity;
use polysim::msg::inbound::Level;
use polysim::msg::ui::{UI_BOOK_LEVELS, UiBookSnapshot, UiBookState, UiEvent};
use polysim::time::DurationUs;
use polysim::time::TsUs;

const SPIN: DurationUs = DurationUs::from_micros(1_000_000);
const WINDOW_BUCKETS: i64 = 300;

const COARSE_SPIN: DurationUs = DurationUs::from_micros(15_000_000);
const COARSE_WINDOW_BUCKETS: i64 = 20;

const COMMITS_PER_BUCKET: i64 = 4;

const TICK: Price = Price(FIXED_SCALE);
const INSTRUMENT: InstrumentId = InstrumentId(0);
const BASE_TS: i64 = 1_753_300_000_000_000;
const BASE_BID: i64 = 65_990;

/// Scene: model, instrument, grid. tick = Price(0) if no venue grid.
pub struct ChartScene {
    pub name: &'static str,
    pub chart: ChartModel,
    pub instrument: InstrumentId,
    pub tick: Price,
}

pub fn chart_scenes() -> Vec<ChartScene> {
    vec![
        growing(),
        full_window(),
        holes(),
        flat(),
        one_sided(),
        no_tick_grid(),
        dense_fills(),
        fill_off_mid(),
        coarse_buckets(),
    ]
}

/// Grows left edge (grow-then-slide). Candles = full_window width.
fn growing() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..90);
    feed.fill(30, Side::Buy, BASE_BID);
    feed.fill(62, Side::Sell, BASE_BID + 2);
    feed.finish("growing — 90 s of 300")
}

/// 5-min window past: domain slides, hairline density.
fn full_window() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..WINDOW_BUCKETS + 120);
    for step in (40..WINDOW_BUCKETS + 120).step_by(90) {
        feed.fill(step, side_for(step), mid_bid(step));
    }
    feed.finish("full window — sliding")
}

/// Two indistinguishable breaks: no mid, lane-lost snapshots.
fn holes() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..120);
    feed.walk(150..240);
    feed.gap();
    feed.walk(240..300);
    feed.finish("holes — missing seconds + lane gap")
}

/// Flat series: guard expands equal high/low (avoids zero-div).
fn flat() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    for step in 0..240 {
        feed.still(step, BASE_BID);
    }
    feed.finish("flat series")
}

/// Bid-only: no mid, chart reports it.
fn one_sided() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    for step in 0..120 {
        feed.bid_only(step);
    }
    feed.finish("one-sided book — no mid")
}

/// Polymarket: no tick, no grid.
fn no_tick_grid() -> ChartScene {
    let mut feed = Feed::new(None);
    feed.walk(0..120);
    ChartScene {
        name: "no tick grid",
        chart: feed.chart,
        instrument: INSTRUMENT,
        tick: Price(0),
    }
}

/// Fills both sides: markers separable from mid.
fn dense_fills() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..120);
    for step in 0..120 {
        feed.fill(step, Side::Buy, mid_bid(step) - 3);
        feed.fill(step, Side::Sell, mid_bid(step) + 5);
    }
    feed.finish("dense fills both sides")
}

/// Fills outside mid range: bounds union -> markers on-screen.
fn fill_off_mid() -> ChartScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..200);
    feed.fill(45, Side::Buy, BASE_BID - 60);
    feed.fill(120, Side::Sell, BASE_BID + 70);
    feed.fill(170, Side::Buy, BASE_BID - 40);
    feed.finish("fills outside the mid range")
}

/// Full window at COARSE: legible body/wicks, period retuning compare.
fn coarse_buckets() -> ChartScene {
    let mut feed = Feed::with_spin(Some(TICK), COARSE_SPIN);
    feed.walk(0..COARSE_WINDOW_BUCKETS);
    feed.fill(11, Side::Buy, mid_bid(11));
    feed.fill(28, Side::Sell, mid_bid(28) + 2);
    feed.finish("coarse 15 s buckets — readable candles")
}

struct Feed {
    chart: ChartModel,
    spin: DurationUs,
    seq: u64,
    gap_next: bool,
}

impl Feed {
    fn new(tick: Option<Price>) -> Self {
        Self::with_spin(tick, SPIN)
    }

    fn with_spin(tick: Option<Price>, spin: DurationUs) -> Self {
        let mut chart = ChartModel::with_capacity(1, spin);
        chart.configure(&[tick], spin);
        Self {
            chart,
            spin,
            seq: 0,
            gap_next: false,
        }
    }

    fn walk(&mut self, steps: std::ops::Range<i64>) {
        for step in steps {
            self.bucket(step, mid_bid(step));
        }
    }

    fn bucket(&mut self, step: i64, bid: i64) {
        for sub in 0..COMMITS_PER_BUCKET {
            let continuity = self.take_continuity();
            let bid = bid + wobble(step * COMMITS_PER_BUCKET + sub);
            self.commit(self.at(step, sub), &[bid], &[bid + 2], continuity);
        }
    }

    /// Degenerate candle: open == high == low == close, tests bounds guard on zero-span series.
    fn still(&mut self, step: i64, bid: i64) {
        let continuity = self.take_continuity();
        self.commit(self.at(step, 0), &[bid], &[bid + 2], continuity);
    }

    fn bid_only(&mut self, step: i64) {
        let continuity = self.take_continuity();
        self.commit(self.at(step, 0), &[mid_bid(step)], &[], continuity);
    }

    fn at(&self, step: i64, sub: i64) -> i64 {
        BASE_TS + step * self.spin.micros() + sub * self.spin.micros() / COMMITS_PER_BUCKET
    }

    fn gap(&mut self) {
        self.gap_next = true;
    }

    fn take_continuity(&mut self) -> BookContinuity {
        if std::mem::take(&mut self.gap_next) {
            BookContinuity::GapBefore
        } else {
            BookContinuity::Continuous
        }
    }

    fn commit(&mut self, micros: i64, bids: &[i64], asks: &[i64], continuity: BookContinuity) {
        self.seq += 1;
        self.chart
            .apply_book(&snapshot(self.seq, micros, bids, asks), continuity);
    }

    fn fill(&mut self, step: i64, side: Side, tick: i64) {
        self.chart.apply_event(&UiEvent::Fill {
            instrument: INSTRUMENT,
            seq: self.seq,
            event_ts_us: TsUs::from_micros(self.at(step, 1)),
            side,
            price: px(tick),
            qty: Qty(FIXED_SCALE),
            commission: 0,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
            quote_level: None,
        });
    }

    fn finish(self, name: &'static str) -> ChartScene {
        ChartScene {
            name,
            chart: self.chart,
            instrument: INSTRUMENT,
            tick: TICK,
        }
    }
}

fn mid_bid(step: i64) -> i64 {
    BASE_BID + triangle(step, 137, 18) + triangle(step, 23, 5)
}

fn wobble(commit: i64) -> i64 {
    triangle(commit, 7, 3) - 1
}

fn triangle(step: i64, period: i64, amplitude: i64) -> i64 {
    let phase = step.rem_euclid(period);
    let rise = phase.min(period - phase);
    rise * amplitude * 2 / period
}

fn side_for(step: i64) -> Side {
    if step % 180 < 90 { Side::Buy } else { Side::Sell }
}

fn snapshot(seq: u64, micros: i64, bids: &[i64], asks: &[i64]) -> UiBookSnapshot {
    let empty = Level {
        price: Price(0),
        qty: Qty(0),
    };
    let mut bid_levels = [empty; UI_BOOK_LEVELS];
    let mut ask_levels = [empty; UI_BOOK_LEVELS];
    for (slot, &tick) in bid_levels.iter_mut().zip(bids) {
        *slot = Level {
            price: px(tick),
            qty: Qty(FIXED_SCALE),
        };
    }
    for (slot, &tick) in ask_levels.iter_mut().zip(asks) {
        *slot = Level {
            price: px(tick),
            qty: Qty(FIXED_SCALE),
        };
    }
    UiBookSnapshot {
        instrument: INSTRUMENT,
        seq,
        event_ts_us: TsUs::from_micros(micros),
        state: UiBookState::Valid,
        bid_len: bids.len() as u16,
        ask_len: asks.len() as u16,
        bids: bid_levels,
        asks: ask_levels,
    }
}

fn px(tick_index: i64) -> Price {
    Price(tick_index * FIXED_SCALE)
}
