//! Hot→UI feed fitness: the engine emits a book snapshot at every commit boundary (and on
//! reset/rotation) with a per-instrument monotonic sequence, and tees the ordered event lane —
//! quotes, per-print trades, real order transitions, feature rows, REAL venue fills and rotations —
//! each stamped with one lane-wide sequence so a consumer counts ring-drop gaps across every kind.
//! Both rings and every lane drop and count rather than stall the hot thread. All of it is a pure
//! function of the message sequence, so replay reproduces the feed byte for byte.
//!
//! A fill is a venue message the engine tees directly, on the fill's own message rather than on the
//! print that inspired it. Every test below that wanted a position drives [`FillPen`] for that
//! reason.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use polysim::config::{RecordedTables, TableKind};
#[cfg(feature = "ui")]
use polysim::desktop::model::UiModel;
use polysim::hot::dispatch::{ExecWiring, HotEngine, HotEngineSetup};
use polysim::hot::exec::{
    CloseReason, DesiredQuote, ExecHalt, ExecSettings, OrderState, QuoteLevel,
};
use polysim::hot::strategy::{DomQuote, Registration, Strategy, StrategyCtx};
use polysim::ids::{AssetId, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{ExecKind, ExecLaneItem, Liquidity, OrderStyle, VenueOrderStatus};
use polysim::msg::inbound::{InboundMessage, Level, SpinTick};
use polysim::msg::persist::FeatureId;
use polysim::msg::ui::{UiBookSnapshot, UiBookState, UiEvent, UiLatencySummary};
use polysim::sink::ExecSink;
use polysim::time::{DurationUs, TsUs};
use rtrb::{Consumer, RingBuffer};

use crate::engine_support::{
    FillPen, ONE, book_reset, delta_chunk, detached_exposure, engine_with_ui, exec_event, idle_at,
    instrument_row, metrics_ring, persist_ring, persist_ring_for, pop, rotation, run_control,
    snapshot_pair, spin, spin_pop, strategy_log_ring, tracker_spec_all, trade, ts, ui_book_ring,
    ui_event_ring,
};

/// A strategy that touches nothing — book snapshots are engine-driven, so the book tests need no
/// callback behaviour.
struct Idle;
impl Strategy for Idle {}

/// Counts strategy callbacks independently of the UI projection. A fill may tee two UI facts, but
/// it remains exactly one strategy callback — otherwise a strategy which responds to both updates
/// and fills would act twice on one venue event.
struct FillCallbackProbe {
    fills: Arc<AtomicUsize>,
    updates: Arc<AtomicUsize>,
}

impl Strategy for FillCallbackProbe {
    fn on_fill(&mut self, _ctx: &mut StrategyCtx<'_>, _fill: &polysim::hot::strategy::Fill) {
        self.fills.fetch_add(1, Ordering::Relaxed);
    }

    fn on_order_update(
        &mut self,
        _ctx: &mut StrategyCtx<'_>,
        _update: &polysim::hot::strategy::OrderUpdate,
    ) {
        self.updates.fetch_add(1, Ordering::Relaxed);
    }
}

/// Declares a two-sided desired quote for instrument 0 every spin. The strategy paints nothing —
/// it declares, and the ENGINE tees what it is trying to hold.
struct QuoteProbe {
    bid: Option<(Price, Qty)>,
    ask: Option<(Price, Qty)>,
}

impl QuoteProbe {
    fn declare(ctx: &mut StrategyCtx<'_>, side: Side, level: Option<(Price, Qty)>) {
        ctx.quote(
            InstrumentId(0),
            side,
            QuoteLevel::ZERO,
            level.map(|(price, qty)| DesiredQuote {
                price,
                qty,
                style: OrderStyle::PostOnly,
            }),
        );
    }
}

impl Strategy for QuoteProbe {
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        Self::declare(ctx, Side::Buy, self.bid);
        Self::declare(ctx, Side::Sell, self.ask);
    }
}

/// Emits three distinct feature rows per spin, so the Feature tee's count, order and values can be
/// pinned against the emit order.
struct FeatureProbe {
    ids: Vec<FeatureId>,
}
impl Strategy for FeatureProbe {
    fn features(&self) -> &'static [&'static str] {
        &["alpha", "beta", "gamma"]
    }
    fn register(&mut self, registration: Registration<'_>) {
        self.ids = registration.features.to_vec();
    }
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        ctx.emit(self.ids[0], InstrumentId(0), 1.5);
        ctx.emit(self.ids[1], InstrumentId(0), 2.5);
        ctx.emit(self.ids[2], InstrumentId(0), 3.5);
    }
}

/// Exercises the strategy-origin tees at once (feature, quote) so a mixed feed's lane-wide sequence
/// can be checked for global monotonicity across kinds. The order and fill kinds ride in on their own
/// messages beside it — no strategy can produce one, which is the point of the seam.
struct MixedProbe {
    feature: Option<FeatureId>,
}
impl Strategy for MixedProbe {
    fn features(&self) -> &'static [&'static str] {
        &["mixed"]
    }
    fn register(&mut self, registration: Registration<'_>) {
        self.feature = registration.features.first().copied();
    }
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        ctx.emit(self.feature.expect("registered"), InstrumentId(0), 7.0);
        QuoteProbe::declare(ctx, Side::Buy, Some((Price(100 * ONE), Qty(ONE))));
    }
}

fn pop_books(consumer: &mut Consumer<UiBookSnapshot>) -> Vec<UiBookSnapshot> {
    std::iter::from_fn(|| consumer.pop().ok()).collect()
}

fn pop_events(consumer: &mut Consumer<UiEvent>) -> Vec<UiEvent> {
    std::iter::from_fn(|| consumer.pop().ok()).collect()
}

/// Every Position event on the lane as `(instrument, event time, exposure, PnL)`, in lane order.
fn positions(consumer: &mut Consumer<UiEvent>) -> Vec<(InstrumentId, TsUs, f64, f64)> {
    pop_events(consumer)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Position {
                instrument,
                event_ts_us,
                exposure_quote,
                pnl_quote,
                ..
            } => Some((instrument, event_ts_us, exposure_quote, pnl_quote)),
            _ => None,
        })
        .collect()
}

fn level(price: i64, qty: i64) -> Level {
    Level {
        price: Price(price),
        qty: Qty(qty),
    }
}

