//! Polling and buffering for simulator input lanes.

use super::super::core::market::ResetReason;
use super::super::lanes::SimLane;
use super::SimActor;
use crate::msg::exec::ExecLaneItem;
use crate::time::TsUs;

impl SimActor {
    pub(super) fn poll_lanes(&mut self) {
        self.intake_commands();
        self.intake_trades();
        self.intake_depth();
    }

    pub(super) fn intake_commands(&mut self) {
        while let Ok(item) = self.commands.pop() {
            match item {
                ExecLaneItem::Command(stamped) => {
                    self.horizon.observe(SimLane::Command, stamped.issued_ts_us);
                    if let Err(error) = self
                        .commands_due
                        .try_push(self.latency.arrival(stamped.issued_ts_us), stamped)
                    {
                        self.fatal.trip(format!("exchange simulator: {error}"));
                        return;
                    }
                }
                ExecLaneItem::Watermark(at_ts_us) => {
                    self.horizon.observe(SimLane::Command, at_ts_us)
                }
            }
        }
        let watermark = self.horizon.watermark(SimLane::Command);
        self.readiness.observe_lane(SimLane::Command, watermark);
    }

    pub(super) fn intake_trades(&mut self) {
        let Self {
            trades,
            driver,
            latency,
            ..
        } = self;
        let latency = *latency;
        trades.drain(|batch| driver.venue_mut().on_market_batch(batch, latency));
        self.observe_lane(SimLane::Trade, self.trades.proven_watermark_ts_us());
    }

    pub(super) fn intake_depth(&mut self) {
        let Self {
            depth,
            driver,
            latency,
            ..
        } = self;
        let latency = *latency;
        depth.drain(|batch| driver.venue_mut().on_market_batch(batch, latency));
        self.observe_lane(SimLane::Depth, self.depth.proven_watermark_ts_us());
    }

    fn observe_lane(&mut self, lane: SimLane, watermark: Option<TsUs>) {
        let Some(watermark) = watermark else {
            return;
        };
        self.horizon.observe(lane, watermark);
        self.readiness.observe_lane(lane, watermark);
    }

    #[cold]
    pub(super) fn suspend(&mut self, reason: ResetReason) {
        if !self.driver.venue().market().is_matching_live() {
            return;
        }
        if let Some(at_ts_us) = self.venue_now() {
            self.driver.venue_mut().suspend_matching(reason, at_ts_us);
            self.market_recovery_generation = self.driver.venue().market().recovery().generation;
        }
        self.driver.close();
        self.reopen();
        self.readiness.withdraw();
    }
}
