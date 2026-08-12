//! Polymarket execution POLICY: the decisions the driver makes that no wire format dictates.
//!
//! Every one of these exists because the venue mints the only order id there is, and mints it in
//! the answer to the placement. That single fact produces a held-frame buffer, an adopt-or-leave
//! rule for orders nobody can name, a cancel that has to wait out the venue's own hold, and a token
//! binding that is only tradeable once three separate reads agree. Socket behaviour stays out;
//! what is pinned here is what the driver DECIDES.

use polysim::adapters::exec::{ExecCore, ExecEffect, ExecRequest, MirroredOrder, RequestId};
use polysim::adapters::polymarket::exec::binding::{
    BindingStep, Bindings, ENRICHMENT_RETRY, EnrichmentRead, MAX_ENRICHMENT_ATTEMPTS,
};
use polysim::adapters::polymarket::exec::codec::{
    DecodeContext, HttpAnswer, OrderIndex, OrdersRead, TokenTable, UnmappedOrder, VenueAnswer,
    decode_heartbeat, decode_neg_risk, decode_orders_page,
};
use polysim::adapters::polymarket::exec::correlate::{
    DELAYED_HOLD, DelayedOrders, PENDING_CAPACITY, PENDING_TTL, PendingFrames, UnmappedVerdict,
    WithheldCancel, classify_unmapped, restates_balances,
};
use polysim::adapters::polymarket::rotation::{OutcomeLeg, TokenId, WindowAssignment};
use polysim::ids::{ClientOrderId, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{ExecCommand, ExecEvent, ExecKind, Provenance, VenueOrderStatus};
use polysim::time::{DurationUs, TsUs};

const UP: InstrumentId = InstrumentId(0);
const DOWN: InstrumentId = InstrumentId(1);
const CONDITION: &str = "0xcondition";
const UP_TOKEN: &str = "111";
const DOWN_TOKEN: &str = "222";

fn at(micros: i64) -> TsUs {
    TsUs::from_micros(micros)
}

fn assignment(open: i64, close: i64) -> WindowAssignment {
    WindowAssignment {
        up: OutcomeLeg {
            instrument: UP,
            token: TokenId::from(UP_TOKEN),
        },
        down: OutcomeLeg {
            instrument: DOWN,
            token: TokenId::from(DOWN_TOKEN),
        },
        window_open_ts_us: at(open),
        window_close_ts_us: at(close),
        condition_id: std::sync::Arc::from(CONDITION),
    }
}

fn ready(step: BindingStep) -> Vec<polysim::adapters::polymarket::exec::binding::ReadyBinding> {
    match step {
        BindingStep::Ready(ready) => ready,
        other => panic!("expected a completed binding, got {other:?}"),
    }
}

/// A window is not tradeable when it arrives. Tick size and the neg-risk flag are separate reads,
/// and the flag picks the exchange contract the signature is checked against — binding early would
/// sign every order for the wrong one.
#[test]
fn a_binding_is_incomplete_until_the_tick_and_both_neg_risk_flags_land() {
    let mut bindings = Bindings::default();

    let step = bindings.on_assignment(&assignment(0, 300_000_000));
    let BindingStep::Enrich {
        condition_id,
        tokens,
    } = step
    else {
        panic!("a fresh assignment must ask for its enrichment reads");
    };
    assert_eq!(&*condition_id, CONDITION);
    assert_eq!(tokens.len(), 2, "one neg-risk read per outcome token");

    assert_eq!(
        bindings.on_market(CONDITION, Price(1_000_000)),
        BindingStep::Wait,
        "tick alone is not enough"
    );
    assert_eq!(
        bindings.on_neg_risk(CONDITION, UP, false),
        BindingStep::Wait,
        "one leg's flag is not enough — the other leg is a separate order"
    );

    let ready = ready(bindings.on_neg_risk(CONDITION, DOWN, false));
    assert_eq!(ready.len(), 2);
    assert!(ready.iter().all(|entry| entry.tick == Price(1_000_000)));
    assert!(ready.iter().all(|entry| !entry.is_neg_risk));
}

/// The venue is the authority on this flag and it is per token, so a `true` answer must survive to
/// the binding: it is the difference between the standard and the neg-risk exchange contract.
#[test]
fn the_neg_risk_flag_reaches_the_binding_unchanged() {
    let mut bindings = Bindings::default();
    bindings.on_assignment(&assignment(0, 300_000_000));
    bindings.on_market(CONDITION, Price(1_000_000));
    bindings.on_neg_risk(CONDITION, UP, true);
    let ready = ready(bindings.on_neg_risk(CONDITION, DOWN, true));
    assert!(ready.iter().all(|entry| entry.is_neg_risk));
}

/// An answer for a window that has already been displaced must not resurrect it. Enrichment reads
/// are slower than a rotation is wide, so late answers are ordinary.
#[test]
fn an_answer_for_an_unknown_window_settles_nothing() {
    let mut bindings = Bindings::default();
    assert_eq!(
        bindings.on_market("0xsomeoneelses", Price(1_000_000)),
        BindingStep::Wait
    );
    assert_eq!(
        bindings.on_neg_risk("0xsomeoneelses", UP, false),
        BindingStep::Wait
    );
}

/// The two enrichment reads leave on the assignment, but a single transient failure of either would
/// leave the instrument unbound for the whole five-minute window — every placement refused
/// `UnboundInstrument`. The retry re-issues only the read still outstanding, so a failed market read
/// followed by a success still yields a usable binding.
#[test]
fn a_transient_enrichment_read_failure_does_not_forfeit_the_window() {
    let mut bindings = Bindings::default();
    let start = at(0);
    let step = bindings.on_assignment(&assignment(0, 300_000_000));
    assert!(matches!(step, BindingStep::Enrich { .. }));

    // The neg-risk reads land for both legs; the market read fails (nothing calls on_market).
    bindings.on_neg_risk(CONDITION, UP, false);
    bindings.on_neg_risk(CONDITION, DOWN, false);

    // The first poll only arms the retry timer — the initial reads already went out on the
    // assignment, so re-issuing now would double them.
    assert!(
        bindings.due_enrichment_reads(start).is_empty(),
        "the first poll arms, it does not re-issue"
    );
    assert!(
        bindings.due_enrichment_reads(start).is_empty(),
        "nothing is due before the retry interval elapses"
    );

    let due = bindings.due_enrichment_reads(start + ENRICHMENT_RETRY);
    assert_eq!(due.len(), 1, "only the read still missing is re-issued");
    assert!(
        matches!(&due[0], EnrichmentRead::Market { condition_id } if &**condition_id == CONDITION),
        "the neg-risk reads already landed, so only the market read comes back"
    );

    // The retry succeeds and the binding completes.
    let ready = ready(bindings.on_market(CONDITION, Price(1_000_000)));
    assert_eq!(
        ready.len(),
        2,
        "the window is usable after a transient blip"
    );
}

/// The retry is bounded: a market that never answers is given up rather than re-read for the rest of
/// the window, which would only burn the read budget.
#[test]
fn a_persistently_unreadable_binding_is_given_up_not_retried_forever() {
    let mut bindings = Bindings::default();
    bindings.on_assignment(&assignment(0, 300_000_000));

    let mut now = at(0);
    bindings.due_enrichment_reads(now); // arms the timer
    for _ in 0..MAX_ENRICHMENT_ATTEMPTS + 3 {
        now = now + ENRICHMENT_RETRY;
        bindings.due_enrichment_reads(now);
    }
    assert!(
        bindings
            .due_enrichment_reads(now + ENRICHMENT_RETRY)
            .is_empty(),
        "past the attempt cap the binding is gone, so no read is due"
    );
    assert_eq!(
        bindings.on_market(CONDITION, Price(1_000_000)),
        BindingStep::Wait,
        "a late answer for a given-up binding settles nothing"
    );
    assert!(bindings.refused() >= 1, "the given-up binding is counted");
}

/// The edge sweep is a backstop, and a backstop that fires every housekeeping tick would spend the
/// venue's cancel bucket on an empty book.
#[test]
fn the_close_margin_backstop_fires_once_per_window() {
    let close = 300_000_000;
    let margin = DurationUs::from_micros(3_000_000);
    let mut bindings = Bindings::default();
    bindings.on_assignment(&assignment(0, close));
    bindings.on_market(CONDITION, Price(1_000_000));
    bindings.on_neg_risk(CONDITION, UP, false);
    bindings.on_neg_risk(CONDITION, DOWN, false);

    assert!(
        bindings
            .close_margin_reached(at(close - 10_000_000), margin)
            .is_empty(),
        "ten seconds out is not inside a three second margin"
    );
    let reached = bindings.close_margin_reached(at(close - 1_000_000), margin);
    assert_eq!(reached.len(), 2, "both legs of the window expire together");
    assert!(reached.iter().any(|leg| &*leg.token_id == UP_TOKEN));
    assert!(
        bindings.close_margin_reached(at(close), margin).is_empty(),
        "the same window must not be swept twice"
    );
}

/// The next window re-arms the backstop: it is a different market with a different close.
#[test]
fn a_fresh_window_re_arms_the_backstop() {
    let margin = DurationUs::from_micros(3_000_000);
    let mut bindings = Bindings::default();
    bindings.on_assignment(&assignment(0, 300_000_000));
    bindings.on_market(CONDITION, Price(1_000_000));
    bindings.on_neg_risk(CONDITION, UP, false);
    bindings.on_neg_risk(CONDITION, DOWN, false);
    assert_eq!(
        bindings.close_margin_reached(at(299_000_000), margin).len(),
        2
    );

    let next = WindowAssignment {
        condition_id: std::sync::Arc::from("0xnext"),
        ..assignment(300_000_000, 600_000_000)
    };
    bindings.on_assignment(&next);
    bindings.on_market("0xnext", Price(1_000_000));
    bindings.on_neg_risk("0xnext", UP, false);
    bindings.on_neg_risk("0xnext", DOWN, false);
    assert_eq!(
        bindings.close_margin_reached(at(599_000_000), margin).len(),
        2,
        "a new window's close is a new sweep"
    );
}

/// A sell against a token whose CLOB allowance cache is cold is refused as an empty wallet however
/// approved the chain is, so the edge tracks warmth per token rather than per account.
#[test]
fn allowance_warmth_is_tracked_per_token() {
    let mut bindings = Bindings::default();
    assert!(!bindings.is_allowance_warm(UP_TOKEN));
    bindings.claim_allowance_refresh(UP_TOKEN);
    bindings.on_allowance_answered(UP_TOKEN, true);
    assert!(bindings.is_allowance_warm(UP_TOKEN));
    assert!(
        !bindings.is_allowance_warm(DOWN_TOKEN),
        "the other outcome is a different token with its own cache entry"
    );
}

/// The refresh endpoint allows 50 calls per ten seconds and a withheld sell is re-decided every
/// spin. Asking again while an answer is outstanding would spend that budget on one question.
#[test]
fn one_allowance_refresh_is_outstanding_per_token_at_a_time() {
    let mut bindings = Bindings::default();
    assert!(
        bindings.claim_allowance_refresh(UP_TOKEN),
        "the first ask is sent"
    );
    assert!(
        !bindings.claim_allowance_refresh(UP_TOKEN),
        "a second ask while the first is unanswered is not"
    );
    assert!(
        bindings.claim_allowance_refresh(DOWN_TOKEN),
        "a different token is a different question"
    );

    // A refusal has to leave the token askable, or a single failed refresh withholds its sells for
    // the rest of the run.
    bindings.on_allowance_answered(UP_TOKEN, false);
    assert!(!bindings.is_allowance_warm(UP_TOKEN));
    assert!(bindings.claim_allowance_refresh(UP_TOKEN));

    bindings.on_allowance_answered(UP_TOKEN, true);
    assert!(
        !bindings.claim_allowance_refresh(UP_TOKEN),
        "a warm cache is asked about no further"
    );
}

/// A claim only ever clears on an answer, so a refresh that never reached the venue would hold the
/// token marked in-flight forever — and every sell on it is withheld waiting for an answer nobody
/// asked for. The driver hands the claim back when the request is dropped.
#[test]
fn a_refresh_that_never_left_leaves_the_token_askable() {
    let mut bindings = Bindings::default();
    assert!(bindings.claim_allowance_refresh(UP_TOKEN));
    bindings.release_allowance_refresh(UP_TOKEN);
    assert!(
        bindings.claim_allowance_refresh(UP_TOKEN),
        "a dropped refresh must not withhold this token's sells for the rest of the run"
    );
}

fn held(frames: &mut PendingFrames, text: &str, now: i64) {
    frames.hold(text.to_owned(), at(now));
}

/// The window between a placement's bytes leaving and its answer landing is one in which the venue
/// can already be reporting fills on it. Dropping those frames loses a fill.
#[test]
fn a_frame_naming_an_unmapped_order_is_held_and_handed_back_intact() {
    let mut frames = PendingFrames::new();
    held(&mut frames, "{\"event_type\":\"order\"}", 0);
    assert_eq!(frames.len(), 1);

    let drained = frames.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].text, "{\"event_type\":\"order\"}");
    assert_eq!(
        frames.len(),
        0,
        "a drain empties the buffer for the re-read"
    );

    // Still unattributable: the caller hands it back rather than losing it.
    frames.re_hold(drained[0].clone());
    assert_eq!(frames.len(), 1);
}

