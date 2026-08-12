//! Framework-free vocabulary: lifecycle phases, instrument catalog, live book/quote feed.
//! Zero egui -> engine independent of GUI toolkit. Transport types live here, survive out-of-process workstation.

// No CONSUMER without ui feature -> workstation separate process. Engine produces unconditionally:
// hot path never knows if listening. ui gate still lints, catches dead code.
#![cfg_attr(not(feature = "ui"), allow(dead_code))]

mod latency;

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use rtrb::{Consumer, RingBuffer};

use crate::config::{ExecutionMode, StrategyId};
use crate::hot::exec::{ExecHalt, OrderState, QuoteLevel, RejectOrigin};
use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use crate::msg::exec::Liquidity;
use crate::msg::inbound::Level;
use crate::msg::persist::FeatureId;
use crate::registry::Registry;
use crate::runtime::ExitReport;
use crate::shutdown::ShutdownRequest;
use crate::sink::{UiBookSink, UiEventSink};
use crate::time::TsUs;

pub use latency::{UiLatencyCell, UiLatencyRow, UiLatencySummary};

/// Lifecycle messages handful per run; small ring absorbs burst. UiWiring try_sends -> full drops not stalls.
const LIFECYCLE_CAPACITY: usize = 16;

/// Book levels per side. DOM shows 12/side but grouped rows aggregate ticks; 16-level feed shows half-empty.
pub const UI_BOOK_LEVELS: usize = 32;

/// Desired quote levels per side, mirrors execution ladder hard capacity.
pub const UI_QUOTE_LEVELS: usize = 8;

/// Detailed working orders in one atomic snapshot, matches execution ladder capacity. Invariant breach still reported.
pub const UI_ORDER_SNAPSHOT_CAPACITY: usize = UI_QUOTE_LEVELS;

/// Defensive max per side total_working: 32 current-run + 256 prior-run repair slots. Normal one per side.
pub const UI_ORDER_SNAPSHOT_MAX_TOTAL: u16 = 288;

/// Book-snapshot ring: ~tens commits/s per 5-min run. Full = drop+count, not stall.
pub const UI_BOOK_RING_CAPACITY: usize = 1024;

/// Event ring: per-print trades + per-spin features (≥63/spin) hundreds events/s. Full = drop+count.
pub const UI_EVENT_RING_CAPACITY: usize = 16_384;

/// Rare transitions; engine reports on itself. Draining/Stopped carry engine's drain reason + ExitReport.
/// No startup phase: UI attaches to running engine.
#[derive(Debug, Clone)]
pub(crate) enum UiLifecycle {
    Ready(UiCatalog),
    Draining { reason: Box<str> },
    Stopped(ExitReport),
}

/// Static run shape snapshotted from frozen Registry post-preflight. Public: out-of-process workstation
/// rebuilds from link frames, fitness suite drives assembly.
#[derive(Debug, Clone)]
pub struct UiCatalog {
    pub strategy_id: Box<str>,
    pub window_title: Box<str>,
    /// Drives the local and linked execution-mode badge.
    pub execution_mode: Option<ExecutionMode>,
    pub spin_interval_us: u64,
    pub instruments: Vec<UiInstrument>,
    /// Feature names indexed by dense FeatureId, source strategy startup. Monitor labels features.
    pub feature_names: Vec<Box<str>>,
}

impl UiCatalog {
    pub fn instrument(&self, instrument: InstrumentId) -> Option<&UiInstrument> {
        self.instruments
            .iter()
            .find(|candidate| candidate.instrument_id == instrument)
    }

    /// Snapshot frozen registry post-preflight. feature_names = strategy dense-id dict, read pre-engine.
    pub(crate) fn from_registry(
        strategy_id: &StrategyId,
        execution_mode: Option<ExecutionMode>,
        spin_interval_us: u64,
        feature_names: Vec<Box<str>>,
        registry: &Registry,
    ) -> Self {
        let instruments = registry
            .instruments()
            .iter()
            .map(|row| UiInstrument {
                instrument_id: row.instrument_id,
                display: row.display.clone(),
                base: row.base.clone(),
                quote: row.quote.clone(),
                base_asset: row.base_asset,
                quote_asset: row.quote_asset,
                tick_size: row.tick_size,
                lot_size: row.lot_size,
                qty_scale: row.qty_scale,
            })
            .collect();
        Self {
            strategy_id: strategy_id.as_str().into(),
            window_title: format!("Polysim — {}", strategy_id.as_str()).into(),
            execution_mode,
            spin_interval_us,
            instruments,
            feature_names,
        }
    }
}