/// A one-instrument engine wired to keep every UI feed consumer; `strategy` decides whether quotes
/// flow. Warmup zero unless a test wants suppression.
fn ui_engine(
    strategy: Box<dyn Strategy>,
    warmup: DurationUs,
) -> (HotEngine, Consumer<UiBookSnapshot>, Consumer<UiEvent>) {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    let (sink, _persist) = persist_ring(256);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (engine, books, events) =
        engine_with_ui(&instruments, strategy, sink, log_sink, metrics, warmup);
    // The persist/log/metrics consumers are dropped here on purpose: these tests inspect only the UI
    // feed, and those lanes fill-then-drop harmlessly.
    (engine, books, events)
}

/// The same engine WITH a command ring, i.e. execution wired.
///
/// `exec.sink` is the OUTBOUND command ring and nothing else, which is why [`ui_engine`] can fold
/// real venue fills without one — a fill is money whether or not this engine is the one quoting. So
/// "execution is off" is exactly `sink.is_none()`, and it is the only thing separating these two
/// fixtures. Keep them apart: collapsing them back into one loses the ability to assert what an
/// unwired engine must NOT say.
fn ui_engine_with_exec(
    strategy: Box<dyn Strategy>,
) -> (HotEngine, Consumer<UiEvent>, Consumer<ExecLaneItem>) {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    let (persist, _persist_out) = persist_ring(256);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (book_sink, _books) = ui_book_ring(256);
    let (event_sink, events) = ui_event_ring(1024);
    let (commands_producer, commands) = RingBuffer::<ExecLaneItem>::new(256);
    let engine = HotEngine::new(HotEngineSetup {
        exec: Some(ExecWiring {
            sink: ExecSink::new(commands_producer),
            settings: ExecSettings::disabled(),
            run_nonce: 0x5150_0001,
        }),
        exposure: detached_exposure(),
        instruments: &instruments,
        strategy,
        persistence: Some(persist),
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink: book_sink,
        ui_event_sink: event_sink,
        link: None,
        warmup: DurationUs::ZERO,
    });
    (engine, events, commands)
}

fn execution_frames(consumer: &mut Consumer<UiEvent>) -> Vec<ExecHalt> {
    pop_events(consumer)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Execution { halt, .. } => Some(halt),
            _ => None,
        })
        .collect()
}

/// One committed multi-chunk update emits exactly one snapshot, its top-16 mirrors the book, and a
/// following committed delta bumps the per-instrument sequence.
#[test]
fn snapshot_emitted_only_at_commit_boundary_with_monotonic_seq() {
    let (mut engine, mut books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);

    let (bids, asks) = snapshot_pair(
        0,
        &[(100 * ONE, 2 * ONE), (99 * ONE, ONE)],
        &[(101 * ONE, ONE), (102 * ONE, 3 * ONE)],
        10,
    );
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    assert!(
        pop_books(&mut books).is_empty(),
        "a non-final chunk is mid-update — no snapshot until the commit"
    );

    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    let snaps = pop_books(&mut books);
    assert_eq!(
        snaps.len(),
        1,
        "the commit chunk emits exactly one snapshot"
    );
    let snap = snaps[0];
    assert_eq!(snap.seq, 0, "the first snapshot is sequence 0");
    assert_eq!(snap.instrument, InstrumentId(0));
    assert_eq!(snap.state, UiBookState::Valid);
    assert_eq!(snap.event_ts_us, ts(10));
    assert_eq!((snap.bid_len, snap.ask_len), (2, 2));
    assert_eq!(snap.bids[0], level(100 * ONE, 2 * ONE));
    assert_eq!(snap.bids[1], level(99 * ONE, ONE));
    assert_eq!(snap.asks[0], level(101 * ONE, ONE));
    assert_eq!(snap.asks[1], level(102 * ONE, 3 * ONE));

    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Book(delta_chunk(0, Side::Buy, &[(100 * ONE, 5 * ONE)], 20)),
    );
    let snaps = pop_books(&mut books);
    assert_eq!(snaps.len(), 1, "a committed delta emits the next snapshot");
    assert_eq!(snaps[0].seq, 1, "per-instrument seq is strictly monotonic");
    assert_eq!(snaps[0].bids[0], level(100 * ONE, 5 * ONE));
    assert!(
        pop_events(&mut events).is_empty(),
        "an idle strategy tees no quote events"
    );
}

/// A reset emits an `AwaitingSnapshot` snapshot with empty sides; a rotation emits too.
#[test]
fn reset_emits_awaiting_and_rotation_emits() {
    let (mut engine, mut books, _events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(101 * ONE, ONE)], 10);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    assert_eq!(pop_books(&mut books).len(), 1, "the seed commit emitted");

    engine.dispatch(pop(0, 0), &InboundMessage::BookReset(book_reset(0, 20)));
    let snaps = pop_books(&mut books);
    assert_eq!(snaps.len(), 1, "a reset emits one snapshot");
    assert_eq!(snaps[0].state, UiBookState::AwaitingSnapshot);
    assert_eq!((snaps[0].bid_len, snaps[0].ask_len), (0, 0));
    assert_eq!(snaps[0].seq, 1, "the reset continues the sequence");

    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, 300 * ONE, 600 * ONE, 30)),
    );
    let snaps = pop_books(&mut books);
    assert_eq!(snaps.len(), 1, "a rotation refreshes the UI book");
    assert_eq!(snaps[0].seq, 2);
}

/// Book snapshots ride ahead of the warmup gate, so operators watch the book build before the
/// strategy goes live.
#[test]
fn book_snapshots_emit_during_warmup() {
    // Warmup 10s; all messages land at t=1s, so the strategy stays suppressed the whole test.
    let (mut engine, mut books, _events) = ui_engine(Box::new(Idle), DurationUs::from_secs(10));
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(101 * ONE, ONE)], 1_000_000);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    let snaps = pop_books(&mut books);
    assert_eq!(snaps.len(), 1, "a book commit emits during warmup");
    assert_eq!(snaps[0].state, UiBookState::Valid);
}

