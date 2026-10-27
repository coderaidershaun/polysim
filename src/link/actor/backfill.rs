//! Sends each newly added subscriber the chart memory a few datagrams per flush, so the relay's
//! socket is never flooded.

use std::net::SocketAddr;

use crate::ids::InstrumentId;
use crate::msg::ui::UiHistory;
use crate::time::TsUs;

use super::super::subscribers::LINK_MAX_SUBSCRIBERS;
use super::memory::ChartMemory;

/// About 800 datagrams a second for one subscriber, beside the live feed. An unpaced burst of
/// several hundred datagrams overflows the relay's receive buffer and is lost.
const BACKFILL_DATAGRAMS_PER_FLUSH: u8 = 8;

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[error("backfill table full at {capacity} - refused the backfill for {address}")]
pub(super) struct BackfillRefused {
    address: SocketAddr,
    capacity: usize,
}

/// The part of one instrument's memory this backfill sends, fixed when it reaches the instrument.
struct Span {
    next_index: u64,
    end_index: u64,
    cutoff_ts_us: TsUs,
}

struct Backfill {
    address: SocketAddr,
    slot: usize,
    span: Option<Span>,
    sent_this_flush: u8,
}

impl Backfill {
    /// Instruments go in id order. A sample that left the memory before it was sent is skipped, and
    /// after a rotation the memory's front is past the span's end, which ends that instrument.
    fn next_chunk(&mut self, memory: &ChartMemory) -> Option<UiHistory> {
        while let Some(held) = memory.instrument(self.slot) {
            let Some(span) = self.span.as_mut() else {
                self.span = held.newest_ts_us().map(|cutoff_ts_us| Span {
                    next_index: held.start_index(),
                    end_index: held.end_index(),
                    cutoff_ts_us,
                });
                if self.span.is_none() {
                    self.slot += 1;
                }
                continue;
            };
            span.next_index = span.next_index.max(held.start_index());
            if span.next_index >= span.end_index {
                self.span = None;
                self.slot += 1;
                continue;
            }
            let instrument = InstrumentId(self.slot as u16);
            let chunk = held.chunk(
                instrument,
                span.cutoff_ts_us,
                span.next_index..span.end_index,
            );
            span.next_index += u64::from(chunk.len);
            return Some(chunk);
        }
        None
    }
}

pub(super) struct Backfills {
    active: Vec<Backfill>,
}

impl Backfills {
    pub(super) fn new() -> Self {
        Self {
            active: Vec::with_capacity(LINK_MAX_SUBSCRIBERS),
        }
    }

    /// A backfill already running for `address` starts over.
    ///
    /// # Errors
    /// [`BackfillRefused`] when every slot is taken by another address.
    pub(super) fn start(&mut self, address: SocketAddr) -> Result<(), BackfillRefused> {
        self.active.retain(|backfill| backfill.address != address);
        if self.active.len() >= LINK_MAX_SUBSCRIBERS {
            return Err(BackfillRefused {
                address,
                capacity: LINK_MAX_SUBSCRIBERS,
            });
        }
        self.active.push(Backfill {
            address,
            slot: 0,
            span: None,
            sent_this_flush: 0,
        });
        Ok(())
    }

    pub(super) fn begin_flush(&mut self) {
        for backfill in &mut self.active {
            backfill.sent_this_flush = 0;
        }
    }

    /// The next chunk due this flush, or `None` once every backfill has spent its share or ended.
    /// A backfill with nothing left is dropped here.
    pub(super) fn next_due(&mut self, memory: &ChartMemory) -> Option<(SocketAddr, UiHistory)> {
        let mut index = 0;
        while index < self.active.len() {
            let backfill = &mut self.active[index];
            if backfill.sent_this_flush >= BACKFILL_DATAGRAMS_PER_FLUSH {
                index += 1;
                continue;
            }
            match backfill.next_chunk(memory) {
                Some(chunk) => {
                    backfill.sent_this_flush += 1;
                    return Some((backfill.address, chunk));
                }
                None => {
                    self.active.remove(index);
                }
            }
        }
        None
    }
}