/// Registry row reduced to what UI renders. tick_size/lot_size None until venue stamps (preflight/later).
/// base_asset/quote_asset index Balance lane.
#[derive(Debug, Clone)]
pub struct UiInstrument {
    pub instrument_id: InstrumentId,
    pub display: Box<str>,
    pub base: Box<str>,
    pub quote: Box<str>,
    pub base_asset: AssetId,
    pub quote_asset: AssetId,
    pub tick_size: Option<Price>,
    pub lot_size: Option<Qty>,
    pub qty_scale: i64,
}

/// Book reconstructability. Mirrors engine state so UI dims awaiting snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiBookState {
    AwaitingSnapshot,
    Valid,
}

/// Top-of-book copied at commit, fixed-size+Copy for hot ring stamp, no alloc. seq = per-instrument lane counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiBookSnapshot {
    pub instrument: InstrumentId,
    pub seq: u64,
    pub event_ts_us: TsUs,
    pub state: UiBookState,
    pub bid_len: u16,
    pub ask_len: u16,
    pub bids: [Level; UI_BOOK_LEVELS],
    pub asks: [Level; UI_BOOK_LEVELS],
}

// Copy cost bounded; 32-level cap keeps it here.
const _: () = assert!(size_of::<UiBookSnapshot>() <= 1048);

/// Desired ladder for one instrument. Array index = stable quote level; None = withdraw.
/// Desire ≠ truth: OrderUpdate arrives when venue confirms. Shown apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DomQuote {
    pub bids: [Option<(Price, Qty)>; UI_QUOTE_LEVELS],
    pub asks: [Option<(Price, Qty)>; UI_QUOTE_LEVELS],
}

impl DomQuote {
    /// Construct one-level ladder without partial wire init.
    #[inline]
    pub fn top(bid: Option<(Price, Qty)>, ask: Option<(Price, Qty)>) -> Self {
        let mut quote = Self::default();
        quote.bids[0] = bid;
        quote.asks[0] = ask;
        quote
    }
}

/// Detailed row in atomic working-order snapshot. quote_level optional for inherited orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiWorkingOrder {
    pub client_id: ClientOrderId,
    pub quote_level: Option<QuoteLevel>,
    pub state: OrderState,
    pub price: Price,
    pub qty: Qty,
    pub filled: Qty,
}

impl UiWorkingOrder {
    pub const EMPTY: Self = Self {
        client_id: ClientOrderId(0),
        quote_level: None,
        state: OrderState::Free,
        price: Price(0),
        qty: Qty(0),
        filled: Qty(0),
    };
}

/// Engine→UI event on ordered lane. seq = lane-wide monotonic counter across all kinds.
/// Pure function of message sequence. Not Eq: Feature + Position carry f64.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    Quote {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        quote: DomQuote,
    },
    /// Public print, teed pre-warmup gate.
    Trade {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        aggressor: Side,
        price: Price,
        qty: Qty,
    },
    /// Engine order after venue event. state = engine table not wire: distinguishes venue-confirmed vs in-flight.
    OrderUpdate {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        client_id: ClientOrderId,
        /// Stable strategy ladder level; absent for inherited/adopted orders that predate this run.
        quote_level: Option<QuoteLevel>,
        side: Side,
        state: OrderState,
        price: Price,
        qty: Qty,
        filled: Qty,
    },
    /// One side's complete order set at spin boundary. Detailed prefix bounded; total_working exact.
    /// One event = one datagram -> no partial snapshot loss. Previous persists until next atomic cut.
    OrderSnapshot {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        side: Side,
        detail_len: u8,
        total_working: u16,
        orders: [UiWorkingOrder; UI_ORDER_SNAPSHOT_CAPACITY],
    },
    /// Feature row teed off persist lane; monitor shows value landing in Parquet.
    Feature {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        feature: FeatureId,
        value: f64,
    },
    /// Real execution on engine order. price/qty = fill not order. commission in commission_asset (≠quote).
    Fill {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        /// Stable strategy ladder level; absent when the filled order was inherited/adopted.
        quote_level: Option<QuoteLevel>,
        side: Side,
        price: Price,
        qty: Qty,
        commission: i64,
        commission_asset: AssetId,
        /// None if venue didn't say. Absent ≠ "taker".
        liquidity: Option<Liquidity>,
    },
    /// Asset absolute balance at venue, re-sent whole on move. Never delta: loss = permanent money wrong.
    Balance {
        asset: AssetId,
        seq: u64,
        event_ts_us: TsUs,
        free: i64,
        locked: i64,
    },
    /// Refused quote; engine gate or venue. Separate kind: refusal ≠ order transition.
    Reject {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        side: Side,
        origin: RejectOrigin,
    },
    /// Execution kill switch, re-sent every spin when wired. Absolute state: dropped frame heals next spin.
    Execution {
        seq: u64,
        event_ts_us: TsUs,
        halt: ExecHalt,
    },
    /// An instrument's window rotated; the monitor's System channel records the handover.
    Rotation {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
    },
    /// Instrument mark state, re-sent every spin. ABSOLUTE not increments: drop = never sent.
    /// f64: display quantities only; exact mantissas stay in engine.
    Position {
        instrument: InstrumentId,
        seq: u64,
        event_ts_us: TsUs,
        exposure_quote: f64,
        pnl_quote: f64,
    },
    /// Engine self-timing, re-sent every spin. Absolute rollups: a dropped frame heals next spin.
    Latency {
        seq: u64,
        event_ts_us: TsUs,
        summary: UiLatencySummary,
    },
}

