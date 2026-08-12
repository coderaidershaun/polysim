//! Feature store + channel histories. Pure data; folds events in event time (never wall clock).

use super::history::BoundedHistory;
use crate::hot::exec::{OrderState, QuoteLevel, RejectOrigin};
use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use crate::msg::exec::Liquidity;
use crate::msg::persist::FeatureId;
use crate::msg::ui::{UiBookState, UiEvent};
use crate::time::{DurationUs, TsUs};

/// Trade tape capacity (hundreds/5min; scrollback not ledger).
const TRADE_TAPE_CAPACITY: usize = 256;

/// System/Order/Fill history capacities (256 rows, oldest evict).
const SYSTEM_CAPACITY: usize = 256;
const ORDER_CAPACITY: usize = 256;
const FILL_CAPACITY: usize = 256;

/// Value + freshness ts. last_changed on diff (re-emissions bump last_update).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeatureCell {
    pub value: f64,
    pub last_update_ts: TsUs,
    pub last_changed_ts: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeRow {
    pub at: TsUs,
    pub aggressor: Side,
    pub price: Price,
    pub qty: Qty,
}

/// Transition/refusal (Orders channel, same question, one tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderEvent {
    Transition {
        client_id: ClientOrderId,
        state: OrderState,
        price: Price,
        qty: Qty,
        filled: Qty,
    },
    Refused {
        origin: RejectOrigin,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderRow {
    pub at: TsUs,
    pub instrument: InstrumentId,
    pub quote_level: Option<QuoteLevel>,
    pub side: Side,
    pub event: OrderEvent,
}

/// Venue execution (commission in commission_asset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillRow {
    pub at: TsUs,
    pub instrument: InstrumentId,
    pub quote_level: Option<QuoteLevel>,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub commission: i64,
    pub commission_asset: AssetId,
    /// None when venue didn't say. Absent ≠ "taker" (fee differs).
    pub liquidity: Option<Liquidity>,
}

/// Framework-free lifecycle transition (app hands to System channel, model never reads it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemNote {
    Starting,
    Ready,
    Draining { reason: Box<str> },
    Stopped { graceful: bool, reason: Box<str> },
}

/// System-channel row. Typed not pre-formatted (view renders, fitness pins values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemEvent {
    Lifecycle(SystemNote),
    Rotation {
        instrument: InstrumentId,
    },
    /// Book AwaitingSnapshot->Valid round trip (one row).
    BookResynced {
        instrument: InstrumentId,
    },
    EventsLost {
        count: u64,
    },
    BooksLost {
        count: u64,
    },
}

/// System-channel row. `at` = source event time (rotations/book/gaps) or None (lifecycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemRow {
    pub at: Option<TsUs>,
    pub event: SystemEvent,
}

/// Feature values + channel histories. Per-instrument storage grows on demand before catalog.
pub struct MonitorModel {
    feature_count: usize,
    features: Vec<Option<FeatureCell>>,
    latest_feed_ts_us: Vec<Option<TsUs>>,
    book_state: Vec<Option<UiBookState>>,
    trades: Vec<BoundedHistory<TradeRow>>,
    system: BoundedHistory<SystemRow>,
    orders: BoundedHistory<OrderRow>,
    fills: BoundedHistory<FillRow>,
    spin_interval: DurationUs,
}

impl MonitorModel {
    /// Pre-size for instruments and features at known spin cadence (framework-free seam for fitness).
    pub fn with_capacity(
        instrument_count: usize,
        feature_count: usize,
        spin_interval: DurationUs,
    ) -> Self {
        Self {
            feature_count,
            features: vec![None; instrument_count * feature_count],
            latest_feed_ts_us: vec![None; instrument_count],
            book_state: vec![None; instrument_count],
            trades: (0..instrument_count)
                .map(|_| BoundedHistory::new(TRADE_TAPE_CAPACITY))
                .collect(),
            system: BoundedHistory::new(SYSTEM_CAPACITY),
            orders: BoundedHistory::new(ORDER_CAPACITY),
            fills: BoundedHistory::new(FILL_CAPACITY),
            spin_interval,
        }
    }

