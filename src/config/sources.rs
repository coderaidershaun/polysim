//! Source schema: tagged enum; seam for new venues.

use serde::Deserialize;

use super::execution::ExecutionMode;
use super::tracker::TrackerSpec;
use crate::labelled_enum::labelled_enum;
use crate::time::DurationUs;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "exchange", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceSpec {
    Binance {
        market: BinanceMarket,
        /// Single deployment field (avoid dual-env desync).
        #[serde(default)]
        env: BinanceEnv,
        base: String,
        quote: String,
        #[serde(default)]
        subscriptions: Subscriptions,
        #[serde(default = "default_kline_intervals")]
        kline_intervals: Vec<KlineInterval>,
        #[serde(default = "default_book_capacity")]
        book_capacity: usize,
        max_exposure_quote: f64,
        tracker: TrackerSpec,
    },
    Polymarket {
        series: PolySeries,
        #[serde(default)]
        subscriptions: PolySubscriptions,
        #[serde(default = "default_poly_book_capacity")]
        book_capacity: usize,
        max_exposure_quote: f64,
        tracker: TrackerSpec,
    },
}

impl SourceSpec {
    /// The venue as an operator spells it in `exchange:`.
    pub fn venue(&self) -> &'static str {
        match self {
            SourceSpec::Binance { .. } => "binance",
            SourceSpec::Polymarket { .. } => "polymarket",
        }
    }

    /// Which execution edges this venue actually has. Polymarket carries no simulated venue: the
    /// fill model is built on binance spot's depth granularity and aggregate trades.
    pub fn execution_modes(&self) -> &'static [ExecutionMode] {
        match self {
            SourceSpec::Binance { .. } => {
                &[ExecutionMode::Off, ExecutionMode::Sim, ExecutionMode::Live]
            }
            SourceSpec::Polymarket { .. } => &[ExecutionMode::Off, ExecutionMode::Live],
        }
    }

    pub fn supports_execution(&self, mode: ExecutionMode) -> bool {
        self.execution_modes().contains(&mode)
    }
}

labelled_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum BinanceMarket {
        Spot = "spot",
        Perpetual = "perpetual",
    }
    pub fn as_str;
}

labelled_enum! {
    /// Config is the lower layer (adapter re-exports); names one type for both halves.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum BinanceEnv {
        #[default]
        Production = "production",
        Testnet = "testnet",
    }
    pub fn as_str;
}

labelled_enum! {
    /// Seam for additional series.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
    pub enum PolySeries {
        #[serde(rename = "btc-updown-5m")]
        BtcUpDown5m = "btc-updown-5m",
    }
    pub fn as_str;
}

impl PolySeries {
    pub const fn window_len(self) -> DurationUs {
        match self {
            PolySeries::BtcUpDown5m => DurationUs::from_secs(300),
        }
    }

    /// The venue symbols this series registers, as `[up, down]` per rotation slot and in slot order.
    /// The rotation driver pairs its legs from here rather than re-typing the suffixes, so a symbol
    /// only ever has one spelling.
    pub fn slot_leg_symbols(self) -> [[Box<str>; 2]; 2] {
        let base = self.as_str();
        [["a-up", "a-down"], ["b-up", "b-down"]]
            .map(|slot| slot.map(|leg| format!("{base}-{leg}").into_boxed_str()))
    }

    pub fn slot_symbols(self) -> [Box<str>; 4] {
        let [[a_up, a_down], [b_up, b_down]] = self.slot_leg_symbols();
        [a_up, a_down, b_up, b_down]
    }

    /// The Gamma series this engine resolves windows from.
    pub const fn gamma_series_id(self) -> &'static str {
        match self {
            PolySeries::BtcUpDown5m => "10684",
        }
    }
}

/// No klines here (single market-data channel; unexpressable > default-true trap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolySubscriptions {
    #[serde(default = "default_true")]
    pub trades: bool,
    #[serde(default = "default_true")]
    pub book_updates: bool,
    #[serde(default = "default_true")]
    pub book_snapshots: bool,
}

