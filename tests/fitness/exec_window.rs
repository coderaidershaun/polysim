//! FITNESS: on a venue whose markets EXPIRE, an order must never be resting when one does.
//!
//! Every other gate in the engine protects against a price or a size. This one protects against a
//! clock, and it is the only gate whose failure cannot be traded out of afterwards: a fill taken in
//! the last moment of a five-minute market leaves a position on contracts that settle at nothing or
//! at everything, with no book left to place an order into. The strategy is expected to withdraw
//! first; this is what holds when it does not.
//!
//! Both halves are pinned, and the second is why the first is not enough. Refusing new declarations
//! stops the ladder GROWING, and the reconciler then withdraws what is already there — one order per
//! side per spin, so a full ladder comes down over eight of them. The sweep takes the whole thing in
//! one pass.
//!
//! The gate is driven entirely by message stamps: the window comes from a `MarketRotation` and the
//! instant from the spin tick, so a replay refuses and sweeps at exactly the same points.

use polysim::config::RecordedTables;
use polysim::hot::dispatch::HotEngine;
use polysim::hot::exec::{
    DesiredQuote, ExecSettings, OrderSlot, OrderState, QuoteLevel, RejectOrigin, RejectReason,
};
use polysim::hot::strategy::{Strategy, StrategyCtx, WindowInfo};
use polysim::ids::{Price, Side};
use polysim::msg::exec::{
    ExecCommand, ExecEvent, ExecKind, ExecLaneItem, OrderStyle, VenueOrderStatus,
};
use polysim::msg::inbound::{InboundMessage, SpinTick};
use polysim::msg::ui::UiEvent;
use polysim::time::{DurationUs, TsUs};
use rtrb::Consumer;

use crate::engine_support::{ALL_TABLES, ONE, exec_event, pop, rotation};
use crate::risk_gate::{
    ASK, BID, CEILING, INSTRUMENT, QUOTE_QTY, QuotingSetup, TwoSidedQuoter, built_quoting_engine,
    drain_commands, exec_settings, make_ready, reseat_book, row_with_ceiling, spin_at,
};

/// A window is 300 seconds and quoting stops 3 seconds before it closes, which is the shipped
/// polymarket shape.
const WINDOW: DurationUs = DurationUs::from_secs(300);
const MARGIN: DurationUs = DurationUs::from_secs(3);
const OPEN: i64 = 1_000_000_000;
const CLOSE: i64 = OPEN + 300 * 1_000_000;

fn windowed_engine() -> (HotEngine, Consumer<ExecLaneItem>, Consumer<UiEvent>) {
    let built = built_quoting_engine(QuotingSetup {
        row: row_with_ceiling(CEILING),
        strategy: Box::new(TwoSidedQuoter),
        restored: &[],
        settings: ExecSettings {
            quote_stop_margin: MARGIN,
            ..exec_settings()
        },
        tables: RecordedTables::new(&ALL_TABLES),
        run_nonce: 0,
    });
    (built.engine, built.commands, built.ui_events)
}

/// Ready, quotable, and told which window it is in — everything except the instant, which each case
/// then chooses.
fn armed_at(when: i64) -> (HotEngine, Consumer<ExecLaneItem>, Consumer<UiEvent>) {
    let (mut engine, mut commands, ui_events) = windowed_engine();
    make_ready(&mut engine, when);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, OPEN, CLOSE, when)),
    );
    for message in reseat_book(BID, ASK, when) {
        engine.dispatch(pop(0, 0), &message);
    }
    drain_commands(&mut commands);
    (engine, commands, ui_events)
}

fn places(commands: &[ExecCommand]) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, ExecCommand::Place { .. }))
        .count()
}

fn cancels(commands: &[ExecCommand]) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, ExecCommand::Cancel { .. }))
        .count()
}

fn refusals(ui_events: &mut Consumer<UiEvent>) -> Vec<RejectReason> {
    let mut reasons = Vec::new();
    while let Ok(event) = ui_events.pop() {
        if let UiEvent::Reject {
            origin: RejectOrigin::Local(reason),
            ..
        } = event
        {
            reasons.push(reason);
        }
    }
    reasons
}