/// Bounded by design. The oldest goes because a newer frame describes a more recent state, and
/// the loss is counted rather than silent.
#[test]
fn the_held_buffer_drops_the_oldest_and_counts_it() {
    let mut frames = PendingFrames::new();
    for index in 0..PENDING_CAPACITY {
        held(&mut frames, &format!("frame-{index}"), index as i64);
    }
    assert_eq!(frames.dropped(), 0);

    held(&mut frames, "frame-overflow", PENDING_CAPACITY as i64);
    assert_eq!(
        frames.len(),
        PENDING_CAPACITY,
        "capacity is a cap, not a hint"
    );
    assert_eq!(frames.dropped(), 1);
    let remaining = frames.drain();
    assert_eq!(
        remaining.first().map(|frame| frame.text.as_str()),
        Some("frame-1"),
        "the oldest frame is the one that left"
    );
    assert_eq!(
        remaining.last().map(|frame| frame.text.as_str()),
        Some("frame-overflow")
    );
}

/// A frame no mapping ever explains describes a fill the ledger has not seen. It leaves on a
/// deadline and is counted, so the driver can force a re-read instead of waiting forever.
#[test]
fn a_held_frame_expires_on_its_deadline_and_is_counted() {
    let mut frames = PendingFrames::new();
    held(&mut frames, "early", 0);
    held(&mut frames, "late", 5_000_000);

    assert!(
        frames.expired(at(PENDING_TTL.micros() - 1)).is_empty(),
        "nothing expires before the ttl"
    );
    let expired = frames.expired(at(PENDING_TTL.micros() + 1));
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].text, "early");
    assert_eq!(frames.abandoned(), 1);
    assert_eq!(frames.len(), 1, "the younger frame keeps waiting");
}