/// The whole feed is a pure function of the input sequence: two fresh engines fed an identical mixed
/// sequence — books, trades (Trade + a probe OrderUpdate and Fill), spins (Feature + Quote +
/// Position) and a rotation — emit byte-identical snapshot and event streams, and the
/// streams exercise every kind that mix can reach so the pin cannot pass vacuously.
#[test]
fn replay_produces_identical_feed() {
    let run = || {
        let (mut engine, mut books, mut events) =
            ui_engine(Box::new(MixedProbe { feature: None }), DurationUs::ZERO);
        let mut pen = FillPen::new(0);
        let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(101 * ONE, ONE)], 10);
        engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
        engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
        for seq in 0..5u64 {
            let when = 100 + seq as i64 * 10;
            let side = if seq % 2 == 0 { Side::Buy } else { Side::Sell };
            engine.dispatch(
                pop(0, 0),
                &InboundMessage::Trade(trade(0, 100 * ONE, ONE, side, when)),
            );
            for message in pen.fill(side, 100 * ONE, ONE, when + 1) {
                engine.dispatch(pop(0, 0), &message);
            }
            engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(seq, when + 2)));
            engine.dispatch(
                pop(0, 0),
                &InboundMessage::Book(delta_chunk(
                    0,
                    Side::Buy,
                    &[(100 * ONE, (seq as i64 + 2) * ONE)],
                    when + 5,
                )),
            );
        }
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::MarketRotation(rotation(0, 300 * ONE, 600 * ONE, 200)),
        );
        (pop_books(&mut books), pop_events(&mut events))
    };
    let (first_books, first_events) = run();
    let (second_books, second_events) = run();
    assert_eq!(
        first_books, second_books,
        "book snapshot sequence is replay-deterministic"
    );
    assert_eq!(
        lane_shape(&first_events),
        lane_shape(&second_events),
        "the lane's kinds and sequence numbers are replay-deterministic"
    );
    assert_eq!(
        message_derived(&first_events),
        message_derived(&second_events),
        "event sequence is replay-deterministic across every kind"
    );
    // `Latency` is the one lane member that is MEASURED rather than derived: its queue-wait and
    // processing sums come off the machine's clock, so two runs report different numbers by
    // construction. The determinism this suite rests on is over hot state and the messages that
    // produce it, which is why the shape comparison above still binds where the payload one cannot.
    assert!(
        first_events
            .iter()
            .any(|event| matches!(event, UiEvent::Latency { .. })),
        "the spins teed self-timing, so the exclusion is over a kind that was genuinely present"
    );

    // Balance and Reject need messages this mix does not carry. Execution and OrderSnapshot require
    // a command ring; the wired per-spin test below pins both. Every kind this unwired mix CAN reach
    // must appear here, or the determinism claim is over a stream that never exercised it.
    let mut kinds = [false; 7];
    for event in &first_events {
        let slot = match event {
            UiEvent::Quote { .. } => 0,
            UiEvent::Trade { .. } => 1,
            UiEvent::OrderUpdate { .. } => 2,
            UiEvent::Feature { .. } => 3,
            UiEvent::Fill { .. } => 4,
            UiEvent::Rotation { .. } => 5,
            UiEvent::Position { .. } => 6,
            UiEvent::OrderSnapshot { .. } | UiEvent::Execution { .. } => continue,
            UiEvent::Balance { .. } | UiEvent::Reject { .. } => continue,
            UiEvent::Latency { .. } => continue,
        };
        kinds[slot] = true;
    }
    assert!(
        kinds.iter().all(|&seen| seen),
        "the determinism pin must cover every event kind the mix reaches, saw {kinds:?}"
    );
    assert!(
        !first_books.is_empty(),
        "the run actually emitted a book feed"
    );
}

/// A tiny ring saturates under an undrained burst — drops are counted, not silently lost — and the
/// next snapshot after a drain carries the latest state, so the feed self-heals.
#[test]
fn saturated_book_ring_drops_then_heals() {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    let (sink, _persist) = persist_ring(256);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (ui_book_sink, mut books) = ui_book_ring(4);
    let (ui_event_sink, _events) = ui_event_ring(64);
    let mut engine = HotEngine::new(HotEngineSetup {
        exec: None,
        exposure: detached_exposure(),
        instruments: &instruments,
        strategy: Box::new(Idle),
        persistence: Some(sink),
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
    });

    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(101 * ONE, ONE)], 10);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    for i in 0..20i64 {
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::Book(delta_chunk(
                0,
                Side::Buy,
                &[(100 * ONE, (i + 2) * ONE)],
                100 + i,
            )),
        );
    }
    assert!(
        engine.dropped_ui_books() > 0,
        "an undrained burst past the 4-slot ring drops and counts"
    );

    let _ = pop_books(&mut books);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Book(delta_chunk(0, Side::Buy, &[(100 * ONE, 99 * ONE)], 200)),
    );
    let snaps = pop_books(&mut books);
    assert_eq!(snaps.len(), 1, "after a drain the next snapshot lands");
    assert_eq!(
        snaps[0].bids[0],
        level(100 * ONE, 99 * ONE),
        "and it carries the latest book state, not a stale one"
    );
}

/// The ENGINE tees the desired quote, once per instrument per spin, carrying what the strategy
/// declared for THAT spin and the spin's own event time. The strategy has no UI call at all: it
/// declares into level-triggered state the engine owns, so the ladder can never show a quote the
/// engine is not actually trying to hold.
#[test]
fn desired_quote_tees_once_per_spin_with_the_spin_ts() {
    let bid = Some((Price(100 * ONE), Qty(ONE)));
    let ask = Some((Price(101 * ONE), Qty(2 * ONE)));
    let (mut engine, _books, mut events) =
        ui_engine(Box::new(QuoteProbe { bid, ask }), DurationUs::ZERO);

    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 12_345)));
    let quotes = quote_events(&mut events);
    assert_eq!(quotes.len(), 1, "one instrument tees one Quote per spin");
    let (instrument, event_ts_us, quote) = quotes[0];
    assert_eq!(instrument, InstrumentId(0));
    assert_eq!(
        event_ts_us,
        ts(12_345),
        "the event carries the spin's own event time, not a wall clock read"
    );
    assert_eq!(quote, DomQuote::top(bid, ask));
}

/// A declaration expires after ONE spin: the next spin, with the strategy declaring nothing, tees an
/// EMPTY quote rather than repeating the last one. That is what clears the ladder when a strategy
/// wedges mid-logic — a repeat would leave a level on screen the engine has already cancelled.
#[test]
fn an_expired_declaration_tees_an_empty_quote() {
    let bid = Some((Price(100 * ONE), Qty(ONE)));
    let (mut engine, _books, mut events) = ui_engine(
        Box::new(DeclareOnceProbe {
            bid,
            declared: false,
        }),
        DurationUs::ZERO,
    );

    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 100)));
    let quotes = quote_events(&mut events);
    assert_eq!(quotes[0].2, DomQuote::top(bid, None));

    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(1, 200)));
    let quotes = quote_events(&mut events);
    assert_eq!(
        quotes[0].2,
        DomQuote::default(),
        "a side not re-declared this spin reads as absent, not as the last thing it said"
    );
}

