//! Market-data intake for the simulated venue.

use rtrb::Consumer;

use super::lanes::SimLane;
use crate::msg::inbound::{BookChunkKind, InboundMessage, MarketTapItem, TappedMessage, VenueMeta};
use crate::time::TsUs;

pub struct MarketTapLane {
    consumer: Consumer<MarketTapItem>,
    drained: Vec<TappedMessage>,
    watermark_ts_us: TsUs,
    lane: SimLane,
}

impl MarketTapLane {
    pub fn new(consumer: Consumer<MarketTapItem>, lane: SimLane) -> Self {
        Self {
            consumer,
            drained: Vec::new(),
            watermark_ts_us: TsUs::from_micros(i64::MIN),
            lane,
        }
    }

    /// The batch never escapes: leaving it with the caller is how the reusable buffer gets lost and
    /// every later drain allocates a fresh one.
    pub(super) fn drain(&mut self, visit: impl FnOnce(&[TappedMessage])) {
        let mut batch = std::mem::take(&mut self.drained);
        self.drain_into(&mut batch);
        visit(&batch);
        self.drained = batch;
    }

    fn drain_into(&mut self, drained: &mut Vec<TappedMessage>) {
        drained.clear();
        while let Ok(item) = self.consumer.pop() {
            let proof = match item {
                MarketTapItem::Watermark { received_ts_us } => received_ts_us,
                MarketTapItem::Event(tapped) => {
                    let TappedMessage {
                        message,
                        venue_meta,
                    } = tapped;
                    assert!(
                        has_required_evidence(&message, venue_meta),
                        "{} lane event {message:?} arrived with venue metadata {venue_meta:?} — \
                         its continuity evidence was lost",
                        self.lane
                    );
                    drained.push(tapped);
                    message.received_ts_us()
                }
            };
            assert!(
                proof >= self.watermark_ts_us,
                "{} lane rewound from {} to {} — the producer clamps, so this is a bug",
                self.lane,
                self.watermark_ts_us.micros(),
                proof.micros()
            );
            self.watermark_ts_us = proof;
        }
    }

    pub fn proven_watermark_ts_us(&self) -> Option<TsUs> {
        (self.watermark_ts_us != TsUs::from_micros(i64::MIN)).then_some(self.watermark_ts_us)
    }

    pub fn is_producer_gone(&self) -> bool {
        self.consumer.is_abandoned()
    }
}

fn has_required_evidence(message: &InboundMessage, venue_meta: VenueMeta) -> bool {
    match message {
        InboundMessage::Trade(_) => matches!(venue_meta, VenueMeta::Trade { .. }),
        InboundMessage::BookReset(_) => matches!(venue_meta, VenueMeta::DepthReset { .. }),
        InboundMessage::Book(chunk) => match chunk.kind {
            BookChunkKind::Delta => matches!(venue_meta, VenueMeta::DepthDelta { .. }),
            BookChunkKind::Snapshot => venue_meta == VenueMeta::None,
        },
        _ => venue_meta == VenueMeta::None,
    }
}