#[test]
fn the_quote_window_opens_at_the_open_and_shuts_a_margin_before_the_close() {
    let window = WindowInfo {
        open_ts_us: TsUs::from_micros(OPEN),
        close_ts_us: TsUs::from_micros(CLOSE),
    };
    let at = |micros: i64| TsUs::from_micros(micros);

    assert!(
        !window.admits_quote_at(at(OPEN - 1), MARGIN),
        "one microsecond before the open the market is not there to quote into"
    );
    assert!(
        window.admits_quote_at(at(OPEN), MARGIN),
        "the open is inside the window it opens"
    );
    assert!(
        window.admits_quote_at(at(CLOSE - MARGIN.micros() - 1), MARGIN),
        "the last instant before the stop still quotes"
    );
    assert!(
        !window.admits_quote_at(at(CLOSE - MARGIN.micros()), MARGIN),
        "the stop itself is out — the margin is what the ladder needs to come down in"
    );
    assert!(
        window.is_past_quote_stop(at(CLOSE - MARGIN.micros()), MARGIN),
        "and the same instant is what arms the sweep"
    );
    assert!(
        !window.is_past_quote_stop(at(CLOSE - MARGIN.micros() - 1), MARGIN),
        "one microsecond earlier it does not"
    );
    assert_eq!(
        WINDOW,
        window.close_ts_us.diff(window.open_ts_us),
        "the fixture really is a whole window, not a sliver either boundary sits outside"
    );
}

#[test]
fn a_quote_declared_before_the_market_opens_is_refused_by_name() {
    let (mut engine, mut commands, mut ui_events) = armed_at(OPEN - 60 * 1_000_000);
    spin_at(&mut engine, 1, OPEN - 30 * 1_000_000);
    let early = drain_commands(&mut commands);
    assert_eq!(
        places(&early),
        0,
        "the market has not opened and the strategy is declaring into nothing: {early:?}"
    );
    assert!(
        refusals(&mut ui_events).contains(&RejectReason::OutsideWindow),
        "a refusal with no reason on the panel is how a gate that could never arm survives a run"
    );

    let (mut engine, mut commands, _ui_events) = armed_at(OPEN + 10 * 1_000_000);
    spin_at(&mut engine, 1, OPEN + 20 * 1_000_000);
    let inside = drain_commands(&mut commands);
    assert!(
        places(&inside) > 0,
        "the identical declaration inside the window has to reach the venue, or the case above is \
         reading a fixture that never asked for an order: {inside:?}"
    );
}

struct LadderQuoter;

const LADDER_DEPTH: usize = 3;

impl Strategy for LadderQuoter {
    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
        for level in 0..LADDER_DEPTH {
            ctx.quote(
                INSTRUMENT,
                Side::Buy,
                QuoteLevel::new(level as u8).expect("within the fixed ladder"),
                Some(DesiredQuote {
                    price: Price(BID - level as i64 * ONE),
                    qty: QUOTE_QTY,
                    style: OrderStyle::PostOnly,
                }),
            );
        }
    }
}

fn laddered_engine() -> (HotEngine, Consumer<ExecLaneItem>, Consumer<UiEvent>) {
    let built = built_quoting_engine(QuotingSetup {
        row: row_with_ceiling(CEILING),
        strategy: Box::new(LadderQuoter),
        restored: &[],
        settings: ExecSettings {
            quote_stop_margin: MARGIN,
            max_orders_per_side: LADDER_DEPTH,
            ..exec_settings()
        },
        tables: RecordedTables::new(&ALL_TABLES),
        run_nonce: 0,
    });
    (built.engine, built.commands, built.ui_events)
}

fn seat_ladder(
    engine: &mut HotEngine,
    commands: &mut Consumer<ExecLaneItem>,
    first_seq: u64,
    when: i64,
) -> u64 {
    for step in 0..LADDER_DEPTH as u64 {
        let at = when + step as i64 * 1_000_000;
        spin_at(engine, first_seq + step, at);
        let placed = drain_commands(commands);
        assert_eq!(
            places(&placed),
            1,
            "one placement per spin is the reconciler's whole-side rule: {placed:?}"
        );
        ack_placed(engine, &placed, at + 100_000);
    }
    first_seq + LADDER_DEPTH as u64
}