fn cancel_request(client_id: ClientOrderId) -> ExecRequest {
    ExecRequest::Cancel {
        instrument: UP,
        client_id,
    }
}

/// Request ids are the core's to mint, so the one under test is a real one: the identity that must
/// survive the wait is the identity the core issued, not a number the test invented.
fn minted_cancel(client_id: ClientOrderId) -> (RequestId, ExecRequest) {
    let mut core = ExecCore::with_limits(4, 16);
    // Cancels are admitted from any connected phase; a core still Down would refuse this one.
    core.on_connected(&mut |_| {});
    let mut minted = None;
    core.on_command(
        ExecCommand::Cancel {
            instrument: UP,
            client_id,
        },
        &mut |effect| {
            if let ExecEffect::Send {
                request_id,
                request,
            } = effect
            {
                minted = Some((request_id, request));
            }
        },
    );
    minted.expect("a cancel for an unmirrored order is sent straight through")
}

/// A marketable order is held 250 ms at the venue and CANNOT be cancelled during it. Sending the
/// cancel anyway earns a refusal that counts against the hard-reject streak, so it waits.
#[test]
fn a_cancel_inside_the_taker_hold_is_withheld_and_released_once() {
    let client_id = ClientOrderId(0x5150);
    let mut delayed = DelayedOrders::default();
    delayed.on_delayed(client_id, at(1_000_000));

    let release_at = delayed
        .held_until(client_id, at(1_000_000))
        .expect("an order inside its hold reports when it leaves it");
    assert_eq!(release_at, at(1_000_000) + DELAYED_HOLD);
    assert!(
        delayed
            .held_until(client_id, at(1_000_000) + DELAYED_HOLD)
            .is_none(),
        "the hold ends at the stamp, it does not linger"
    );

    let (minted_id, minted_request) = minted_cancel(client_id);
    delayed.withhold(WithheldCancel {
        request_id: minted_id,
        request: minted_request,
        recon_seq: 3,
        release_at,
    });
    assert!(
        delayed.released(at(1_100_000)).is_empty(),
        "the hold is still running"
    );

    let released = delayed.released(release_at);
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].recon_seq, 3);
    assert!(
        delayed.released(at(9_999_999)).is_empty(),
        "a released cancel is sent ONCE — never-retry survives the delay"
    );

    // The identity the core minted rides through the wait: re-deciding would mint a second request
    // id for an order the core already believes has one cancel outstanding.
    let ExecEffect::Send {
        request_id,
        request,
    } = released[0].into_effect()
    else {
        panic!("a released cancel is a send");
    };
    assert_eq!(request_id, minted_id);
    assert_eq!(request, cancel_request(client_id));
}