    /// Adopt run dimensions once catalog arrives (spin cadence, feature width). Grid rebuild discards nothing (empty at startup).
    pub(crate) fn configure(
        &mut self,
        instrument_count: usize,
        feature_count: usize,
        spin_interval: DurationUs,
    ) {
        self.spin_interval = spin_interval;
        self.feature_count = feature_count;
        let instruments = instrument_count.max(self.latest_feed_ts_us.len());
        self.latest_feed_ts_us.resize(instruments, None);
        self.book_state.resize(instruments, None);
        while self.trades.len() < instruments {
            self.trades.push(BoundedHistory::new(TRADE_TAPE_CAPACITY));
        }
        self.features = vec![None; instruments * feature_count];
    }

    pub(crate) fn apply_event(&mut self, event: &UiEvent) {
        match *event {
            UiEvent::Quote {
                instrument,
                event_ts_us,
                ..
            } => self.touch_feed(instrument, event_ts_us),
            UiEvent::Trade {
                instrument,
                event_ts_us,
                aggressor,
                price,
                qty,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.trades[instrument.0 as usize].push(TradeRow {
                    at: event_ts_us,
                    aggressor,
                    price,
                    qty,
                });
            }
            UiEvent::OrderUpdate {
                instrument,
                event_ts_us,
                client_id,
                quote_level,
                side,
                state,
                price,
                qty,
                filled,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.orders.push(OrderRow {
                    at: event_ts_us,
                    instrument,
                    quote_level,
                    side,
                    event: OrderEvent::Transition {
                        client_id,
                        state,
                        price,
                        qty,
                        filled,
                    },
                });
            }
            UiEvent::OrderSnapshot {
                instrument,
                event_ts_us,
                ..
            } => self.touch_feed(instrument, event_ts_us),
            UiEvent::Reject {
                instrument,
                event_ts_us,
                side,
                origin,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.orders.push(OrderRow {
                    at: event_ts_us,
                    instrument,
                    quote_level: None,
                    side,
                    event: OrderEvent::Refused { origin },
                });
            }
            UiEvent::Balance { .. } | UiEvent::Execution { .. } | UiEvent::Latency { .. } => {}
            UiEvent::Feature {
                instrument,
                event_ts_us,
                feature,
                value,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.apply_feature(instrument, feature, value, event_ts_us);
            }
            UiEvent::Fill {
                instrument,
                event_ts_us,
                quote_level,
                side,
                price,
                qty,
                commission,
                commission_asset,
                liquidity,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.fills.push(FillRow {
                    at: event_ts_us,
                    instrument,
                    quote_level,
                    side,
                    price,
                    qty,
                    commission,
                    commission_asset,
                    liquidity,
                });
            }
            UiEvent::Rotation {
                instrument,
                event_ts_us,
                ..
            } => {
                self.touch_feed(instrument, event_ts_us);
                self.system.push(SystemRow {
                    at: Some(event_ts_us),
                    event: SystemEvent::Rotation { instrument },
                });
            }
            UiEvent::Position {
                instrument,
                event_ts_us,
                ..
            } => self.touch_feed(instrument, event_ts_us),
        }
    }

    /// Record round trip (atomic repair row). Drop silent; one row per round-trip (not per direction) reduces noise.
    pub(crate) fn observe_book_state(
        &mut self,
        instrument: InstrumentId,
        state: UiBookState,
        at: TsUs,
    ) {
        let index = instrument.0 as usize;
        self.ensure_instruments(index + 1);
        let previous = self.book_state[index].replace(state);
        if previous != Some(UiBookState::AwaitingSnapshot) || state != UiBookState::Valid {
            return;
        }
        self.system.push(SystemRow {
            at: Some(at),
            event: SystemEvent::BookResynced { instrument },
        });
    }

    pub fn book_state(&self, instrument: InstrumentId) -> Option<UiBookState> {
        self.book_state
            .get(instrument.0 as usize)
            .copied()
            .flatten()
    }

    pub(crate) fn note_events_lost(&mut self, count: u64, at: TsUs) {
        self.system.push(SystemRow {
            at: Some(at),
            event: SystemEvent::EventsLost { count },
        });
    }