/// Declares once, on the first spin only, so the expiry above is driven by a strategy that genuinely
/// stopped talking rather than by one that never started.
struct DeclareOnceProbe {
    bid: Option<(Price, Qty)>,
    declared: bool,
}

impl Strategy for DeclareOnceProbe {
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        if self.declared {
            return;
        }
        self.declared = true;
        QuoteProbe::declare(ctx, Side::Buy, self.bid);
    }
}

/// Every Quote event on the lane as `(instrument, event time, quote)`, in lane order.
fn quote_events(consumer: &mut Consumer<UiEvent>) -> Vec<(InstrumentId, TsUs, DomQuote)> {
    pop_events(consumer)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Quote {
                instrument,
                event_ts_us,
                quote,
                ..
            } => Some((instrument, event_ts_us, quote)),
            _ => None,
        })
        .collect()
}

/// Every public print tees a Trade event ahead of the warmup gate, carrying the print faithfully, so
/// an operator watches the tape build before the strategy goes live.
#[test]
fn trade_events_emit_ahead_of_the_warmup_gate() {
    // Warmup 10s; the print lands at t=1s, so the strategy callback stays suppressed the whole test.
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::from_secs(10));
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Trade(trade(0, 100 * ONE, 3 * ONE, Side::Sell, 1_000_000)),
    );
    let evs = pop_events(&mut events);
    assert_eq!(evs.len(), 1, "a print tees exactly one Trade event");
    let UiEvent::Trade {
        instrument,
        seq,
        event_ts_us,
        aggressor,
        price,
        qty,
    } = evs[0]
    else {
        panic!("a print tees a Trade event, got {:?}", evs[0]);
    };
    assert_eq!(instrument, InstrumentId(0));
    assert_eq!(seq, 0, "the first event is sequence 0");
    assert_eq!(event_ts_us, ts(1_000_000), "stamped with the ingress time");
    assert_eq!(aggressor, Side::Sell);
    assert_eq!(price, Price(100 * ONE));
    assert_eq!(qty, Qty(3 * ONE));
}

/// Each banked `FeatureRow` tees to one Feature event, in emit order, mirroring the value that lands
/// in Parquet — the same drain that fills the persist lane.
#[test]
fn feature_rows_tee_in_order_with_their_values() {
    let (mut engine, _books, mut events) =
        ui_engine(Box::new(FeatureProbe { ids: Vec::new() }), DurationUs::ZERO);
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 500)));

    let features: Vec<_> = pop_events(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Feature {
                seq,
                event_ts_us,
                feature,
                value,
                ..
            } => Some((seq, event_ts_us, feature, value)),
            _ => None,
        })
        .collect();
    assert_eq!(features.len(), 3, "three emits tee three Feature events");
    let expected = [
        (FeatureId(0), 1.5),
        (FeatureId(1), 2.5),
        (FeatureId(2), 3.5),
    ];
    for (index, ((seq, event_ts_us, feature, value), (want_id, want_value))) in
        features.iter().zip(expected).enumerate()
    {
        assert_eq!(
            *seq, index as u64,
            "tees carry the lane-wide sequence in order"
        );
        assert_eq!(*event_ts_us, ts(500), "stamped with the spin's event time");
        assert_eq!(*feature, want_id, "features tee in emit order");
        assert_eq!(*value, want_value, "the teed value equals the banked one");
    }
}

/// Recording and displaying are ORTHOGONAL operator intents, so a feature tees to the monitor even
/// when it is not recorded: with no `persistence:` block at all, and with one whose `tables` names
/// other tables. The tee is the monitor's only source of feature values, so a regression here blanks
/// the panel with no error anywhere — and it would silently defeat watching a linked TE's feature
/// arrive. Both arms also pin that nothing reaches Parquet, which is the half the config does gate.
#[test]
fn features_tee_to_the_ui_even_when_they_are_not_recorded() {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];

    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (ui_book_sink, _books) = ui_book_ring(64);
    let (ui_event_sink, mut events) = ui_event_ring(64);
    let mut engine = HotEngine::new(HotEngineSetup {
        exec: None,
        exposure: detached_exposure(),
        instruments: &instruments,
        strategy: Box::new(FeatureProbe { ids: Vec::new() }),
        persistence: None,
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
    });
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 500)));
    assert_eq!(
        teed_features(&mut events),
        vec![
            (0, FeatureId(0), 1.5),
            (1, FeatureId(1), 2.5),
            (2, FeatureId(2), 3.5)
        ],
        "with no persistence at all, every emit still tees in order with its value"
    );

    // Persistence ON but `features` unnamed: the ring exists and must stay empty, while the tee runs.
    let (persistence, mut records) =
        persist_ring_for(256, RecordedTables::new(&[TableKind::Trades]));
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (mut engine, _books, mut events) = engine_with_ui(
        &instruments,
        Box::new(FeatureProbe { ids: Vec::new() }),
        persistence,
        log_sink,
        metrics,
        DurationUs::ZERO,
    );
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 500)));
    assert_eq!(
        teed_features(&mut events).len(),
        3,
        "a table set that omits features still tees all three"
    );
    assert!(
        records.pop().is_err(),
        "not one unrecorded feature reaches the persist ring"
    );
    assert_eq!(
        engine.dropped_persist_records(),
        0,
        "a tee-only row never displaces a row bound for Parquet, so it counts as no drop"
    );
}

fn teed_features(events: &mut Consumer<UiEvent>) -> Vec<(u64, FeatureId, f64)> {
    pop_events(events)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Feature {
                seq,
                feature,
                value,
                ..
            } => Some((seq, feature, value)),
            _ => None,
        })
        .collect()
}