/// The hold belongs to the order, not to the run: once the order is gone the stamp must go with it
/// or a later id reusing the slot inherits a wait it never earned.
#[test]
fn a_terminal_order_forgets_its_hold() {
    let client_id = ClientOrderId(0x5150);
    let mut delayed = DelayedOrders::default();
    delayed.on_delayed(client_id, at(1_000_000));
    delayed.forget(client_id);
    assert!(delayed.held_until(client_id, at(1_000_000)).is_none());
}

/// A placement whose POST is still outstanding: mine, no venue id, not yet ambiguous.
fn mine(client_id: u64, side: Side, price: i64, qty: i64) -> MirroredOrder {
    MirroredOrder {
        instrument: UP,
        client_id: ClientOrderId(client_id),
        side,
        price: Price(price),
        qty: Qty(qty),
        provenance: Provenance::Mine,
        has_sent_cancel: false,
        is_ambiguous: false,
    }
}

/// A placement whose answer will never come — it timed out or its transport failed, which marks the
/// mirror entry ambiguous. Only such a slot is adoptable; a live in-flight one is deferred.
fn lost(client_id: u64, side: Side, price: i64, qty: i64) -> MirroredOrder {
    MirroredOrder {
        is_ambiguous: true,
        ..mine(client_id, side, price, qty)
    }
}