    pub(crate) fn note_books_lost(&mut self, count: u64, at: TsUs) {
        self.system.push(SystemRow {
            at: Some(at),
            event: SystemEvent::BooksLost { count },
        });
    }

    pub(crate) fn note_lifecycle(&mut self, note: SystemNote) {
        self.system.push(SystemRow {
            at: None,
            event: SystemEvent::Lifecycle(note),
        });
    }

    /// Feature value for instrument (None = never emitted or id out of range).
    pub fn feature(&self, instrument: InstrumentId, feature: FeatureId) -> Option<FeatureCell> {
        if self.feature_count == 0 || feature.0 as usize >= self.feature_count {
            return None;
        }
        let index = instrument.0 as usize * self.feature_count + feature.0 as usize;
        self.features.get(index).copied().flatten()
    }

    pub fn feature_count(&self) -> usize {
        self.feature_count
    }

    /// Newest event time instrument produced (event-time ref for feature freshness).
    pub fn latest_feed_ts_us(&self, instrument: InstrumentId) -> Option<TsUs> {
        self.latest_feed_ts_us
            .get(instrument.0 as usize)
            .copied()
            .flatten()
    }

    pub fn spin_interval(&self) -> DurationUs {
        self.spin_interval
    }

    /// Instrument's public-print tape, newest-first (empty if past storage).
    pub fn trades(&self, instrument: InstrumentId) -> impl Iterator<Item = &TradeRow> {
        self.trades
            .get(instrument.0 as usize)
            .into_iter()
            .flat_map(BoundedHistory::iter_newest_first)
    }

    pub fn system(&self) -> impl Iterator<Item = &SystemRow> {
        self.system.iter_newest_first()
    }

    pub fn orders(&self) -> impl Iterator<Item = &OrderRow> {
        self.orders.iter_newest_first()
    }

    pub fn fills(&self) -> impl Iterator<Item = &FillRow> {
        self.fills.iter_newest_first()
    }

    /// Public prints ever appended (monotonic basis for Pub trades unseen count).
    pub fn trades_appended(&self, instrument: InstrumentId) -> u64 {
        self.trades
            .get(instrument.0 as usize)
            .map_or(0, BoundedHistory::appended)
    }

    pub fn system_appended(&self) -> u64 {
        self.system.appended()
    }

    pub fn orders_appended(&self) -> u64 {
        self.orders.appended()
    }

    pub fn fills_appended(&self) -> u64 {
        self.fills.appended()
    }

    fn touch_feed(&mut self, instrument: InstrumentId, at: TsUs) {
        let index = instrument.0 as usize;
        self.ensure_instruments(index + 1);
        let slot = &mut self.latest_feed_ts_us[index];
        if slot.is_none_or(|existing| at.micros() > existing.micros()) {
            *slot = Some(at);
        }
    }

    fn apply_feature(
        &mut self,
        instrument: InstrumentId,
        feature: FeatureId,
        value: f64,
        at: TsUs,
    ) {
        if self.feature_count == 0 || feature.0 as usize >= self.feature_count {
            return;
        }
        let index = instrument.0 as usize * self.feature_count + feature.0 as usize;
        match self.features[index] {
            Some(ref mut cell) => {
                // Bit equality not == (re-emitted values including NaN = unchanged, -0.0/+0.0 differ).
                let changed = cell.value.to_bits() != value.to_bits();
                cell.value = value;
                cell.last_update_ts = at;
                if changed {
                    cell.last_changed_ts = at;
                }
            }
            None => {
                self.features[index] = Some(FeatureCell {
                    value,
                    last_update_ts: at,
                    last_changed_ts: at,
                });
            }
        }
    }

    fn ensure_instruments(&mut self, len: usize) {
        if self.latest_feed_ts_us.len() >= len {
            return;
        }
        self.latest_feed_ts_us.resize(len, None);
        self.book_state.resize(len, None);
        while self.trades.len() < len {
            self.trades.push(BoundedHistory::new(TRADE_TAPE_CAPACITY));
        }
        self.features.resize(len * self.feature_count, None);
    }
}
