//! FITNESS: the session loss budget is a KILL SWITCH, and what decides whether it halts is whether
//! the loss has been REALISED — never whether the position happens to be exactly flat.
//!
//! The two verdicts differ because only one of them can still come back. A loss still marked to
//! market may be recovered by the position moving, so the engine withdraws the side that would ADD
//! to it and keeps the reducing side quoting: withdrawing both is a deadlock, since the only thing
//! that unwinds a position is a fill and the only thing that produces a fill is a quote
//! (`risk_gate.rs` is that property's own file). A loss booked into closed round trips and paid fees
//! is gone, and no price can hand it back — so it halts.
//!
//! Reading "realised" off an exactly-zero position is what this file exists to stop. One satoshi of
//! residue — the ordinary state after any partial fill — turned the kill switch into a permanent
//! side restriction, and losses then kept growing on the side that was still admitted.

use polysim::config::{RecordedTables, TableKind, TrackerSpec};
use polysim::exposure::InstrumentExposure;
use polysim::hot::dispatch::{ExecWiring, ExposureWiring, HotEngine, HotEngineSetup};
use polysim::hot::exec::{
    ExecLimits, ExecSettings, FeeModel, LossVerdict, OrderBudget, SessionPnl, assess_loss,
};
use polysim::hot::strategy::Strategy;
use polysim::ids::{InstrumentId, Qty, Side};
use polysim::msg::exec::{CancelReason, ExecCommand, ExecLaneItem};
use polysim::msg::inbound::InboundMessage;
use polysim::registry::InstrumentRow;
use polysim::sink::ExecSink;
use polysim::time::DurationUs;
use rtrb::{Consumer, RingBuffer};

use crate::engine_support::{
    FillPen, ONE, book_reset, exposure_ring, instrument_row, metrics_ring, persist_ring_for, pop,
    rotation, snapshot_pair, spin, strategy_log_ring, ui_book_ring, ui_event_ring,
};

/// The shipped config's budget, at the shipped scale.
const BUDGET: i64 = 5 * ONE;

/// One mantissa of base: the residue a partial fill leaves, and the value that used to be the
/// difference between a kill switch and a side restriction.
const DUST: Qty = Qty(1);

fn session(
    mark_to_market_quote: Option<i64>,
    realised_quote: i64,
    position_base: Qty,
) -> SessionPnl {
    SessionPnl {
        mark_to_market_quote,
        realised_quote,
        position_base,
    }
}

/// FITNESS: a breached REALISED loss halts whatever the position is. Dust, a whole unit, or nothing
/// at all — the money is banked and no mark can return it, so there is no state in which the engine
/// may go on quoting.
#[test]
fn a_realised_breach_halts_whatever_residue_the_position_holds() {
    for position in [Qty(0), DUST, Qty(ONE), Qty(-ONE)] {
        assert_eq!(
            assess_loss(session(Some(-6 * ONE), -6 * ONE, position), BUDGET),
            LossVerdict::Realised,
            "a realised loss past the budget while holding {} is still a realised loss",
            position.0
        );
    }
}

/// FITNESS: a loss that is only marked to market withdraws a side and never halts, because the
/// position moving is exactly what could still recover it — and the side it keeps is the one that
/// shrinks the position.
#[test]
fn an_unrealised_breach_only_withdraws_the_adding_side() {
    assert_eq!(
        assess_loss(session(Some(-6 * ONE), 0, Qty(ONE)), BUDGET),
        LossVerdict::MarkToMarket {
            reducing: Side::Sell
        },
        "a long whose mark fell is recoverable, and selling is the way back"
    );
    assert_eq!(
        assess_loss(session(Some(-6 * ONE), 0, Qty(-ONE)), BUDGET),
        LossVerdict::MarkToMarket {
            reducing: Side::Buy
        }
    );
    assert_eq!(
        assess_loss(session(Some(-4 * ONE), -4 * ONE, DUST), BUDGET),
        LossVerdict::Within,
        "inside the budget on both legs, so nothing is withdrawn at all"
    );
}

/// FITNESS: an instrument that has never had an honest valuation still has an honest REALISED
/// number. There is no mark-to-market verdict to give before the first two-sided book, but a round
/// trip closed in that window really did lose the money it lost.
#[test]
fn realised_is_judged_before_any_mark_exists() {
    assert_eq!(
        assess_loss(session(None, -6 * ONE, DUST), BUDGET),
        LossVerdict::Realised
    );
    assert_eq!(
        assess_loss(session(None, -4 * ONE, Qty(ONE)), BUDGET),
        LossVerdict::Within,
        "no mark means no mark-to-market verdict — it must not be invented as a breach"
    );
}

