//! Deterministic exposure/PnL: stream -> ChartModel + PositionModel. Risk takes MID window, no hand-drawn curves.

use polysim::desktop::chart_model::{BookContinuity, ChartModel};
use polysim::desktop::position_chart_model::PositionModel;
use polysim::ids::{AssetId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::Liquidity;
use polysim::msg::inbound::Level;
use polysim::msg::ui::{UI_BOOK_LEVELS, UiBookSnapshot, UiBookState, UiEvent};
use polysim::time::DurationUs;
use polysim::time::TsUs;

const SPIN: DurationUs = DurationUs::from_micros(1_000_000);
const WINDOW_BUCKETS: i64 = 300;

const COMMITS_PER_BUCKET: i64 = 2;

const TICK: Price = Price(FIXED_SCALE);
const INSTRUMENT: InstrumentId = InstrumentId(0);

/// Realistic epoch -> f64 norm before f32.
const BASE_TS: i64 = 1_753_300_000_000_000;

/// BTC scale: $1 = 1 tick.
const BASE_BID: i64 = 65_990;

/// Order size 0.01 base: handful fills axis at BASE_BID.
const ORDER_QTY: Qty = Qty(FIXED_SCALE / 100);

pub struct PositionScene {
    pub name: &'static str,
    pub check: &'static str,
    pub chart: ChartModel,
    pub positions: PositionModel,
    pub instrument: InstrumentId,
    pub tick: Price,
}

pub fn position_scenes() -> Vec<PositionScene> {
    vec![
        pnl_through_zero(),
        exposure_steps(),
        flat_at_zero(),
        dropped_frames(),
        rotation_clears_then_re_marks(),
        rotation_then_new_window(),
        no_tick_grid(),
    ]
}

/// Position held, mark oscillates entry: PnL crosses zero. Toggle rescales axis, most different series.
fn pnl_through_zero() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..20);
    feed.buy(20);
    feed.walk(20..WINDOW_BUCKETS);
    feed.finish(
        "pnl through zero — bought at 20 s, mark oscillates",
        "X rescales the axis from hundreds of dollars to single dollars; the PnL line crosses zero",
    )
}

/// Inventory long->short: exposure flips sign, JUMPS on fills, DRIFTS between (~243 levels not 13), mark-to-market essence.
fn exposure_steps() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    for step in 0..WINDOW_BUCKETS {
        // Buy phase short on purpose: equal split -> never short, negative color unused.
        if step % 24 == 0 && step > 0 {
            match step < WINDOW_BUCKETS / 3 {
                true => feed.buy(step),
                false => feed.sell(step),
            }
        }
        feed.bucket(step);
        feed.emit(step);
    }
    feed.finish(
        "exposure — long, through flat, into short",
        "line JUMPS at each fill and DRIFTS with the mark between them; it is flat ONLY where the \
         position is zero (spins 0-23 before the first fill, and 192-215 mid-scene) because zero \
         times any mark is zero. Crosses into short and the readout's colour flips with the sign",
    )
}

/// Marked, never traded: axis floor catches 1e-8 mantissas. Every run first minutes, not corner case.
fn flat_at_zero() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..WINDOW_BUCKETS);
    feed.finish(
        "flat at zero — the minimum-span floor",
        "axis spans at least ONE WHOLE QUOTE UNIT, never 0.00000001 steps",
    )
}

/// Ring overflow: frames drop (drop policy safe on absolute state), mid chart draws through. Line SPLITS hole, no bridge.
fn dropped_frames() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..40);
    feed.buy(40);
    feed.walk(40..110);
    feed.walk_undelivered(110..170);
    feed.walk(170..WINDOW_BUCKETS);
    feed.finish(
        "dropped frames — a full event ring, books unaffected",
        "risk line SPLITS at the hole while the mid above keeps drawing straight through it",
    )
}

/// Rotation clears + re-marks: draws flat zero (NOT silent). Catches "no data" vs "zero" painter bug. Stale exposure = ghost.
fn rotation_clears_then_re_marks() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..60);
    feed.buy(60);
    feed.walk(60..140);
    feed.rotate(140);
    feed.walk(141..WINDOW_BUCKETS);
    feed.finish(
        "rotation — clears, then re-marks at flat ZERO",
        "the series starts at the LEFT EDGE as a flat line ON zero — drawn, not absent; the empty \
         stretch to its right is what absent looks like. Nothing bridges back to the pre-rotation \
         series",
    )
}

/// ALIGNMENT: 70-bucket stagger LOAD-BEARING. Mid @140, risk @210 -> catches wrong window derivation. 1-bucket stagger looks right, catches nothing.
fn rotation_then_new_window() -> PositionScene {
    let mut feed = Feed::new(Some(TICK));
    feed.walk(0..60);
    feed.buy(60);
    feed.walk(60..140);
    feed.rotate(140);
    feed.walk_undelivered(140..210);
    feed.buy(210);
    feed.walk(210..WINDOW_BUCKETS);
    feed.finish(
        "rotation, silence, new window",
        "crosshair sits on the same spin in both charts, though they start at different buckets",
    )
}

/// Polymarket: no tick -> MID empty, risk paints empty (shared window consequence).
fn no_tick_grid() -> PositionScene {
    let mut feed = Feed::new(None);
    feed.walk(0..60);
    feed.buy(60);
    feed.walk(60..WINDOW_BUCKETS);
    PositionScene {
        tick: Price(0),
        ..feed.finish(
            "no tick grid — no window, so no risk chart either",
            "KNOWN RULED LIMITATION, not a bug: no stamped tick = no window = no risk chart",
        )
    }
}