fn resting(side: Side, price: i64, qty: i64) -> UnmappedOrder {
    UnmappedOrder {
        instrument: UP,
        venue_order_id: "0xvenue".into(),
        side,
        price: Price(price),
        qty: Qty(qty),
        filled: Qty(0),
        status: VenueOrderStatus::New,
    }
}

/// A placement whose answer was lost leaves an order this run placed and cannot name. Adopting it is
/// the only way the engine regains the ability to cancel it.
#[test]
fn an_unmapped_order_is_adopted_by_the_placement_that_lost_its_answer() {
    let mirror = [lost(0xa1, Side::Buy, 45_000_000, 500_000_000)];
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 45_000_000, 500_000_000),
            |_| false,
        ),
        UnmappedVerdict::Adopt(ClientOrderId(0xa1)),
    );
}

/// R16: while a placement on the side is still in flight — a mine mirror order with no venue id that
/// is not yet ambiguous — an unmapped resting order there is indistinguishable from a person's own
/// order at the venue. Adopting it would bind a SECOND venue id to our slot the moment our own answer
/// lands, double-folding the position. So it is deferred, not classified, until the place resolves.
#[test]
fn an_unmapped_order_is_deferred_while_a_placement_on_the_side_is_in_flight() {
    let mirror = [mine(0xa1, Side::Buy, 45_000_000, 500_000_000)];
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 45_000_000, 500_000_000),
            |_| false,
        ),
        UnmappedVerdict::Defer,
        "an exact price and size match still defers — the answer decides, not a guess",
    );
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 44_000_000, 700_000_000),
            |_| false,
        ),
        UnmappedVerdict::Defer,
        "deferral is keyed on the side, not on a match: a different order on it still waits",
    );
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Sell, 45_000_000, 500_000_000),
            |_| false,
        ),
        UnmappedVerdict::LeaveAlone,
        "the OTHER side has no placement in flight, so an order there is not deferred",
    );
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 45_000_000, 500_000_000),
            |_| true,
        ),
        UnmappedVerdict::LeaveAlone,
        "once our own placement's venue id has arrived it blocks nothing, and the resting order is \
         then unexplained — left alone, never adopted",
    );
}