/// A WIRED engine tees the kill switch and both complete OMS sides every spin, whether or not
/// anything about them changed. Absolute state, for the reason a panel exists: a halt an operator
/// learns about from a log is a halt they learn about late, while an order terminal transition lost
/// to a full ring would otherwise leave the band reading `OPEN` forever.
#[test]
fn a_wired_engine_tees_the_execution_gate_every_spin() {
    let (mut engine, mut events, _commands) = ui_engine_with_exec(Box::new(Idle));
    for seq in 0..3u64 {
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::SpinTick(spin(seq, 700 + seq as i64)),
        );
    }

    let emitted = pop_events(&mut events);
    let gates: Vec<ExecHalt> = emitted
        .iter()
        .filter_map(|event| match event {
            UiEvent::Execution { halt, .. } => Some(*halt),
            _ => None,
        })
        .collect();
    assert_eq!(
        gates,
        vec![ExecHalt::Armed; 3],
        "one Execution frame per spin, carrying the latch verbatim"
    );

    let snapshots: Vec<&UiEvent> = emitted
        .iter()
        .filter(|event| matches!(event, UiEvent::OrderSnapshot { .. }))
        .collect();
    assert_eq!(
        snapshots.len(),
        6,
        "each spin atomically re-states both OMS sides"
    );
    for (spin_index, pair) in snapshots.chunks_exact(2).enumerate() {
        for (side_index, event) in pair.iter().enumerate() {
            assert!(
                matches!(
                    event,
                    UiEvent::OrderSnapshot {
                        instrument: InstrumentId(0),
                        event_ts_us,
                        side,
                        detail_len: 0,
                        total_working: 0,
                        ..
                    } if *event_ts_us == ts(700 + spin_index as i64)
                        && *side == [Side::Buy, Side::Sell][side_index]
                ),
                "an empty complete side cut must still be emitted: {event:?}"
            );
        }
    }
}

/// FITNESS: an engine with no command ring says NOTHING about the gate. The absence is the whole
/// assertion, and no other test can reach it — every one of them checks what the lane carries.
///
/// [`ExecHalt`] is the kill-switch LATCH, not the wired state, so `Armed` on a run that has no ring
/// means only "nothing has tripped". The band renders it under the heading `gate` as `ARMED` in the
/// positive colour, which an operator reads as "armed to trade". The run where that costs money is
/// not the recorder: it is a `mode: live` config whose preflight or queue is missing, which WARNs
/// once and disarms execution for the rest of the run (`runtime::exec`). Nothing can ever be sent,
/// and a green ARMED is the only thing on screen.
///
/// That is the same class as the simulated-fill labels this milestone deleted — a display asserting
/// something false about real money — and worse, because those were true when they were written.
#[test]
fn an_unwired_engine_never_claims_to_be_armed() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    for seq in 0..3u64 {
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::SpinTick(spin(seq, 700 + seq as i64)),
        );
    }

    assert_eq!(
        execution_frames(&mut events),
        Vec::new(),
        "no command ring means no order can ever be sent — the gate must say nothing, not ARMED"
    );
}

/// FITNESS: the engine tees its own timing once per spin, on the ONE lane, and the desktop model
/// keeps the latest.
///
/// It rides the event lane rather than a channel of its own so that a UI which has fallen behind
/// sees the drop in the same sequence it already tracks. That only holds if the kind takes its turn
/// like every other: a Latency frame that numbered itself separately would leave the consumer's gap
/// count reporting losses that never happened, on every other panel at once.
#[test]
fn spins_tee_engine_latency_on_the_one_lane() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    for seq in 0..3u64 {
        engine.dispatch(
            spin_pop(0, 7),
            &InboundMessage::SpinTick(spin(seq, 700 + seq as i64)),
        );
    }

    let evs = pop_events(&mut events);
    let summaries: Vec<UiLatencySummary> = evs
        .iter()
        .filter_map(|event| match event {
            UiEvent::Latency { summary, .. } => Some(*summary),
            _ => None,
        })
        .collect();
    assert_eq!(summaries.len(), 3, "one self-timing frame per spin");
    for (index, event) in evs.iter().enumerate() {
        assert_eq!(
            event.seq(),
            index as u64,
            "self-timing takes its turn in the lane-wide sequence: {evs:?}"
        );
    }

    let latest = summaries[2];
    assert_eq!(
        latest.backlog_ema,
        Some(7.0),
        "a constant backlog is the fold's fixed point, whatever the gaps between spins"
    );
    assert_eq!(
        latest.hot_path.processing.count, 3,
        "a spin's own processing is measured before it reports"
    );

    #[cfg(feature = "ui")]
    {
        let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
        assert_eq!(
            model.latency(),
            None,
            "a model that has seen no spin reports no timing rather than an empty one"
        );
        for event in &evs {
            model.apply_event(*event);
        }
        assert_eq!(model.latency(), Some(latest), "the newest spin wins");
        assert_eq!(model.event_gaps(), 0);

        let skipped = UiEvent::Latency {
            seq: evs.len() as u64 + 2,
            event_ts_us: ts(999),
            summary: UiLatencySummary::default(),
        };
        model.apply_event(skipped);
        assert_eq!(
            model.event_gaps(),
            2,
            "a Latency frame is gap-counted on the one lane like every other kind"
        );
        assert_eq!(
            model.latency(),
            Some(UiLatencySummary::default()),
            "and the latest still wins across the gap"
        );
    }
}

/// One venue fill tees its resulting absolute OrderUpdate and exactly one Fill event carrying THAT
/// fill's own side, price, qty and event time, taking their place on the one lane behind the public
/// print that preceded it.
///
/// The price and quantity are the executed ones rather than the resting order's — the pen's slot
/// rests at a size far larger than it fills, so a tee reading the order instead of the execution
/// shows a number no trade ever happened at.
#[test]
fn a_partial_fill_updates_the_working_order_then_tees_one_fill() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Trade(trade(0, 100 * ONE, ONE, Side::Sell, 4_242)),
    );
    let mut pen = FillPen::new(0);
    for message in pen.fill(Side::Buy, 100 * ONE, ONE, 4_242) {
        engine.dispatch(pop(0, 0), &message);
    }

    let evs = pop_events(&mut events);
    // The pen seats the order before reporting against it, so the adoption tees an OrderUpdate
    // between the print and fill. The fill then tees the absolute post-fill order state ahead of its
    // tape delta: one venue event carries both facts, while the strategy still receives one callback.
    assert_eq!(
        evs.len(),
        4,
        "Trade, adoption, post-fill order state, and exactly one fill tape event"
    );
    assert!(
        matches!(evs[0], UiEvent::Trade { .. }),
        "the print's Trade leads the fill on the one lane"
    );
    assert!(
        matches!(
            evs[1],
            UiEvent::OrderUpdate {
                state: OrderState::Live,
                ..
            }
        ),
        "the adoption tees the order as confirmed-live, got {:?}",
        evs[1]
    );
    assert!(
        matches!(
            evs[2],
            UiEvent::OrderUpdate {
                state: OrderState::Live,
                filled: Qty(ONE),
                ..
            }
        ),
        "a partial fill must leave one live order with its cumulative fill projected, got {:?}",
        evs[2]
    );
    let UiEvent::Fill {
        instrument,
        seq,
        commission,
        commission_asset,
        liquidity,
        event_ts_us,
        quote_level,
        side,
        price,
        qty,
    } = evs[3]
    else {
        panic!(
            "a venue fill tees one Fill event after its order state, got {:?}",
            evs[3]
        );
    };
    assert_eq!(instrument, InstrumentId(0));
    assert_eq!(seq, 3, "the Fill follows its absolute order state");
    assert_eq!(event_ts_us, ts(4_242), "stamped with the fill's event time");
    assert_eq!(quote_level, Some(QuoteLevel::ZERO));
    assert_eq!(side, Side::Buy, "a fill on our bid means we bought");
    assert_eq!(price, Price(100 * ONE));
    assert_eq!(qty, Qty(ONE), "the executed size, not the resting order's");
    // The pen's report is a maker fill charged nothing in an asset the registry does not name. Each
    // of the three is passed through rather than re-derived: a tee that substituted the instrument's
    // quote asset for `UNKNOWN` would book the fee against money it was never charged in, and one
    // that dropped the maker flag would hide the fee tier the whole first live run exists to check.
    assert_eq!(commission, 0);
    assert_eq!(commission_asset, AssetId::UNKNOWN);
    assert_eq!(liquidity, Some(Liquidity::Maker));

    #[cfg(feature = "ui")]
    {
        let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
        for event in evs {
            model.apply_event(event);
        }
        let working = model.exec().working(InstrumentId(0), Side::Buy);
        assert_eq!(
            working.len(),
            1,
            "a partial fill updates the existing BID; it must not retire it or create a duplicate"
        );
        assert_eq!(working[0].filled, Qty(ONE));
    }
}