/// Drives fold via ledger, derived not drawn.
struct Feed {
    chart: ChartModel,
    positions: PositionModel,
    seq: u64,
    position_base: i64,
    cash_quote: i64,
    mark: Option<Price>,
}

impl Feed {
    fn new(tick: Option<Price>) -> Self {
        let mut chart = ChartModel::with_capacity(1, SPIN);
        chart.configure(&[tick], SPIN);
        let mut positions = PositionModel::with_capacity(1, SPIN);
        positions.configure(1, SPIN);
        Self {
            chart,
            positions,
            seq: 0,
            position_base: 0,
            cash_quote: 0,
            mark: None,
        }
    }

    /// Commit book + emit position/bucket.
    fn walk(&mut self, steps: std::ops::Range<i64>) {
        for step in steps {
            self.bucket(step);
            self.emit(step);
        }
    }

    /// Books not dropped, position frames are (full ring policy). Mark SET, books land.
    fn walk_undelivered(&mut self, steps: std::ops::Range<i64>) {
        for step in steps {
            self.bucket(step);
        }
    }

    /// Mark = last mid (mirrors engine re-mark).
    fn bucket(&mut self, step: i64) {
        for sub in 0..COMMITS_PER_BUCKET {
            let bid = mid_bid(step) + wobble(step * COMMITS_PER_BUCKET + sub);
            self.commit(self.at(step, sub), bid);
            self.mark = Some(px(bid + 1));
        }
    }

    /// Absolute state: skip if no mark, f64 at emit.
    fn emit(&mut self, step: i64) {
        let Some(mark) = self.mark else { return };
        let exposure = mark.notional(Qty(self.position_base));
        self.seq += 1;
        self.positions.apply_event(&UiEvent::Position {
            instrument: INSTRUMENT,
            seq: self.seq,
            event_ts_us: TsUs::from_micros(self.at(step, COMMITS_PER_BUCKET - 1)),
            exposure_quote: exposure as f64 / FIXED_SCALE as f64,
            pnl_quote: (self.cash_quote + exposure) as f64 / FIXED_SCALE as f64,
        });
    }

    fn buy(&mut self, step: i64) {
        self.fill(step, Side::Buy);
    }

    fn sell(&mut self, step: i64) {
        self.fill(step, Side::Sell);
    }

    /// Buy pays cash, sell returns.
    fn fill(&mut self, step: i64, side: Side) {
        let price = px(mid_bid(step));
        let notional = price.notional(ORDER_QTY);
        match side {
            Side::Buy => {
                self.position_base += ORDER_QTY.0;
                self.cash_quote -= notional;
            }
            Side::Sell => {
                self.position_base -= ORDER_QTY.0;
                self.cash_quote += notional;
            }
        }
        self.seq += 1;
        self.chart.apply_event(&UiEvent::Fill {
            instrument: INSTRUMENT,
            seq: self.seq,
            event_ts_us: TsUs::from_micros(self.at(step, 0)),
            side,
            price,
            qty: ORDER_QTY,
            commission: 0,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
            quote_level: None,
        });
    }

    /// Window handover resets position, no emit until re-mark.
    fn rotate(&mut self, step: i64) {
        self.seq += 1;
        let event = UiEvent::Rotation {
            instrument: INSTRUMENT,
            seq: self.seq,
            event_ts_us: TsUs::from_micros(self.at(step, 0)),
        };
        self.chart.apply_event(&event);
        self.positions.apply_event(&event);
        self.position_base = 0;
        self.cash_quote = 0;
        self.mark = None;
    }

    fn at(&self, step: i64, sub: i64) -> i64 {
        BASE_TS + step * SPIN.micros() + sub * SPIN.micros() / COMMITS_PER_BUCKET
    }

    fn commit(&mut self, micros: i64, bid: i64) {
        self.seq += 1;
        self.chart
            .apply_book(&snapshot(self.seq, micros, bid), BookContinuity::Continuous);
    }

    fn finish(self, name: &'static str, check: &'static str) -> PositionScene {
        PositionScene {
            name,
            check,
            chart: self.chart,
            positions: self.positions,
            instrument: INSTRUMENT,
            tick: TICK,
        }
    }
}

/// Triangles: slow carries entry, fast breaks monotone.
fn mid_bid(step: i64) -> i64 {
    BASE_BID + wave(step, 137, 400) + wave(step, 23, 40)
}

fn wobble(commit: i64) -> i64 {
    wave(commit, 7, 3)
}

/// Triangle ±amplitude: no RNG, reproducible.
fn wave(step: i64, period: i64, amplitude: i64) -> i64 {
    let phase = step.rem_euclid(period);
    let rise = phase.min(period - phase);
    rise * amplitude * 4 / period - amplitude
}

fn snapshot(seq: u64, micros: i64, bid: i64) -> UiBookSnapshot {
    let empty = Level {
        price: Price(0),
        qty: Qty(0),
    };
    let mut bids = [empty; UI_BOOK_LEVELS];
    let mut asks = [empty; UI_BOOK_LEVELS];
    bids[0] = Level {
        price: px(bid),
        qty: Qty(FIXED_SCALE),
    };
    asks[0] = Level {
        price: px(bid + 2),
        qty: Qty(FIXED_SCALE),
    };
    UiBookSnapshot {
        instrument: INSTRUMENT,
        seq,
        event_ts_us: TsUs::from_micros(micros),
        state: UiBookState::Valid,
        bid_len: 1,
        ask_len: 1,
        bids,
        asks,
    }
}

fn px(tick_index: i64) -> Price {
    Price(tick_index * FIXED_SCALE)
}
