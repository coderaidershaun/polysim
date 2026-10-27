//! The chart inputs the engine remembers and sends to a newly attached workstation, so a fresh tab
//! folds the same mid, fill and position history that a tab attached all along holds.

use crate::ids::{InstrumentId, Price, Side};
use crate::time::{DurationUs, TsUs};

use super::TopOfBook;

/// The length of the rolling window every chart shows. The engine keeps no history older than it.
pub const UI_CHART_WINDOW: DurationUs = DurationUs::from_secs(300);

/// The most samples one `UiHistory` datagram carries. The link's datagram size limit sets it, and
/// the build fails if a larger value would not fit.
pub const UI_HISTORY_SAMPLES: usize = 42;

/// One input of the charts, as the live feed carried it. Each keeps the live frame's own value
/// types, so the workstation folds it through the same code as that frame and draws the same chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiHistorySample {
    Mid {
        event_ts_us: TsUs,
        top: TopOfBook,
        has_gap_before: bool,
    },
    Position {
        event_ts_us: TsUs,
        exposure_quote: f64,
        pnl_quote: f64,
    },
    Fill {
        event_ts_us: TsUs,
        side: Side,
        price: Price,
    },
}

impl UiHistorySample {
    pub const EMPTY: Self = Self::Mid {
        event_ts_us: TsUs::from_micros(0),
        top: TopOfBook {
            bid: Price(0),
            ask: Price(0),
        },
        has_gap_before: false,
    };

    #[inline]
    pub fn event_ts_us(&self) -> TsUs {
        match self {
            Self::Mid { event_ts_us, .. }
            | Self::Position { event_ts_us, .. }
            | Self::Fill { event_ts_us, .. } => *event_ts_us,
        }
    }
}

/// One datagram of an instrument's remembered samples.
///
/// `cutoff_ts_us` is the highest stamp the engine had seen for this instrument when this backfill
/// began, so it names the backfill: a chunk with a new cutoff starts a new one. The bucket holding
/// the cutoff is left to the live feed.
///
/// `first_index` is the index of `samples()[0]`, counted from the instrument's first sample ever,
/// and sample `i` has index `first_index + i`. Indices only rise within one backfill, so a
/// receiver tells a repeated chunk from a lost one.
///
/// Samples are in the order the live feed carried them. Each kind is in event order, but stamps are
/// not sorted across kinds, because books carry receive time and fills carry the venue's time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiHistory {
    pub instrument: InstrumentId,
    pub cutoff_ts_us: TsUs,
    pub first_index: u64,
    pub len: u8,
    pub samples: [UiHistorySample; UI_HISTORY_SAMPLES],
}

impl UiHistory {
    /// The first `len` samples. A hand-built value with a larger `len` is clamped, never a panic.
    #[inline]
    pub fn samples(&self) -> &[UiHistorySample] {
        &self.samples[..usize::from(self.len).min(UI_HISTORY_SAMPLES)]
    }
}