impl Default for PolySubscriptions {
    fn default() -> Self {
        Self {
            trades: true,
            book_updates: true,
            book_snapshots: true,
        }
    }
}

impl From<PolySubscriptions> for Subscriptions {
    fn from(poly: PolySubscriptions) -> Self {
        Subscriptions {
            trades: poly.trades,
            book_updates: poly.book_updates,
            book_snapshots: poly.book_snapshots,
            klines: false,
        }
    }
}

/// All fields default true; omitted blocks enable the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscriptions {
    #[serde(default = "default_true")]
    pub trades: bool,
    #[serde(default = "default_true")]
    pub book_updates: bool,
    #[serde(default = "default_true")]
    pub book_snapshots: bool,
    #[serde(default = "default_true")]
    pub klines: bool,
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self {
            trades: true,
            book_updates: true,
            book_snapshots: true,
            klines: true,
        }
    }
}

labelled_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
    pub enum KlineInterval {
        #[serde(rename = "1m")]
        OneMinute = "1m",
        #[serde(rename = "3m")]
        ThreeMinutes = "3m",
        #[serde(rename = "5m")]
        FiveMinutes = "5m",
        #[serde(rename = "15m")]
        FifteenMinutes = "15m",
        #[serde(rename = "30m")]
        ThirtyMinutes = "30m",
        #[serde(rename = "1h")]
        OneHour = "1h",
        #[serde(rename = "2h")]
        TwoHours = "2h",
        #[serde(rename = "4h")]
        FourHours = "4h",
        #[serde(rename = "6h")]
        SixHours = "6h",
        #[serde(rename = "8h")]
        EightHours = "8h",
        #[serde(rename = "12h")]
        TwelveHours = "12h",
        #[serde(rename = "1d")]
        OneDay = "1d",
        #[serde(rename = "3d")]
        ThreeDays = "3d",
        #[serde(rename = "1w")]
        OneWeek = "1w",
        #[serde(rename = "1M")]
        OneMonth = "1M",
    }
    /// The venue interval string (`@kline_<interval>` and REST `interval=`).
    pub fn as_str;
}

impl KlineInterval {
    /// Candle span in whole minutes, or `None` for the calendar-variable `1M` — a month has no
    /// fixed minute count, so a caller needing a span substitutes its own bound.
    pub fn fixed_minutes(self) -> Option<u64> {
        let minutes = match self {
            KlineInterval::OneMinute => 1,
            KlineInterval::ThreeMinutes => 3,
            KlineInterval::FiveMinutes => 5,
            KlineInterval::FifteenMinutes => 15,
            KlineInterval::ThirtyMinutes => 30,
            KlineInterval::OneHour => 60,
            KlineInterval::TwoHours => 120,
            KlineInterval::FourHours => 240,
            KlineInterval::SixHours => 360,
            KlineInterval::EightHours => 480,
            KlineInterval::TwelveHours => 720,
            KlineInterval::OneDay => 1_440,
            KlineInterval::ThreeDays => 4_320,
            KlineInterval::OneWeek => 10_080,
            KlineInterval::OneMonth => return None,
        };
        Some(minutes)
    }

    /// The candle's span, `None` for the calendar-variable `1M` — see [`fixed_minutes`]. The one
    /// place a span is derived from an interval; a second derivation is a divergence waiting to
    /// happen.
    ///
    /// [`fixed_minutes`]: KlineInterval::fixed_minutes
    pub fn fixed_duration(self) -> Option<DurationUs> {
        self.fixed_minutes()
            .map(|minutes| DurationUs::from_secs(minutes as i64 * 60))
    }
}

fn default_kline_intervals() -> Vec<KlineInterval> {
    vec![KlineInterval::OneMinute]
}

fn default_book_capacity() -> usize {
    10_000
}

fn default_poly_book_capacity() -> usize {
    128
}

fn default_true() -> bool {
    true
}