/// FITNESS: losing nothing is not losing. The breach has to be a LOSS at least as large as the
/// budget, not merely `pnl <= -budget` — that reads break-even as a breach the moment the budget is
/// ZERO, which is the value every budget takes in the execution-off shape, so the kill switch would
/// fire on an engine that has not made one decision.
#[test]
fn a_ledger_that_has_lost_nothing_never_breaches() {
    for budget in [0, BUDGET] {
        assert_eq!(
            assess_loss(session(Some(0), 0, Qty(0)), budget),
            LossVerdict::Within,
            "nothing traded, nothing lost"
        );
        assert_eq!(
            assess_loss(session(Some(ONE), ONE, Qty(ONE)), budget),
            LossVerdict::Within,
            "a profit is not a breach at any budget"
        );
    }
    assert_eq!(
        assess_loss(session(Some(-1), -1, Qty(0)), 0),
        LossVerdict::Realised,
        "a budget of zero admits no loss, so one mantissa of one is still a breach"
    );
}

/// Reads nothing and declares nothing: every command the tests below assert on is the ENGINE's.
struct Idle;
impl Strategy for Idle {}

struct Fixture {
    engine: HotEngine,
    commands: Consumer<ExecLaneItem>,
}

fn fixture(max_session_loss_quote: i64) -> Fixture {
    fixture_restoring(max_session_loss_quote, &[])
}

/// Every limit except the session budget set so wide it cannot be what stops a quote, so a halt in
/// the command stream can only have come from the loss gate.
fn fixture_restoring(max_session_loss_quote: i64, restored: &[InstrumentExposure]) -> Fixture {
    let instruments: [InstrumentRow; 1] = [instrument_row(0, TrackerSpec::default(), 64)];
    let (exposure_sink, _snapshots) = exposure_ring(16);
    let (persistence, _records) =
        persist_ring_for(1_024, RecordedTables::new(&[TableKind::Orders]));
    let (strategy_log_sink, _logs) = strategy_log_ring(64);
    let (metrics_sink, _metrics) = metrics_ring(64);
    let (ui_book_sink, _ui_books) = ui_book_ring(64);
    let (ui_event_sink, _ui_events) = ui_event_ring(256);
    let (commands_producer, commands) = RingBuffer::<ExecLaneItem>::new(256);
    let engine = HotEngine::new(HotEngineSetup {
        instruments: &instruments,
        strategy: Box::new(Idle),
        persistence: Some(persistence),
        strategy_log_sink,
        metrics_sink,
        ui_book_sink,
        ui_event_sink,
        link: None,
        warmup: DurationUs::ZERO,
        exec: Some(ExecWiring {
            sink: ExecSink::new(commands_producer),
            settings: ExecSettings {
                limits: ExecLimits {
                    requote_threshold_ticks: 1,
                    max_quote_distance_centi_bps: 100_000_000,
                    max_book_age: DurationUs::from_secs(3_600),
                    max_order_notional_quote: 1_000_000 * ONE,
                },
                max_orders_per_side: 1,
                min_base_balance: 0,
                min_quote_balance: 0,
                max_consecutive_rejects: 5,
                max_session_loss_quote,
                inflight_timeout: DurationUs::from_secs(3_600),
                // No silence sweep: a reconciliation request in the command stream would be noise
                // to filter rather than a behaviour these tests are about.
                exec_silence_spins: u32::MAX,
                order_reap_window: DurationUs::from_secs(3_600),
                quote_stop_margin: DurationUs::ZERO,
                flatten_slack_ticks: 0,
                order_budget: OrderBudget::NONE,
                fee_model: FeeModel::None,
                taker_fee_rate: 0,
                holds_reservations_until_settled: true,
            },
            run_nonce: 0,
        }),
        exposure: ExposureWiring {
            restored,
            sink: exposure_sink,
        },
    });
    Fixture { engine, commands }
}

/// Drop the book and restate it whole, so the mark moves with no transient crossing to reason about.
fn reseat_book(bid: i64, ask: i64, when: i64) -> [InboundMessage; 3] {
    let (bids, asks) = snapshot_pair(0, &[(bid, ONE)], &[(ask, ONE)], when);
    [
        InboundMessage::BookReset(book_reset(0, when)),
        InboundMessage::Book(bids),
        InboundMessage::Book(asks),
    ]
}