/// FITNESS: a full fill retires the UI's working order on the fill's own message sequence.
///
/// This is the production failure that made the workstation report OPEN 2 BID: the execution core
/// closed the first bid, but the feed emitted only a Fill tape event. The model therefore retained
/// the old live cell and counted the next genuine bid beside it. Driving the hot engine and then the
/// desktop fold together pins the whole seam rather than merely checking that some event was sent.
#[test]
fn a_full_fill_retires_the_working_order_in_the_ui_model() {
    let fill_callbacks = Arc::new(AtomicUsize::new(0));
    let update_callbacks = Arc::new(AtomicUsize::new(0));
    let strategy = FillCallbackProbe {
        fills: Arc::clone(&fill_callbacks),
        updates: Arc::clone(&update_callbacks),
    };
    let (mut engine, _books, mut events) = ui_engine(Box::new(strategy), DurationUs::ZERO);
    let mut pen = FillPen::new(0);
    let InboundMessage::Exec(mut adoption) = pen
        .adopt(Side::Buy, 100 * ONE, 900)
        .expect("a fresh pen has seated nothing")
    else {
        unreachable!("FillPen adoption is an execution event");
    };
    adoption.qty = Qty(2 * ONE);
    let client_id = adoption.client_id;
    engine.dispatch(pop(0, 0), &InboundMessage::Exec(adoption));

    let full_qty = Qty(2 * ONE);
    let full_fill = InboundMessage::Exec(polysim::msg::exec::ExecEvent {
        kind: ExecKind::ReportTrade,
        status: Some(VenueOrderStatus::Filled),
        last_price: Price(100 * ONE),
        last_qty: full_qty,
        cumulative_qty: full_qty,
        cumulative_quote: Price(100 * ONE).notional(full_qty),
        qty: full_qty,
        ..exec_event(InstrumentId(0), client_id, Side::Buy, 100 * ONE, 901)
    });
    engine.dispatch(pop(0, 0), &full_fill);

    let evs = pop_events(&mut events);
    assert_eq!(
        evs.len(),
        3,
        "adoption plus a terminal OrderUpdate and one Fill tape event"
    );
    assert!(
        matches!(
            evs[1],
            UiEvent::OrderUpdate {
                client_id: id,
                state: OrderState::Closed(CloseReason::Filled),
                filled,
                ..
            } if id == client_id && filled == full_qty
        ),
        "the fill must project the engine's terminal state before its tape event: {:?}",
        evs[1]
    );
    assert!(
        matches!(evs[2], UiEvent::Fill { qty, .. } if qty == full_qty),
        "the same venue event still emits exactly one fill tape record"
    );
    assert_eq!(
        fill_callbacks.load(Ordering::Relaxed),
        1,
        "the venue fill still earns exactly one strategy fill callback"
    );
    assert_eq!(
        update_callbacks.load(Ordering::Relaxed),
        1,
        "only the adoption earns a strategy order-update callback; the fill's second projection is UI-only"
    );

    #[cfg(feature = "ui")]
    {
        let mut model = UiModel::with_capacity(1, DurationUs::from_micros(100_000));
        for event in evs {
            model.apply_event(event);
        }
        assert!(
            model.exec().working(InstrumentId(0), Side::Buy).is_empty(),
            "a fully-filled bid must be absent before any replacement bid can be counted"
        );
    }
}

/// FITNESS: a venue event that moves one of OUR orders reaches the UI carrying the ENGINE's state
/// for it, faithfully, on the event's own message.
///
/// This replaces `order_intents_tee_with_the_faithful_action`, which pinned the same guarantee on
/// the path that no longer exists — a strategy banked an `OrderAction` and the drain teed it. The
/// feeder is now the venue: `on_exec` folds the event, `ExecCallback::Update` names the transition,
/// and `emit_order_update` tees it. The guarantee survived the rewrite even though nothing it was
/// written against did, which is why it earns a test rather than being dropped with its subject.
///
/// `state` is the load-bearing field. It is the order table's own answer, not the wire's, so the DOM
/// can tell an order the venue has CONFIRMED from one whose command is still outstanding. A tee that
/// echoed the wire status would collapse that distinction and paint `live` over a command in flight.
#[test]
fn a_venue_transition_tees_the_engines_own_order_state() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    let mut pen = FillPen::new(0);
    let adoption = pen
        .adopt(Side::Buy, 100 * ONE, 900)
        .expect("a fresh pen has seated nothing");
    engine.dispatch(pop(0, 0), &adoption);

    let evs = pop_events(&mut events);
    assert_eq!(evs.len(), 1, "one venue event tees exactly one UI event");
    let UiEvent::OrderUpdate {
        instrument,
        seq,
        event_ts_us,
        side,
        state,
        price,
        qty,
        filled,
        ..
    } = evs[0]
    else {
        panic!("an adoption tees an OrderUpdate, got {:?}", evs[0]);
    };
    assert_eq!(instrument, InstrumentId(0));
    assert_eq!(seq, 0, "the first event on the lane");
    assert_eq!(event_ts_us, ts(900), "stamped with the venue event's time");
    assert_eq!(side, Side::Buy);
    assert_eq!(
        state,
        OrderState::Live,
        "an adopted order is venue-confirmed, so the DOM may paint it as resting size"
    );
    assert_eq!(price, Price(100 * ONE));
    assert_eq!(qty, Qty(i64::MAX / 4), "the pen seats an oversized slot");
    assert_eq!(filled, Qty(0), "nothing has executed against it yet");
}