/// Named once so the three accessors below cannot drift apart on which kinds they cover.
macro_rules! every_ui_event {
    ($field:ident) => {
        UiEvent::Quote { $field, .. }
            | UiEvent::Trade { $field, .. }
            | UiEvent::OrderUpdate { $field, .. }
            | UiEvent::OrderSnapshot { $field, .. }
            | UiEvent::Feature { $field, .. }
            | UiEvent::Fill { $field, .. }
            | UiEvent::Balance { $field, .. }
            | UiEvent::Reject { $field, .. }
            | UiEvent::Execution { $field, .. }
            | UiEvent::Rotation { $field, .. }
            | UiEvent::Position { $field, .. }
            | UiEvent::Latency { $field, .. }
    };
}

impl UiEvent {
    /// Lane-wide sequence across kinds. Consumer tracks drop gaps; reads seq before variant.
    #[inline]
    pub fn seq(&self) -> u64 {
        match self {
            every_ui_event!(seq) => *seq,
        }
    }

    /// Restamp a replayed event onto the consumer's lane. Fixtures build events kind-first and
    /// number them afterwards; without this they rebuild every variant to move one field.
    #[inline]
    pub fn set_seq(&mut self, seq: u64) {
        let slot = match self {
            every_ui_event!(seq) => seq,
        };
        *slot = seq;
    }

    /// Event time regardless of kind. Read to stamp gap notes when sequence jumps.
    #[inline]
    pub fn event_ts_us(&self) -> TsUs {
        match self {
            every_ui_event!(event_ts_us) => *event_ts_us,
        }
    }
}

// Quote is widest variant: 16 levels cost more but bounded publication + alloc-free complete ladder.
const _: () = assert!(size_of::<UiEvent>() <= 416);

/// Engine end of UI seam: shutdown latch, lifecycle sender, two hot→UI feed producers.
pub(crate) struct UiWiring {
    pub shutdown: ShutdownRequest,
    pub lifecycle: SyncSender<UiLifecycle>,
    pub books: UiBookSink,
    pub events: UiEventSink,
}

/// Consumer end of UI seam: lifecycle receiver + two consumers. No ShutdownRequest (out-of-process).
pub(crate) struct UiChannels {
    pub lifecycle: Receiver<UiLifecycle>,
    pub books: Consumer<UiBookSnapshot>,
    pub events: Consumer<UiEvent>,
}

/// Build paired ends of UI seam: lifecycle channel, two pre-allocated hot→UI rings, shutdown latch.
pub(crate) fn ui_channel() -> (UiWiring, UiChannels) {
    let (lifecycle_tx, lifecycle_rx) = sync_channel(LIFECYCLE_CAPACITY);
    let (book_producer, book_consumer) = RingBuffer::<UiBookSnapshot>::new(UI_BOOK_RING_CAPACITY);
    let (event_producer, event_consumer) = RingBuffer::<UiEvent>::new(UI_EVENT_RING_CAPACITY);
    (
        UiWiring {
            shutdown: ShutdownRequest::new(),
            lifecycle: lifecycle_tx,
            books: UiBookSink::new(book_producer),
            events: UiEventSink::new(event_producer),
        },
        UiChannels {
            lifecycle: lifecycle_rx,
            books: book_consumer,
            events: event_consumer,
        },
    )
}
