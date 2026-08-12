//! Simulator taps at the Binance normalization boundary.

use std::time::Duration;

use crate::adapters::binance::rest::{RestError, RestRequest};
use crate::error;
use crate::msg::inbound::TappedMessage;

use super::{Actor, liveness};

impl Actor {
    // Both consumers must observe the same prefix.
    pub(super) fn flush_tapped(&mut self, out: Vec<TappedMessage>) {
        for tapped in out {
            let mut message = tapped.message;
            message.set_queued_ts_us(self.clock.now());
            self.producer.push_tapped(message, tapped.venue_meta);
        }
    }

    /// Uses the simulator heartbeat for tapped lanes.
    pub(super) fn poll_period(&self) -> Duration {
        if self.producer.has_tap() { self.tap_heartbeat } else { liveness::LIVENESS_POLL }
    }

    /// Every REST read this actor makes goes through here, whichever lane it serves: a fetch that
    /// outlives the heartbeat emits no watermark for its duration and stalls the simulator's
    /// ordering gate. Falls through to a plain fetch when the lane carries no tap.
    ///
    /// # Errors
    /// Returns the request error unchanged.
    pub(super) async fn fetch_text_beating(
        &mut self,
        request: &RestRequest,
    ) -> Result<String, RestError> {
        if !self.producer.has_tap() {
            return self.rest.fetch_text(request).await;
        }
        let mut fetch = std::pin::pin!(self.rest.fetch_text(request));
        loop {
            match tokio::time::timeout(self.tap_heartbeat, &mut fetch).await {
                Ok(result) => return result,
                Err(_elapsed) => self.producer.push_tap_watermark(self.clock.now()),
            }
        }
    }

    // Never reuse a sequence generation.
    #[cold]
    pub(super) fn on_epoch_exhausted(&self) {
        error!(
            "binance adapter {} exhausted its connection epoch counter",
            self.label
        );
        self.fatal.trip(format!(
            "binance adapter {} exhausted its connection epoch counter",
            self.label
        ));
    }
}
