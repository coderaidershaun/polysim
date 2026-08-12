//! Simulated stream startup and readiness announcements.

use super::super::wire::decimal;
use super::{EffectStamp, SimExecDriver, asset_name, venue_millis};
use crate::adapters::binance::exec::{DecodeContext, account_snapshot_chunks};
use crate::adapters::binance::rest::Balance;
use crate::adapters::exec::{Phase, open_orders_snapshot_end, stream_ready};
use crate::msg::inbound::InboundMessage;
use crate::shutdown::FatalSignal;
use crate::time::TsUs;

use super::SimDriverContext;

impl SimExecDriver {
    pub fn open(&mut self, effective_ts_us: TsUs, fatal: &FatalSignal) {
        self.has_swept = false;
        let mut effects = Vec::new();
        self.core.on_connected(&mut |effect| effects.push(effect));
        self.apply(&effects, EffectStamp::at(effective_ts_us), fatal);
    }

    pub fn close(&mut self) {
        self.core.on_disconnected();
    }

    pub fn can_arm(&self) -> bool {
        !self.core.has_prior_run() && self.core.phase() == Phase::Resyncing
    }

    /// # Panics
    /// If [`SimExecDriver::can_arm`] is false.
    pub fn announce_readiness(
        &mut self,
        at_ts_us: TsUs,
        context: SimDriverContext<'_>,
    ) -> Vec<InboundMessage> {
        assert!(
            self.core.on_stream_ready(),
            "the simulated venue armed a core that could not verify its stream"
        );
        let decode = DecodeContext {
            received_ts_us: at_ts_us,
            ..context.decode
        };
        let settlement = self.venue.wallet_mut().settle(venue_millis(at_ts_us));
        let mut messages = vec![InboundMessage::Exec(stream_ready(at_ts_us))];
        let rows: Vec<Balance> = settlement
            .balances
            .iter()
            .map(|balance| Balance {
                asset: asset_name(decode.assets, balance.asset).into(),
                free: decimal(balance.free).into(),
                locked: decimal(balance.locked).into(),
            })
            .collect();
        let chunks = account_snapshot_chunks(&rows, settlement.update_ts_ms as i64, &decode)
            .expect("the wallet's own balances decode: it minted every string in them");
        messages.extend(chunks.into_iter().map(InboundMessage::Account));
        messages.push(InboundMessage::Exec(open_orders_snapshot_end(
            self.instrument,
            at_ts_us,
        )));

        self.fold(&messages, EffectStamp::at(at_ts_us), context.fatal);
        messages
    }

    pub fn owes_nothing(&self) -> bool {
        self.schedule.owes_nothing()
    }
}
