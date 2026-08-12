//! Microsecond time types + monotonic engine clock: every timestamp µs-since-epoch in typed wrapper.

use core::ops::{Add, Sub};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Microseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TsUs(i64);

impl TsUs {
    #[inline]
    pub const fn from_micros(us: i64) -> Self {
        Self(us)
    }

    #[inline]
    pub const fn micros(self) -> i64 {
        self.0
    }

    /// Signed span `self - earlier`; negative when `self` precedes `earlier`.
    #[inline]
    pub const fn diff(self, earlier: TsUs) -> DurationUs {
        DurationUs(self.0 - earlier.0)
    }

    pub(crate) fn civil(self) -> CivilTime {
        let seconds = self.0.div_euclid(1_000_000);
        let day_seconds = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
        CivilTime {
            year,
            month,
            day,
            hour: day_seconds / 3_600,
            minute: (day_seconds % 3_600) / 60,
            second: day_seconds % 60,
            micros: self.0.rem_euclid(1_000_000),
        }
    }
}

/// UTC calendar breakdown of an instant, for the display paths that spell a timestamp out. Held
/// apart from [`TsUs`] because nothing downstream of it is a timestamp any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CivilTime {
    pub(crate) year: i64,
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) hour: i64,
    pub(crate) minute: i64,
    pub(crate) second: i64,
    pub(crate) micros: i64,
}

/// Signed span in microseconds. Latency uses DurationUs, no third type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationUs(i64);

impl DurationUs {
    pub const ZERO: DurationUs = DurationUs(0);

    /// The engine's smallest representable step.
    pub const RESOLUTION: DurationUs = DurationUs(1);

    #[inline]
    pub const fn from_micros(us: i64) -> Self {
        Self(us)
    }

    #[inline]
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms * 1_000)
    }

    #[inline]
    pub const fn from_secs(secs: i64) -> Self {
        Self(secs * 1_000_000)
    }

    #[inline]
    pub const fn micros(self) -> i64 {
        self.0
    }

    #[inline]
    pub const fn to_secs(self) -> f64 {
        self.0 as f64 / 1e6
    }
}

impl Add<DurationUs> for TsUs {
    type Output = TsUs;

    #[inline]
    fn add(self, rhs: DurationUs) -> TsUs {
        TsUs(self.0 + rhs.0)
    }
}

impl Sub<DurationUs> for TsUs {
    type Output = TsUs;

    #[inline]
    fn sub(self, rhs: DurationUs) -> TsUs {
        TsUs(self.0 - rhs.0)
    }
}

/// Monotonic clock: wall-clock anchor at start + Instant elapsed. Immune to NTP steps, stays comparable to venue.
#[derive(Debug, Clone)]
pub struct EngineClock {
    wall_anchor_ts_us: i64,
    mono_anchor: Instant,
}

impl EngineClock {
    pub fn start() -> Self {
        let wall_anchor_ts_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as i64)
            .expect("system clock reads before the unix epoch — cannot anchor engine time");
        Self {
            wall_anchor_ts_us,
            mono_anchor: Instant::now(),
        }
    }

    /// Called twice per hot-path message; small monotonic read folds into caller.
    #[inline]
    pub fn now(&self) -> TsUs {
        let elapsed_us = self.mono_anchor.elapsed().as_micros() as i64;
        TsUs::from_micros(self.wall_anchor_ts_us + elapsed_us)
    }
}

/// Wall-clock µs at call: names run (link restart, controller epoch base, Parquet disambiguator).
/// Not engine state -> plain clock read OK. Pre-epoch -> 0 not panic.
pub(crate) fn boot_stamp_us() -> TsUs {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as i64);
    TsUs::from_micros(micros)
}

/// Hinnant civil_from_days: proleptic Gregorian (year,month,day) from signed day count since Unix epoch.
/// Exact arithmetic across full range. Shared by log+Parquet -> no calendar crate needed.
pub(crate) fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pivot = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pivot + 2) / 5 + 1;
    let month = if month_pivot < 10 { month_pivot + 3 } else { month_pivot - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}