/// Same credentials reach the venue's own website order entry. An order nothing this run placed
/// explains is somebody's, and cancelling it is not a recovery.
#[test]
fn an_unmapped_order_with_no_matching_placement_is_left_alone() {
    let mirror = [lost(0xa1, Side::Buy, 45_000_000, 500_000_000)];
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Sell, 45_000_000, 500_000_000),
            |_| false,
        ),
        UnmappedVerdict::LeaveAlone,
        "the other side is not this order"
    );
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 45_000_000, 500_000_000),
            |_| true,
        ),
        UnmappedVerdict::LeaveAlone,
        "an order whose venue id already arrived is accounted for; this one is not it"
    );
    assert_eq!(
        classify_unmapped(&[], &resting(Side::Buy, 45_000_000, 500_000_000), |_| false),
        UnmappedVerdict::LeaveAlone,
        "an empty mirror explains nothing"
    );
}

/// A ladder can leave more than one unanswered slot on a side, so the price and size decide which.
#[test]
fn price_and_size_pick_between_two_unanswered_placements() {
    let mirror = [
        lost(0xa1, Side::Buy, 45_000_000, 500_000_000),
        lost(0xa2, Side::Buy, 44_000_000, 700_000_000),
    ];
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 44_000_000, 700_000_000),
            |_| false,
        ),
        UnmappedVerdict::Adopt(ClientOrderId(0xa2)),
    );
}

/// A prior run's order carries no client id this run could mint, so it can never be an adoption —
/// the boot sweep cancels it by venue id instead.
#[test]
fn a_prior_run_order_is_never_adopted() {
    let mirror = [MirroredOrder {
        provenance: Provenance::PriorRun,
        ..lost(0xa1, Side::Buy, 45_000_000, 500_000_000)
    }];
    assert_eq!(
        classify_unmapped(
            &mirror,
            &resting(Side::Buy, 45_000_000, 500_000_000),
            |_| false,
        ),
        UnmappedVerdict::LeaveAlone,
    );
}

/// At boot the token table is empty until the first rotation lands, so a prior run's order rests on
/// a token this run cannot map. Dropped, it would fill behind an armed engine spending real balance
/// the ledger never sees. The codec must SURFACE it — id and side, which is all a cancel needs — so
/// the boot branch can sweep it by venue id; the previous behaviour returned it as nothing.
#[test]
fn an_order_on_an_unbound_token_is_surfaced_for_the_boot_sweep_not_dropped() {
    let tokens = TokenTable::with_retired_capacity(4);
    let orders = OrderIndex::with_capacity(16);
    let context = DecodeContext {
        tokens: &tokens,
        orders: &orders,
        api_key: "apikey",
        received_ts_us: at(0),
    };
    let body = r#"{"data":[{"id":"0xprior","asset_id":"unbound-token","side":"SELL","price":"0.62","original_size":"5","size_matched":"0","status":"LIVE","created_at":1}]}"#;
    let VenueAnswer::Answered(decoded) = decode_orders_page(
        HttpAnswer { status: 200, body },
        OrdersRead {
            instrument: UP,
            recon_seq: 0,
        },
        &context,
    )
    .expect("a well-formed page decodes") else {
        panic!("a 200 page is answered, not unavailable");
    };
    assert!(
        decoded.unmapped.is_empty(),
        "the token is bound to nothing, so this is not a bound-token unmapped order"
    );
    assert_eq!(
        decoded.unattributable.len(),
        1,
        "an order on an unbound token is surfaced, never silently dropped"
    );
    assert_eq!(&*decoded.unattributable[0].venue_order_id, "0xprior");
    assert_eq!(decoded.unattributable[0].side, Side::Sell);
    assert!(
        decoded
            .events
            .iter()
            .all(|event| event.kind == ExecKind::SnapshotEnd),
        "nothing was mappable, so the only event is the readiness marker"
    );
}