fn dispatch_all(fixture: &mut Fixture, messages: &[InboundMessage]) {
    for message in messages {
        fixture.engine.dispatch(pop(0, 0), message);
    }
}

fn spin_at(fixture: &mut Fixture, seq: u64, when: i64) {
    fixture
        .engine
        .dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(seq, when)));
}

/// Whether the engine pulled every order it owns because the kill switch tripped. That command is
/// what a halt DOES, so it is the honest way to observe one from outside.
fn has_halted(commands: &mut Consumer<ExecLaneItem>) -> bool {
    let mut halted = false;
    while let Ok(item) = commands.pop() {
        let ExecLaneItem::Command(stamped) = item else {
            continue;
        };
        halted |= matches!(
            stamped.command,
            ExecCommand::CancelOurs {
                reason: CancelReason::Halt,
                ..
            }
        );
    }
    halted
}

/// FITNESS: a round trip that loses more than the budget and leaves one mantissa behind HALTS. This
/// is the whole finding: at `signum() == 0` the residue read as an open position, the verdict
/// downgraded to a side restriction, and the budget stopped being a kill switch for the rest of the
/// run while losses kept growing on the side that was still admitted.
#[test]
fn a_round_trip_that_breaches_the_budget_halts_even_holding_dust() {
    let mut fixture = fixture(BUDGET);
    dispatch_all(&mut fixture, &reseat_book(99 * ONE, 101 * ONE, 0));
    spin_at(&mut fixture, 0, 10);
    assert!(
        !has_halted(&mut fixture.commands),
        "nothing has traded yet, so nothing may have tripped"
    );

    // Buy a whole unit at 100, then sell all but one mantissa of it at 90: ten quote units realised
    // against a budget of five, with dust left over.
    let mut pen = FillPen::new(0);
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 100 * ONE, ONE, 20));
    dispatch_all(&mut fixture, &reseat_book(89 * ONE, 91 * ONE, 30));
    dispatch_all(&mut fixture, &pen.fill(Side::Sell, 90 * ONE, ONE - 1, 40));
    spin_at(&mut fixture, 1, 50);

    assert!(
        has_halted(&mut fixture.commands),
        "ten quote units are gone against a budget of five and the engine is still quoting — one \
         mantissa of residue must not downgrade a kill switch to a side restriction"
    );
}

/// FITNESS: the same shaped loss that is NOT realised does not halt. Without this the test above
/// passes for a gate that halts on any breach at all, which would deadlock every position it was
/// meant to protect — the engine must keep the reducing side quoting or it can never get back to
/// flat.
#[test]
fn an_unrealised_loss_of_the_same_size_does_not_halt() {
    let mut fixture = fixture(BUDGET);
    dispatch_all(&mut fixture, &reseat_book(99 * ONE, 101 * ONE, 0));
    spin_at(&mut fixture, 0, 10);

    let mut pen = FillPen::new(0);
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 100 * ONE, ONE, 20));
    // The mark alone falls to 90 — no second fill, so nothing is closed and nothing is banked.
    dispatch_all(&mut fixture, &reseat_book(89 * ONE, 91 * ONE, 30));
    spin_at(&mut fixture, 1, 40);

    assert!(
        !has_halted(&mut fixture.commands),
        "a position whose mark fell is still recoverable by trading, and halting there traps it"
    );
}

/// One sell against `opened`, run with NO book at all so nothing has a mark: the mark-to-market leg
/// has no verdict to give and the halt can only have come from the realised one.
fn halts_after(opened: &[(i64, i64)], closed: &[(i64, i64)], budget: i64) -> bool {
    halts_after_restoring(&[], opened, closed, budget)
}

fn halts_after_restoring(
    restored: &[InstrumentExposure],
    opened: &[(i64, i64)],
    closed: &[(i64, i64)],
    budget: i64,
) -> bool {
    let mut fixture = fixture_restoring(budget, restored);
    let mut pen = FillPen::new(0);
    let mut when = 10;
    for (side, fills) in [(Side::Buy, opened), (Side::Sell, closed)] {
        for (price, qty) in fills {
            dispatch_all(&mut fixture, &pen.fill(side, *price, *qty, when));
            when += 10;
        }
    }
    spin_at(&mut fixture, 0, when);
    has_halted(&mut fixture.commands)
}

