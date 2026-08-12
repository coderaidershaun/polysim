//! Multivariate Hawkes: rolling events and linear exponential kernels.
//! Times/components in lockstep FastQueues, paired eviction.

mod linear;

pub use linear::{MultivariateHawkes, MultivariateParams};

use super::window::EventWindow;
use crate::hot::series::FastQueue;
use crate::time::TsUs;

/// Rolling window, full -> evict oldest, fits see most recent.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateEvents {
    window: EventWindow,
    components: FastQueue<i64>,
    dimension: usize,
}

impl MultivariateEvents {
    /// # Panics
    /// `dimension == 0` or `max_events == 0` — config bug.
    pub fn new(dimension: usize, max_events: usize) -> Self {
        assert!(
            dimension != 0,
            "multivariate hawkes needs at least one component"
        );
        Self {
            window: EventWindow::new(max_events),
            components: FastQueue::new(max_events, 2),
            dimension,
        }
    }

    /// Timestamps must be non-decreasing; an earlier stamp is clamped to the previous one so every
    /// recursion still sees `dt >= 0`, and is counted — see
    /// [`MultivariateEvents::out_of_order_count`].
    ///
    /// # Panics
    /// `component >= dimension` — wiring bug.
    pub fn push(&mut self, ts: TsUs, component: usize) {
        assert!(
            component < self.dimension,
            "component {component} outside the {}-component process",
            self.dimension
        );
        self.window.push(ts);
        self.components.push(component as i64);
    }

    pub fn clear(&mut self) {
        self.window.clear();
        self.components.clear();
    }

    pub fn len(&self) -> usize {
        self.window.len()
    }

    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Events whose stamp preceded their predecessor and were clamped — lifetime.
    pub fn out_of_order_count(&self) -> u64 {
        self.window.out_of_order_count()
    }

    pub fn times_secs(&self) -> &[f64] {
        self.window.times_secs()
    }

    pub(crate) fn components(&self) -> &[i64] {
        self.components.as_slice()
    }

    pub(crate) fn window_end_secs(&self, now: TsUs) -> Option<f64> {
        self.window.end_secs(now)
    }

    fn last_ts(&self) -> Option<TsUs> {
        self.window.last_ts()
    }
}
