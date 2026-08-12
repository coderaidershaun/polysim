//! Univariate Hawkes: the rolling [`HawkesEvents`] window every evaluator and fitter reads, plus
//! the three kernel families — linear exponential, its nonlinear variants, and discrete-time counts.

mod discrete;
mod linear;
mod nonlinear;

pub use discrete::{DiscreteCounts, DiscreteParams};
pub use linear::{HawkesParams, UnivariateHawkes};
pub use nonlinear::{LogisticParams, LogisticShape, QuadraticParams};
pub(crate) use nonlinear::{logistic_compensator, logistic_log_intensity_sum};

use super::window::EventWindow;
use crate::time::TsUs;

/// Rolling window of event times as seconds relative to first event; full window evicts oldest.
#[derive(Debug, Clone, PartialEq)]
pub struct HawkesEvents {
    window: EventWindow,
}

impl HawkesEvents {
    /// # Panics
    /// `max_events == 0` (config bug).
    pub fn new(max_events: usize) -> Self {
        Self {
            window: EventWindow::new(max_events),
        }
    }

    /// Timestamps must be non-decreasing; an earlier stamp is clamped to the previous one so every
    /// recursion still sees `dt >= 0`, and is counted — see [`HawkesEvents::out_of_order_count`].
    pub fn push(&mut self, ts: TsUs) {
        self.window.push(ts);
    }

    /// Wipes the window and its time origin; the lifetime anomaly counter survives.
    pub fn clear(&mut self) {
        self.window.clear();
    }

    pub fn len(&self) -> usize {
        self.window.len()
    }

    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }

    /// Events whose stamp preceded their predecessor and were clamped — lifetime.
    pub fn out_of_order_count(&self) -> u64 {
        self.window.out_of_order_count()
    }

    /// Oldest-first relative seconds — always one contiguous slice.
    pub fn times_secs(&self) -> &[f64] {
        self.window.times_secs()
    }

    /// `now` as relative seconds, clamped to at least the last event; `None` before any event.
    pub(crate) fn window_end_secs(&self, now: TsUs) -> Option<f64> {
        self.window.end_secs(now)
    }

    fn last_ts(&self) -> Option<TsUs> {
        self.window.last_ts()
    }
}