/// FITNESS: what the kill switch measures is the AVERAGE COST realised loss, and this pins the
/// number rather than the direction — the same tape is run at a budget one unit inside it and one
/// unit outside, so the gate has to fire in exactly one of the two.
///
/// Two lot prices and a partial close is the case where the plausible alternatives separate. Buying
/// at 100 and 120 and selling one unit at 100 loses ten at average cost; FIFO would say zero,
/// because the lot it closes is the one bought at 100. A gate reading cash alone would say twenty,
/// which is the whole position's unrealised loss with nothing closed at all. The second pair drives
/// a sell straight THROUGH flat into a short, where the basis released and the basis opened come out
/// of one fill and a fold that took either for the whole would be wrong by the other.
#[test]
fn the_halt_fires_on_the_average_cost_realised_loss() {
    let two_lots = [(100 * ONE, ONE), (120 * ONE, ONE)];
    let partial_close = [(100 * ONE, ONE)];
    assert!(
        halts_after(&two_lots, &partial_close, 9 * ONE),
        "closing one of two units bought at 100 and 120 for 100 realises ten — FIFO would call it \
         nothing and go on quoting"
    );
    assert!(
        !halts_after(&two_lots, &partial_close, 11 * ONE),
        "ten realised is inside a budget of eleven, and a gate reading the whole position's cash \
         would have found twenty"
    );

    // Selling two units against a position of one closes it and opens a short with the remainder.
    let through_flat = [(100 * ONE, ONE), (90 * ONE, 2 * ONE)];
    assert!(
        halts_after(&two_lots, &through_flat, 29 * ONE),
        "ten from the partial close and twenty more selling the last unit at 90 against a basis \
         of 110"
    );
    assert!(
        !halts_after(&two_lots, &through_flat, 31 * ONE),
        "the short the flip opened must carry its own cost basis rather than realise on arrival"
    );
}

/// A row a previous run left: `cash` is every run's banked result so far, `basis` is what the
/// position still held actually cost. The two are independent, and that is the point — a flat row
/// with cash in it is an engine that has traded before and is holding nothing right now.
fn restored(position_base: i64, cash_quote: i64, basis_quote: i64) -> [InstrumentExposure; 1] {
    [InstrumentExposure {
        instrument: InstrumentId(0),
        position_base: Qty(position_base),
        cash_quote,
        basis_quote,
    }]
}

/// FITNESS: a profit banked by PREVIOUS runs does not extend this run's budget. The gate has to fire
/// at five quote units lost THIS session, not at fifty-five.
///
/// The failure it forbids is fail-open and unbounded, which is what makes it a kill-switch defect
/// rather than an accounting one: a ledger seeded with `basis = -cash` releases the inherited
/// fifty back into realised PnL as the position closes, so the engine reads a profit while it is
/// losing money, and every further run that ends ahead raises the ceiling again.
#[test]
fn an_inherited_profit_never_extends_this_sessions_budget() {
    let banked_profit = restored(0, 50 * ONE, 0);

    // Bought at 100 and sold at 94: six lost this session against a budget of five.
    assert!(
        halts_after_restoring(
            &banked_profit,
            &[(100 * ONE, ONE)],
            &[(94 * ONE, ONE)],
            BUDGET
        ),
        "six quote units are gone this session and the engine is still quoting — it is spending an \
         earlier run's profit as if it were budget"
    );
    // Four lost, which is inside the budget — so the test above is reading the LIMIT and not merely
    // a gate that halts on any closed trade at all.
    assert!(
        !halts_after_restoring(
            &banked_profit,
            &[(100 * ONE, ONE)],
            &[(96 * ONE, ONE)],
            BUDGET
        ),
        "four quote units lost is inside a budget of five"
    );
}

/// FITNESS: a loss banked by PREVIOUS runs does not halt a fresh one. The mirror of the test above
/// and the fail-CLOSED direction: read absolutely, an inherited fifty-unit loss trips the switch on
/// the first completed round trip of every run that follows, however well that run is doing.
#[test]
fn an_inherited_loss_never_halts_a_fresh_run_on_its_first_round_trip() {
    let banked_loss = restored(0, -50 * ONE, 0);

    assert!(
        !halts_after_restoring(
            &banked_loss,
            &[(100 * ONE, ONE)],
            &[(100 * ONE, ONE)],
            BUDGET
        ),
        "this session bought and sold at the same price and has lost nothing — the halt came from a \
         previous run's result"
    );
}