/// FITNESS: a venue that said NOTHING about liquidity arrives as `None`, never as `Some(Maker)`.
///
/// The sibling above pins present-stays-present; this pins absent-stays-absent, and they are
/// different properties. Only this one guards against FABRICATION — a tee that defaulted to `Maker`
/// because a post-only order usually is one would pass the sibling and invent the answer here.
/// "Usually" is exactly the reasoning that makes it wrong: the case that matters is the one where
/// the assumption fails, which is a post-only order that did not rest.
///
/// It costs nothing today — this account is 10 bps both ways, so maker and taker price the same —
/// and that is what makes it easy to wave through. The `fills` table exists to be reconciled against
/// a Binance statement, and the moment a BNB discount or a VIP tier splits the two rates, fabricated
/// rows become indistinguishable from honest ones in the column an accountant checks. Same rule as
/// `Option<OrderStyle>`, as `lost` counting apart from `in flight`, and as an unreported balance
/// rendering `—` rather than zero: an invented value and an absent one must stay distinguishable.
#[test]
fn a_silent_venue_leaves_liquidity_absent() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    let mut pen = FillPen::new(0);
    let adoption = pen
        .adopt(Side::Buy, 100 * ONE, 4_242)
        .expect("a fresh pen has seated nothing");
    engine.dispatch(pop(0, 0), &adoption);
    engine.dispatch(
        pop(0, 0),
        &pen.silent_report(Side::Buy, 100 * ONE, ONE, 4_242),
    );

    let fills: Vec<Option<Liquidity>> = pop_events(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Fill { liquidity, .. } => Some(liquidity),
            _ => None,
        })
        .collect();
    assert_eq!(
        fills,
        vec![None],
        "absent must stay absent — it is not the same claim as maker"
    );
}

/// A window rotation tees a Rotation event (ahead of the warmup gate, like its book refresh).
#[test]
fn rotation_tees_an_event() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::from_secs(10));
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, 300 * ONE, 600 * ONE, 30)),
    );
    let evs = pop_events(&mut events);
    assert_eq!(evs.len(), 1, "a rotation tees exactly one Rotation event");
    let UiEvent::Rotation {
        instrument,
        seq,
        event_ts_us,
    } = evs[0]
    else {
        panic!("a rotation tees a Rotation event, got {:?}", evs[0]);
    };
    assert_eq!(instrument, InstrumentId(0));
    assert_eq!(seq, 0);
    assert_eq!(
        event_ts_us,
        ts(30),
        "stamped with the rotation's ingress time"
    );
}

/// FITNESS: every spin re-states each marked instrument's exposure and PnL as ABSOLUTE quote units,
/// which is what makes a dropped Position frame equivalent to one never sent. Deltas would
/// silently skew a consumer forever after one loss, and nothing would report it. An instrument with
/// no honest valuation says nothing at all rather than a zero it cannot stand behind: none before
/// its first two-sided book, and none again after a rotation clears its mark — while its neighbour
/// keeps reporting.
#[test]
fn spins_emit_absolute_position_state() {
    let instruments = [
        instrument_row(0, tracker_spec_all(1), 64),
        instrument_row(1, tracker_spec_all(1), 64),
    ];
    let (sink, _persist) = persist_ring(256);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (mut engine, _books, mut events) = engine_with_ui(
        &instruments,
        Box::new(Idle),
        sink,
        log_sink,
        metrics,
        DurationUs::ZERO,
    );

    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 10)));
    assert!(
        positions(&mut events).is_empty(),
        "with no book yet there is no mark, so the engine reports no position at all"
    );

    // Mid of 100/102 marks instrument 0 at 101.
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(102 * ONE, ONE)], 20);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(1, 30)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(0), ts(30), 0.0, 0.0)],
        "a marked but flat instrument reports zeroes stamped with the spin's event time; the \
         unmarked one is still silent"
    );

    // Mid of 200/204 marks instrument 1 at 202, and it stays flat for the rest of the run.
    let (bids, asks) = snapshot_pair(1, &[(200 * ONE, ONE)], &[(204 * ONE, ONE)], 40);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    let mut pen = FillPen::new(0);
    for message in pen.fill(Side::Buy, 100 * ONE, 3 * ONE, 50) {
        engine.dispatch(pop(0, 0), &message);
    }
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(2, 60)));
    // Bought 3 @ 100 and marked at 101: exposure 303, cash -300, so PnL is the 3 the mark moved.
    assert_eq!(
        positions(&mut events),
        vec![
            (InstrumentId(0), ts(60), 303.0, 3.0),
            (InstrumentId(1), ts(60), 0.0, 0.0),
        ],
        "the fill that arrived since the last spin is folded into what this one reports, and every \
         marked instrument reports"
    );

    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, 300 * ONE, 600 * ONE, 70)),
    );
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(3, 80)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(1), ts(80), 0.0, 0.0)],
        "a rotated instrument goes silent until a new two-sided book re-marks it — the old \
         window's mark would be a lie about the new one"
    );
}

/// A fill that landed since the last spin is folded before this one reports, so an operator never
/// reads a position one spin behind the fills they can already see on the tape. The emission sits at
/// the very end of the spin's own dispatch — after the strategy, its drain and the exec pass — which
/// is what makes that true regardless of how close to the spin the fill arrived.
#[test]
fn a_fill_is_folded_before_the_next_spin_reports() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(102 * ONE, ONE)], 10);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));

    let mut pen = FillPen::new(0);
    for message in pen.fill(Side::Buy, 100 * ONE, 3 * ONE, 15) {
        engine.dispatch(pop(0, 0), &message);
    }
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 20)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(0), ts(20), 303.0, 3.0)],
        "the first spin reports the clip that filled before it, not the flat book it started from"
    );

    for message in pen.fill(Side::Buy, 100 * ONE, 3 * ONE, 25) {
        engine.dispatch(pop(0, 0), &message);
    }
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(1, 30)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(0), ts(30), 606.0, 6.0)],
        "and each later spin includes the clip that filled since the one before it"
    );
}