fn event(kind: ExecKind, last_qty: i64, cumulative_qty: i64) -> ExecEvent {
    ExecEvent {
        instrument: UP,
        client_id: ClientOrderId(0xa1),
        venue_order_id: None,
        trade_id: None,
        kind,
        status: None,
        reject: None,
        provenance: Provenance::Mine,
        side: Side::Buy,
        liquidity: None,
        price: Price(45_000_000),
        qty: Qty(500_000_000),
        last_price: Price(45_000_000),
        last_qty: Qty(last_qty),
        cumulative_qty: Qty(cumulative_qty),
        cumulative_quote: 0,
        commission: 0,
        commission_asset: polysim::ids::AssetId::UNKNOWN,
        reject_code: 0,
        amends_remaining: 0,
        recon_seq: 0,
        exchange_ts_us: at(0),
        request_sent_ts_us: None,
        received_ts_us: at(0),
        queued_ts_us: at(0),
    }
}

/// The hot account table releases a fill's reservation only against a balance stamped LATER than
/// the reservation. An edge that reports a fill and no new balance holds that reservation forever,
/// and the next flatten starves at the funds gate against a wallet that is demonstrably full.
#[test]
fn both_ways_a_fill_is_reported_oblige_a_balance_restatement() {
    assert!(
        restates_balances(&event(ExecKind::ReportTrade, 0, 200_000_000)),
        "a maker fill: the order update is the only report of it that exists"
    );
    assert!(
        restates_balances(&event(ExecKind::AckPlaced, 200_000_000, 200_000_000)),
        "a taker fill: the placement answer reports it once and never again"
    );
}

/// A placement that rested without matching moves no money, and a snapshot is a READ of a fill that
/// was already accounted for. Restating on either would make a resync trigger the read that
/// triggers the next resync.
#[test]
fn a_placement_that_did_not_fill_and_a_snapshot_do_not() {
    assert!(!restates_balances(&event(ExecKind::AckPlaced, 0, 0)));
    assert!(
        !restates_balances(&event(ExecKind::SnapshotOrder, 0, 200_000_000)),
        "a snapshot carries a filled size too — that is exactly the trap"
    );
    for quiet in [
        ExecKind::AckCanceled,
        ExecKind::AckFailed,
        ExecKind::ReportNew,
        ExecKind::ReportCanceled,
        ExecKind::PlaceNotSent,
        ExecKind::AmendNotSent,
        ExecKind::SnapshotEnd,
        ExecKind::StreamReady,
        ExecKind::StreamReset,
    ] {
        assert!(
            !restates_balances(&event(quiet, 0, 0)),
            "{quiet:?} moves nothing"
        );
    }
}

/// The dead man's switch chains: each call echoes the id the last answer carried. A stale id is
/// refused WITH the expected id in the body, so the refusal is itself the recovery — one reader
/// serves both, or a single stale id ends the chain and the venue cancels the book.
#[test]
fn the_heartbeat_id_is_read_from_a_refusal_as_readily_as_from_a_success() {
    assert_eq!(
        &*decode_heartbeat(r#"{"heartbeat_id":"1a2b"}"#).expect("a success carries the next id"),
        "1a2b"
    );
    assert_eq!(
        &*decode_heartbeat(r#"{"error_msg":"Invalid Heartbeat ID","heartbeat_id":"3c4d"}"#)
            .expect("a stale-id refusal carries the id the venue expected"),
        "3c4d"
    );
    assert!(
        decode_heartbeat(r#"{"error_msg":"Unauthorized"}"#).is_err(),
        "an answer with no id at all cannot continue the chain"
    );
}

/// `/clob-markets` does not publish this flag; it has its own endpoint, and the answer picks which
/// exchange contract every order on the token is signed against.
#[test]
fn the_neg_risk_read_is_a_bare_flag() {
    assert!(!decode_neg_risk(r#"{"neg_risk":false}"#).expect("the documented shape"));
    assert!(decode_neg_risk(r#"{"neg_risk":true}"#).expect("the documented shape"));
    assert!(
        !decode_neg_risk("{}").expect("an absent flag reads as the standard exchange"),
        "absence is the common case and must not be a parse failure"
    );
}