#[test]
fn the_quote_stop_pulls_the_whole_ladder_and_cancels_nothing_twice() {
    let (mut engine, mut commands, mut ui_events) = laddered_engine();
    make_ready(&mut engine, OPEN);
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, OPEN, CLOSE, OPEN)),
    );
    for message in reseat_book(BID, ASK, OPEN) {
        engine.dispatch(pop(0, 0), &message);
    }
    drain_commands(&mut commands);
    let seq = seat_ladder(&mut engine, &mut commands, 1, OPEN + 10 * 1_000_000);

    let stop = CLOSE - MARGIN.micros();
    spin_at(&mut engine, seq, stop);
    let swept = drain_commands(&mut commands);
    assert_eq!(cancels(&swept), LADDER_DEPTH);
    assert_eq!(places(&swept), 0, "and nothing new may go on it: {swept:?}");
    assert!(
        refusals(&mut ui_events).contains(&RejectReason::OutsideWindow),
        "the reason the side stopped quoting is the window, and it must say so"
    );

    spin_at(&mut engine, seq + 1, stop + 1_000_000);
    let again = drain_commands(&mut commands);
    assert_eq!(cancels(&again), 0);

    ack_cancelled(&mut engine, &swept, stop + 2_000_000);
    let next_open = CLOSE;
    let next_close = next_open + 300 * 1_000_000;
    engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, next_open, next_close, next_open)),
    );
    for message in reseat_book(BID, ASK, next_open + 1_000_000) {
        engine.dispatch(pop(0, 0), &message);
    }
    drain_commands(&mut commands);
    let seq = seat_ladder(&mut engine, &mut commands, seq + 2, next_open + 2_000_000);

    spin_at(&mut engine, seq, next_close - MARGIN.micros());
    let swept_again = drain_commands(&mut commands);
    assert_eq!(cancels(&swept_again), LADDER_DEPTH);
}

#[test]
fn a_sweep_pulls_resting_quotes_and_never_a_marketable_order() {
    let quote = |state: OrderState| OrderSlot {
        state,
        style: Some(OrderStyle::PostOnly),
        ..OrderSlot::EMPTY
    };
    assert!(quote(OrderState::Live).is_resting_quote());
    for unanswered in [
        OrderState::PendingNew,
        OrderState::CancelInFlight,
        OrderState::AmendInFlight,
        OrderState::Unknown,
    ] {
        assert!(!quote(unanswered).is_resting_quote());
    }
    assert!(
        !OrderSlot {
            state: OrderState::Live,
            style: Some(OrderStyle::Immediate),
            ..OrderSlot::EMPTY
        }
        .is_resting_quote()
    );
}

#[test]
fn an_instrument_with_no_window_quotes_whatever_the_margin_says() {
    let (mut engine, mut commands, _ui_events) = windowed_engine();
    make_ready(&mut engine, 0);
    for message in reseat_book(BID, ASK, 10) {
        engine.dispatch(pop(0, 0), &message);
    }
    drain_commands(&mut commands);

    // An instant that would be deep past the stop of any window, on an instrument that has none.
    spin_at(&mut engine, 1, 20 * ONE);
    let commands = drain_commands(&mut commands);
    assert!(places(&commands) > 0);
    assert_eq!(cancels(&commands), 0, "and nothing to sweep: {commands:?}");
}

fn ack_placed(engine: &mut HotEngine, commands: &[ExecCommand], when: i64) {
    for command in commands {
        let ExecCommand::Place {
            client_id,
            side,
            price,
            ..
        } = command
        else {
            continue;
        };
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::Exec(ExecEvent {
                kind: ExecKind::AckPlaced,
                status: Some(VenueOrderStatus::New),
                ..exec_event(INSTRUMENT, *client_id, *side, price.0, when)
            }),
        );
    }
}

fn ack_cancelled(engine: &mut HotEngine, commands: &[ExecCommand], when: i64) {
    for command in commands {
        let ExecCommand::Cancel { client_id, .. } = command else {
            continue;
        };
        engine.dispatch(
            pop(0, 0),
            &InboundMessage::Exec(ExecEvent {
                kind: ExecKind::ReportCanceled,
                status: Some(VenueOrderStatus::Canceled),
                ..exec_event(INSTRUMENT, *client_id, Side::Buy, BID, when)
            }),
        );
    }
}