/// FITNESS: closing a position this run INHERITED books only what this run made or lost on it. The
/// restored basis is what the position really cost, and it is carried in the file rather than
/// inferred from cash precisely so a run that both banked a result and kept inventory — the ordinary
/// state of any engine that has been running a while — restores both facts separately.
#[test]
fn closing_a_restored_position_books_only_this_runs_result() {
    // Thirty banked and one unit still held, bought at 100: cash is the two together, basis is the
    // position alone. Inferring `basis = -cash` would call the position's cost seventy.
    let held = restored(ONE, -70 * ONE, 100 * ONE);

    assert!(
        halts_after_restoring(&held, &[], &[(95 * ONE, ONE)], BUDGET),
        "selling a unit that cost 100 for 95 loses five, which is the budget exactly"
    );
    assert!(
        !halts_after_restoring(&held, &[], &[(95 * ONE, ONE)], 6 * ONE),
        "and five is inside a budget of six — the loss booked is this run's, not the whole cash leg"
    );
}

/// A rotation, far enough from its close that nothing in the quote-stop path fires — the only thing
/// under test here is what the ledger reset does to the budget.
fn rotate(fixture: &mut Fixture, when: i64) {
    fixture.engine.dispatch(
        pop(0, 0),
        &InboundMessage::MarketRotation(rotation(0, when, when + 300 * 1_000_000, when)),
    );
}

/// FITNESS: the budget spans the RUN, not the market. A venue that rotates every few minutes hands
/// the ledger a new row each time, and the losses of the window just retired go with it — so a
/// switch reading only the live row re-baselines twelve times an hour and can never reach a budget
/// no single window spends.
///
/// Two identical round trips either side of one rotation, each inside the budget and together past
/// it. The pair is the point: the first assertion proves the fixture really is inside the budget
/// before the rotation, so the halt after it cannot be the gate firing early.
#[test]
fn losses_realised_before_a_rotation_still_count_against_the_budget() {
    let mut fixture = fixture(BUDGET);
    let mut pen = FillPen::new(0);

    // No book anywhere in this test, so nothing has a mark and the halt can only come from the
    // realised leg.
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 100 * ONE, ONE, 10));
    dispatch_all(&mut fixture, &pen.fill(Side::Sell, 97 * ONE, ONE, 20));
    spin_at(&mut fixture, 0, 30);
    assert!(
        !has_halted(&mut fixture.commands),
        "three units lost against a budget of five is inside it"
    );

    rotate(&mut fixture, 40);
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 100 * ONE, ONE, 50));
    dispatch_all(&mut fixture, &pen.fill(Side::Sell, 97 * ONE, ONE, 60));
    spin_at(&mut fixture, 1, 70);

    assert!(
        has_halted(&mut fixture.commands),
        "six units are gone against a budget of five and the engine is still quoting — the rotation \
         forgave the first three"
    );
}

/// FITNESS: a rotation must not manufacture a loss out of the profit it just erased.
///
/// The mark-to-market leg is measured against a baseline taken at the first honest valuation, and a
/// rotation zeroes the row that baseline was taken from. Left standing, the next spin reads the
/// whole baseline as this instant's loss — and reads it while FLAT, which is the one shape that
/// halts rather than merely restricting a side. The engine would kill itself for making money.
#[test]
fn a_rotation_does_not_leave_a_stale_baseline_reading_as_a_loss() {
    let mut fixture = fixture(BUDGET);
    let mut pen = FillPen::new(0);

    // Bought below where the book then marks it, and bought BEFORE any book exists so the baseline
    // cannot be taken until the position is already held: the baseline is +10, not zero.
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 90 * ONE, ONE, 10));
    dispatch_all(&mut fixture, &reseat_book(99 * ONE, 101 * ONE, 20));
    spin_at(&mut fixture, 0, 30);
    assert!(
        !has_halted(&mut fixture.commands),
        "a position ten units in profit has breached nothing"
    );

    rotate(&mut fixture, 40);
    spin_at(&mut fixture, 1, 50);
    assert!(
        !has_halted(&mut fixture.commands),
        "the position and its profit were erased together, so the difference between them is not a \
         ten-unit loss this run has to answer for"
    );

    // And the gate is silenced, not disarmed: a real loss on the new window still kills the run.
    dispatch_all(&mut fixture, &pen.fill(Side::Buy, 100 * ONE, ONE, 60));
    dispatch_all(&mut fixture, &pen.fill(Side::Sell, 94 * ONE, ONE, 70));
    spin_at(&mut fixture, 2, 80);
    assert!(
        has_halted(&mut fixture.commands),
        "six units realised on the new window is past the budget however clean the slate was"
    );
}