/// Position state rides ahead of the live gate, like the book and trade tees: marks are set during
/// warmup, and re-stating absolute state to a UI that attached mid-run costs nothing to be right.
#[test]
fn positions_emit_during_warmup() {
    // Warmup 10s; every message lands at t=1s, so the strategy stays suppressed the whole test.
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::from_secs(10));
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(102 * ONE, ONE)], 1_000_000);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 1_000_100)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(0), ts(1_000_100), 0.0, 0.0)],
        "a warmup-suppressed spin still re-states the instrument's mark-to-market"
    );
}

/// The other half of the live gate. Parking stops the strategy, not the arithmetic: the engine is
/// still holding whatever the run left it holding, and a UI that attaches to a parked engine must be
/// told what that is rather than nothing.
#[test]
fn positions_emit_while_parked() {
    let (mut engine, _books, mut events) = ui_engine(Box::new(Idle), DurationUs::ZERO);
    let (bids, asks) = snapshot_pair(0, &[(100 * ONE, ONE)], &[(102 * ONE, ONE)], 10);
    engine.dispatch(pop(0, 0), &InboundMessage::Book(bids));
    engine.dispatch(pop(0, 0), &InboundMessage::Book(asks));
    let mut pen = FillPen::new(0);
    for message in pen.fill(Side::Buy, 100 * ONE, 3 * ONE, 20) {
        engine.dispatch(pop(0, 0), &message);
    }
    let _ = pop_events(&mut events);

    engine.dispatch(pop(0, 0), &run_control(idle_at(1), 30));
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 40)));
    assert_eq!(
        positions(&mut events),
        vec![(InstrumentId(0), ts(40), 303.0, 3.0)],
        "a parked engine keeps re-stating the position it is still carrying, not a zero"
    );
}

/// FITNESS: the one lane-wide sequence is dense and monotonic ACROSS kinds — 0,1,2,… with no gaps
/// and no per-kind counters — over a mixed feed of a rotation, a trade, an order transition, its
/// fill and a spin's feature and quote.
///
/// It asserts the sequence and the kinds PRESENT, never a total. A count would make this fail the
/// next time the engine gains an event kind or a transition gains a step — and it would fail here,
/// in the monotonicity test, for a reason that has nothing to do with monotonicity. That already
/// happened twice in one afternoon: once when a venue fill became two messages rather than one, and
/// once when the execution frame correctly stopped being emitted on an unwired engine. A test that
/// breaks whenever a kind is added is testing cardinality by accident.
#[test]
fn mixed_kind_lane_sequence_is_globally_monotonic() {
    let (mut engine, _books, mut events) =
        ui_engine(Box::new(MixedProbe { feature: None }), DurationUs::ZERO);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, 300 * ONE, 600 * ONE, 10)),
    );
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Trade(trade(0, 100 * ONE, ONE, Side::Buy, 20)),
    );
    let mut pen = FillPen::new(0);
    for message in pen.fill(Side::Buy, 100 * ONE, ONE, 25) {
        engine.dispatch(pop(0, 0), &message);
    }
    engine.dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(0, 30)));

    let evs = pop_events(&mut events);
    for (index, event) in evs.iter().enumerate() {
        assert_eq!(
            event.seq(),
            index as u64,
            "the lane-wide sequence is dense and monotonic across kinds: {evs:?}"
        );
    }

    let kinds: Vec<&str> = evs.iter().map(event_kind).collect();
    for expected in [
        "Rotation",
        "Trade",
        "OrderUpdate",
        "Fill",
        "Feature",
        "Quote",
    ] {
        assert!(
            kinds.contains(&expected),
            "{expected} never reached the one lane: {kinds:?}"
        );
    }
}

/// The lane reduced to what each run must reproduce exactly: which kind landed where, under which
/// sequence number.
fn lane_shape(events: &[UiEvent]) -> Vec<(&'static str, u64)> {
    events
        .iter()
        .map(|event| (event_kind(event), event.seq()))
        .collect()
}

fn message_derived(events: &[UiEvent]) -> Vec<UiEvent> {
    events
        .iter()
        .copied()
        .filter(|event| !matches!(event, UiEvent::Latency { .. }))
        .collect()
}

/// The variant's name alone — enough to assert a kind reached the lane without pinning how many of
/// it did, or what else rode beside it.
fn event_kind(event: &UiEvent) -> &'static str {
    match event {
        UiEvent::Quote { .. } => "Quote",
        UiEvent::Trade { .. } => "Trade",
        UiEvent::OrderUpdate { .. } => "OrderUpdate",
        UiEvent::OrderSnapshot { .. } => "OrderSnapshot",
        UiEvent::Feature { .. } => "Feature",
        UiEvent::Fill { .. } => "Fill",
        UiEvent::Balance { .. } => "Balance",
        UiEvent::Reject { .. } => "Reject",
        UiEvent::Execution { .. } => "Execution",
        UiEvent::Rotation { .. } => "Rotation",
        UiEvent::Position { .. } => "Position",
        UiEvent::Latency { .. } => "Latency",
    }
}

/// A tiny event ring saturating under an undrained trade burst drops and counts, and the sequence
/// keeps climbing through the drops so the next event after a drain reads as a gap and self-heals.
#[test]
fn saturated_event_ring_drops_then_heals() {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    let (sink, _persist) = persist_ring(256);
    let (log_sink, _logs) = strategy_log_ring(64);
    let (metrics, _metrics) = metrics_ring(64);
    let (ui_book_sink, _books) = ui_book_ring(64);
    let (ui_event_sink, mut events) = ui_event_ring(4);
    let mut engine = HotEngine::new(HotEngineSetup {
        exec: None,
        exposure: detached_exposure(),
        instruments: &instruments,
        strategy: Box::new(Idle),
        persistence: Some(sink),
        strategy_log_sink: log_sink,
        metrics_sink: metrics,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
    });

    for i in 0..20i64 {
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::Trade(trade(0, 100 * ONE, ONE, Side::Buy, i)),
        );
    }
    assert!(
        engine.dropped_ui_events() > 0,
        "an undrained burst past the 4-slot ring drops and counts events"
    );

    // Drain, then one more print: its Trade event lands, and its seq reflects every drop before it.
    let _ = pop_events(&mut events);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::Trade(trade(0, 100 * ONE, ONE, Side::Buy, 99)),
    );
    let evs = pop_events(&mut events);
    assert_eq!(evs.len(), 1, "after a drain the next event lands");
    assert_eq!(
        evs[0].seq(),
        20,
        "the sequence counted through the drops — the consumer reads the gap"
    );
}
