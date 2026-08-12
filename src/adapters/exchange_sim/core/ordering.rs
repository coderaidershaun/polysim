//! Effective-time ordering for buffered simulator inputs.

use super::request::TimedAction;
use crate::msg::inbound::{InboundMessage, VenueMeta};
use crate::time::TsUs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    DepthReset,
    SnapshotRebuild,
    DeltaCommit,
    Trade,
    OrderAction,
    Reconcile,
}

pub(super) type EventKey = (TsUs, Phase, u64);

pub(super) trait Keyed {
    fn key(&self) -> EventKey;
}

/// Every buffered simulator input is ordered by the same key, so the rule lives here once: two
/// copies of it could drift apart and only one of them would fail a test.
#[derive(Debug, Clone)]
pub(super) struct SequencedByKey<T> {
    entries: Vec<T>,
    sequence: u64,
}

impl<T: Keyed> SequencedByKey<T> {
    fn next_sequence(&mut self) -> u64 {
        let minted = self.sequence;
        self.sequence += 1;
        minted
    }

    fn insert(&mut self, entry: T) {
        let at = self
            .entries
            .partition_point(|held| held.key() <= entry.key());
        self.entries.insert(at, entry);
    }

    fn peek(&self) -> Option<EventKey> {
        self.entries.first().map(T::key)
    }

    fn pop(&mut self) -> Option<T> {
        (!self.entries.is_empty()).then(|| self.entries.remove(0))
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<T> Default for SequencedByKey<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TimedEntry {
    pub at_ts_us: TsUs,
    pub phase: Phase,
    pub sequence: u64,
    pub action: TimedAction,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Timeline {
    entries: SequencedByKey<TimedEntry>,
}

impl Timeline {
    pub fn schedule(&mut self, at_ts_us: TsUs, action: TimedAction) {
        let sequence = self.entries.next_sequence();
        self.entries.insert(TimedEntry {
            at_ts_us,
            phase: phase_of(action),
            sequence,
            action,
        });
    }

    pub fn peek(&self) -> Option<EventKey> {
        self.entries.peek()
    }

    pub fn pop(&mut self) -> Option<TimedEntry> {
        self.entries.pop()
    }
}

impl Keyed for TimedEntry {
    fn key(&self) -> EventKey {
        (self.at_ts_us, self.phase, self.sequence)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BufferedMarket {
    pub at_ts_us: TsUs,
    pub phase: Phase,
    pub sequence: u64,
    pub message: InboundMessage,
    pub venue_meta: VenueMeta,
}

#[derive(Debug, Clone)]
pub(super) struct MarketBuffer {
    events: SequencedByKey<BufferedMarket>,
    capacity: usize,
}

impl MarketBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: SequencedByKey::default(),
            capacity,
        }
    }

    /// # Panics
    /// On exhaustion, per the capacity note above.
    pub fn push(&mut self, at_ts_us: TsUs, phase: Phase, event: (InboundMessage, VenueMeta)) {
        assert!(
            self.events.len() < self.capacity,
            "the simulated venue's market inbox is full at {} events — the safe horizon has lagged \
             its receipts for longer than the declared delay bound allows, and evicting one would \
             silently change which orders filled",
            self.capacity
        );
        let sequence = self.events.next_sequence();
        self.events.insert(BufferedMarket {
            at_ts_us,
            phase,
            sequence,
            message: event.0,
            venue_meta: event.1,
        });
    }

    pub fn peek(&self) -> Option<EventKey> {
        self.events.peek()
    }

    pub fn pop(&mut self) -> Option<BufferedMarket> {
        self.events.pop()
    }
}

impl Keyed for BufferedMarket {
    fn key(&self) -> EventKey {
        (self.at_ts_us, self.phase, self.sequence)
    }
}

fn phase_of(action: TimedAction) -> Phase {
    match action {
        TimedAction::Activate(_) | TimedAction::Cancel(_) | TimedAction::Amend { .. } => {
            Phase::OrderAction
        }
        TimedAction::AnswerOrderStatus(_) | TimedAction::AnswerOpenOrders => Phase::Reconcile,
    }
}
