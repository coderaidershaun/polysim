//! Link hot-path: peer frames recorded here (not at actor, which is upstream of drop+count ring).

use crate::link::InboundLink;
use crate::msg::persist::{LinkFrameRow, LinkRowKind, PersistRecord};

use super::HotEngine;

impl HotEngine {
    /// Peer frame (recorded before callback for tape order; here not at actor to exclude dropped frames).
    pub(super) fn on_link(&mut self, link: &InboundLink) {
        self.record_link_payload(link);
        if !self.is_strategy_live() {
            return;
        }
        let mut ctx = self.state.ctx(link.received_ts_us);
        self.strategy.on_link(&mut ctx, &link.frame);
    }

    fn record_link_payload(&mut self, link: &InboundLink) {
        let origin = link.frame.origin;
        let payload = &link.frame.payload;
        let values = payload.values();
        let count = values.len() as u16;
        for (slot, value) in values.iter().enumerate() {
            self.state
                .actions
                .push_persist(PersistRecord::LinkFrame(LinkFrameRow {
                    kind: LinkRowKind::Payload,
                    sender_te_hash: origin.sender_te_hash.0,
                    topic: origin.topic.0,
                    seq: origin.seq,
                    slot: slot as u16,
                    count,
                    value: *value,
                    event_ts_us: payload.event_ts_us,
                    received_ts_us: link.received_ts_us,
                }));
        }
    }
}
