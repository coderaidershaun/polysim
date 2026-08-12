//! The rolling event-time window both the univariate and the multivariate processes are built on.
//! It lives here rather than in either of them because the clamping push below is a precondition of
//! every recursion in the module, and two copies of it could disagree.

use crate::hot::series::FastQueue;
use crate::time::{DurationUs, TsUs};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EventWindow {
    times: FastQueue<f64>,
    origin: Option<TsUs>,
    last_secs: f64,
    out_of_order: u64,
}

impl EventWindow {
    /// # Panics
    /// `max_events == 0` (config bug).
    pub(super) fn new(max_events: usize) -> Self {
        assert!(max_events != 0, "hawkes event window must be non-zero");
        Self {
            times: FastQueue::new(max_events, 2),
            origin: None,
            last_secs: 0.0,
            out_of_order: 0,
        }
    }

    /// An earlier stamp than its predecessor is clamped to that predecessor so every recursion still
    /// sees `dt >= 0`, and counted.
    pub(super) fn push(&mut self, ts: TsUs) {
        let origin = *self.origin.get_or_insert(ts);
        let secs = ts.diff(origin).to_secs();
        if secs < self.last_secs {
            self.out_of_order += 1;
        } else {
            self.last_secs = secs;
        }
        self.times.push(self.last_secs);
    }

    /// Wipes the window and its time origin; the lifetime anomaly counter survives.
    pub(super) fn clear(&mut self) {
        self.times.clear();
        self.origin = None;
        self.last_secs = 0.0;
    }

    pub(super) fn len(&self) -> usize {
        self.times.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub(super) fn out_of_order_count(&self) -> u64 {
        self.out_of_order
    }

    /// Oldest-first relative seconds — always one contiguous slice.
    pub(super) fn times_secs(&self) -> &[f64] {
        self.times.as_slice()
    }

    /// `now` as relative seconds, clamped to at least the last event; `None` before any event.
    pub(super) fn end_secs(&self, now: TsUs) -> Option<f64> {
        let last = self.times.last()?;
        let origin = self.origin?;
        Some(now.diff(origin).to_secs().max(last))
    }

    /// The absolute stamp of the newest retained event, reconstructed from the origin — exact,
    /// because the stored seconds came from an integral µs difference.
    pub(super) fn last_ts(&self) -> Option<TsUs> {
        let last = self.times.last()?;
        let origin = self.origin?;
        Some(origin + DurationUs::from_micros((last * 1e6).round() as i64))
    }
}
