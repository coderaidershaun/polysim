//! Execution wire codec goldens. With no testnet, these run against committed venue payloads are
//! the only place the normalisation layer is exercised at all before real money moves through it.
//!
//! Three judgements are pinned here because each is silent and expensive when wrong: the
//! `(x, X)` → `(ExecKind, VenueOrderStatus)` table, the error-code → [`RejectClass`] table, and the
//! client-id → [`Provenance`] test that decides whether the engine may touch an order.

use polysim::adapters::binance::exec::{
    CLIENT_ORDER_ID_LEN, ClockOffset, IgnoredReason, RecvWindow, RejectSubject, RequestSigner,
    ResponseContext, StreamEvent, WireError, classify_client_order_id, classify_error,
    decode_response, decode_stream_event, encode_request, format_client_order_id,
    parse_client_order_id,
};
use polysim::adapters::decode::DecimalFault;
use polysim::adapters::exec::{EngineIdentity, ExecRequest, TeTag};
use polysim::config::RunIdentity;
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::{
    AccountChunkKind, ExecEvent, ExecKind, Liquidity, OrderStyle, Provenance, RejectClass,
    VenueOrderStatus,
};
use polysim::secrets::Secret;
use polysim::time::TsUs;
use proptest::prelude::*;

use crate::fake_venue::{Delivery, FakeVenue, decimal, exec_events};

const NOW: TsUs = TsUs::from_micros(1_785_000_000_700_000);

/// A whole number of quote units as a 1e-8 mantissa. `usdt(118_000)` reads the way the price does;
/// the expanded mantissa does not.
const fn usdt(units: i64) -> Price {
    Price(units * FIXED_SCALE)
}

/// The two orders the committed fixtures name, and a third from an earlier run of this same engine.
const ORDER_A: ClientOrderId = ClientOrderId(0x6a64_f040_000c_0001);
const PRIOR_RUN_ORDER: ClientOrderId = ClientOrderId(0x6a64_e230_0004_0001);

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../fixtures/binance/exec/", $name, ".json"))
    };
}

fn venue() -> FakeVenue {
    FakeVenue::new()
}

fn decode(fixture: &str, venue: &FakeVenue) -> StreamEvent {
    decode_stream_event(fixture, &venue.decode_context(NOW)).expect("committed fixture decodes")
}

fn decode_exec(fixture: &str, venue: &FakeVenue) -> ExecEvent {
    match decode(fixture, venue) {
        StreamEvent::Exec(event) => event,
        other => panic!("expected an order event, got {other:?}"),
    }
}

fn respond(fixture: &str, request: ExecRequest, venue: &FakeVenue) -> Vec<ExecEvent> {
    decode_response(
        fixture,
        &ResponseContext {
            decode: venue.decode_context(NOW),
            request,
            recon_seq: 7,
        },
    )
    .expect("committed fixture decodes")
    .events
}

fn place_request(client_id: ClientOrderId) -> ExecRequest {
    ExecRequest::Place {
        instrument: InstrumentId(0),
        client_id,
        side: Side::Buy,
        price: usdt(118_000),
        qty: Qty(10_000),
        style: OrderStyle::PostOnly,
    }
}

mod execution_type_table {
    use super::*;

    /// Every `(x, X)` pair the venue can report about an order this engine places, pinned against
    /// the committed payload that carries it. A silent remap here reports a state that did not
    /// happen, and every layer downstream would agree with it.
    #[test]
    fn every_report_maps_to_one_kind_and_status() {
        let venue = venue();
        let cases: [(&str, ExecKind, VenueOrderStatus); 8] = [
            (
                fixture!("report_new"),
                ExecKind::ReportNew,
                VenueOrderStatus::New,
            ),
            (
                fixture!("report_trade_partially_filled"),
                ExecKind::ReportTrade,
                VenueOrderStatus::PartiallyFilled,
            ),
            (
                fixture!("report_trade_filled"),
                ExecKind::ReportTrade,
                VenueOrderStatus::Filled,
            ),
            (
                fixture!("report_canceled"),
                ExecKind::ReportCanceled,
                VenueOrderStatus::Canceled,
            ),
            // An amend reports REPLACED with the order still NEW — there is no amendment execution
            // type, and reading REPLACED as a cancellation would close a live order.
            (
                fixture!("report_replaced_by_amend"),
                ExecKind::ReportAmended,
                VenueOrderStatus::New,
            ),
            (
                fixture!("report_rejected_would_match_immediately"),
                ExecKind::ReportRejected,
                VenueOrderStatus::Rejected,
            ),
            (
                fixture!("report_expired"),
                ExecKind::ReportExpired,
                VenueOrderStatus::Expired,
            ),
            // Self-trade prevention: the venue took it away, but the status says the account
            // crossed ITSELF rather than a time-in-force running out.
            (
                fixture!("report_trade_prevention"),
                ExecKind::ReportExpired,
                VenueOrderStatus::ExpiredInMatch,
            ),
        ];
        for (payload, kind, status) in cases {
            let event = decode_exec(payload, &venue);
            assert_eq!(event.kind, kind, "execution type for {payload}");
            assert_eq!(event.status, Some(status), "order status for {payload}");
        }
    }

    /// `l`/`L` describe one execution and `z`/`Z` the order's running totals. The fold uses the
    /// cumulative pair, so a decoder that crossed them would double-count on every redelivery.
    #[test]
    fn last_execution_and_cumulative_totals_stay_distinct() {
        let event = decode_exec(fixture!("report_trade_filled"), &venue());
        assert_eq!(event.last_qty, Qty(6_000), "l — this execution alone");
        assert_eq!(event.last_price, usdt(118_000), "L");
        assert_eq!(event.cumulative_qty, Qty(10_000), "z — the order's total");
        assert_eq!(
            event.cumulative_quote, 1_180_000_000,
            "Z — 11.80 USDT at 1e-8"
        );
        assert_eq!(event.liquidity, Some(Liquidity::Maker));
    }
}

mod client_id_provenance {
    use super::*;

    /// The three-way test the engine acts on. A `Foreign` order is a human's, and the engine never
    /// cancels one — which is also why there is no cancel-everything command anywhere in the design.
    #[test]
    fn tag_and_run_nonce_decide_who_owns_an_order() {
        let venue = venue();
        let mine = decode_exec(fixture!("report_new"), &venue);
        assert_eq!(mine.provenance, Provenance::Mine);
        assert_eq!(mine.client_id, ORDER_A);

        let prior = decode_exec(fixture!("report_prior_run_order"), &venue);
        assert_eq!(prior.provenance, Provenance::PriorRun);
        assert_eq!(prior.client_id, PRIOR_RUN_ORDER);

        let foreign = decode_exec(fixture!("report_foreign_order"), &venue);
        assert_eq!(foreign.provenance, Provenance::Foreign);
        assert_eq!(
            foreign.client_id,
            ClientOrderId(0),
            "a foreign id is not in this engine's id space, so there is no honest id to report"
        );
    }

    /// A cancel and an amend name the ACTING REQUEST in `c` and the order they acted on in `C`.
    /// Keying on `c` addresses a venue-minted id no slot holds, and the event lands nowhere.
    #[test]
    fn cancel_and_amend_are_addressed_by_the_original_id() {
        let venue = venue();
        for payload in [
            fixture!("report_canceled"),
            fixture!("report_replaced_by_amend"),
        ] {
            let event = decode_exec(payload, &venue);
            assert_eq!(event.client_id, ORDER_A, "subject of {payload}");
            assert_eq!(event.provenance, Provenance::Mine);
        }
    }

    proptest! {
        /// Slot addressing and the generation check decode straight out of the string, so the round
        /// trip has to be exact for every id the engine can mint, under every tag.
        #[test]
        fn client_order_id_round_trips_exactly(raw in any::<u64>(), seed in any::<u32>()) {
            let client_id = ClientOrderId(raw);
            let identity = RunIdentity::new(&format!("strategy-{seed}"), &format!("te-{seed}"))
                .expect("generated ids are well formed");
            let tag = TeTag::of(&identity);
            let text = format_client_order_id(tag, client_id);
            prop_assert_eq!(text.len(), CLIENT_ORDER_ID_LEN);
            // Binance's own charset for `newClientOrderId`.
            prop_assert!(
                text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/')),
                "{text} must be a legal binance client order id"
            );
            let (parsed_tag, parsed_id) = parse_client_order_id(&text).expect("round trips");
            prop_assert_eq!(parsed_tag, tag);
            prop_assert_eq!(parsed_id, client_id);
        }
    }

    /// Anything that is not exactly this engine's format is foreign, and foreign orders are never
    /// touched. A loose parse would let an unrelated id alias onto the tag the engine acts on.
    #[test]
    fn malformed_ids_are_foreign_rather_than_a_near_match() {
        let identity = EngineIdentity {
            te_tag: TeTag::of(
                &RunIdentity::new("strat-micro-recorder", "te-binance-spot-btcusdt")
                    .expect("fixture ids are well formed"),
            ),
            run_nonce: 1_785_000_000,
        };
        let ours = format_client_order_id(identity.te_tag, ORDER_A);
        let cases = [
            "",
            "web_a1b2c3d4e5f6",
            "x-electra-abcdefg",
            // A leading `+` is accepted by `from_str_radix` and would alias onto a shorter tag.
            "pd-+ad60b6c-6a64f040000c0001",
            &ours[..ours.len() - 1],
            &format!("{ours}0"),
        ];
        for case in cases {
            assert_eq!(
                classify_client_order_id(case, identity).provenance,
                Provenance::Foreign,
                "{case:?} must not be read as this engine's order"
            );
        }
        assert_eq!(
            classify_client_order_id(&ours, identity).provenance,
            Provenance::Mine
        );
    }
}

mod error_classification {
    use super::*;

    /// The whole table, with the two that cost money called out. `-2011` is never `Gone`: it also
    /// means FILLED, and reaping on the other reading loses a fill the account was really paid for.
    #[test]
    fn every_code_maps_to_the_class_the_engine_may_act_on() {
        use RejectSubject::{Amendment, Cancellation, Placement, StatusQuery};
        let cases: [(i32, &str, RejectSubject, RejectClass); 17] = [
            (
                -2010,
                "Order would immediately match and take.",
                Placement,
                RejectClass::Refused,
            ),
            (
                -2010,
                "Account has insufficient balance for requested action.",
                Placement,
                RejectClass::Fatal,
            ),
            // An unrecognised -2010 halts rather than guessing which of the two it is.
            (-2010, "Some new condition.", Placement, RejectClass::Fatal),
            (
                -2011,
                "Unknown order sent.",
                Cancellation,
                RejectClass::Ambiguous,
            ),
            (
                -2011,
                "Unknown order sent.",
                Amendment,
                RejectClass::Ambiguous,
            ),
            // Definitive ONLY from the query that exists to answer it.
            (
                -2013,
                "Order does not exist.",
                StatusQuery,
                RejectClass::Gone,
            ),
            (
                -2013,
                "Order does not exist.",
                Cancellation,
                RejectClass::Ambiguous,
            ),
            (
                -1021,
                "Timestamp outside recvWindow.",
                Placement,
                RejectClass::StillLive,
            ),
            (
                -1003,
                "Too many requests.",
                Placement,
                RejectClass::StillLive,
            ),
            // "Execution status unknown — the request MAY have gone through." Read as StillLive a
            // placement closes its slot as never-rested, the side frees, and the next spin puts a
            // second order beside one that may be resting.
            (
                -1006,
                "An unexpected response was received from the message bus. Execution status unknown.",
                Placement,
                RejectClass::Ambiguous,
            ),
            (
                -1007,
                "Timeout waiting for response from backend server. Send status unknown; execution status unknown.",
                Placement,
                RejectClass::Ambiguous,
            ),
            // The SAME two codes on a cancel stay StillLive, and the asymmetry is the point: a
            // cancel re-sent after an unknown outcome costs one routine -2011, while a placement
            // re-derived after one costs a second live order. Probing instead would hold the
            // cancel latch shut on an order that is still resting.
            (
                -1006,
                "An unexpected response was received from the message bus. Execution status unknown.",
                Cancellation,
                RejectClass::StillLive,
            ),
            (
                -1007,
                "Timeout waiting for response from backend server. Send status unknown; execution status unknown.",
                Cancellation,
                RejectClass::StillLive,
            ),
            // The venue's ONLY statement about an amend budget. Unclassified it fell to Fatal, so
            // the first order to exhaust its amends would halt the run.
            (
                -2038,
                "Filter failure: MAX_NUM_ORDER_AMENDS",
                Amendment,
                RejectClass::StillLive,
            ),
            (
                -1013,
                "Filter failure: MIN_NOTIONAL",
                Placement,
                RejectClass::Fatal,
            ),
            (-1022, "Signature not valid.", Placement, RejectClass::Fatal),
            (
                -2015,
                "Invalid API-key, IP, or permissions.",
                Placement,
                RejectClass::Fatal,
            ),
        ];
        for (code, message, subject, class) in cases {
            assert_eq!(
                classify_error(code, message, subject),
                class,
                "code {code} answering a {subject:?}"
            );
        }
    }

    proptest! {
        /// A code nobody handled is FATAL, never a retry. Retrying an unhandled failure against an
        /// order endpoint is how one bug becomes a burst of duplicate orders.
        #[test]
        fn unhandled_codes_halt_rather_than_retry(code in -9999i32..0) {
            const HANDLED: [i32; 12] =
                [-1003, -1006, -1007, -1013, -1015, -1021, -1022, -2010, -2011, -2013, -2014, -2038];
            prop_assume!(!HANDLED.contains(&code) && code != -2015);
            prop_assert_eq!(
                classify_error(code, "something new", RejectSubject::Placement),
                RejectClass::Fatal
            );
        }
    }

    /// An error response names no order at all, so the request it answers is the only thing that
    /// says which client id it is about.
    #[test]
    fn an_error_response_is_attributed_from_the_request() {
        let venue = venue();
        let events = respond(
            fixture!("error_2011_cancel_rejected"),
            ExecRequest::Cancel {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
            },
            &venue,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ExecKind::AckFailed);
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].reject, Some(RejectClass::Ambiguous));
        assert_eq!(events[0].reject_code, -2011);
        // A cancel rejection is not about an amendment at all, so it must not claim a spent budget
        // on the way past.
        assert_eq!(events[0].amends_remaining, ExecEvent::AMENDS_UNKNOWN);
    }
}

/// A request whose answer never came leaves one of three things behind, and only ONE of them is
/// worth a connection. Pinned on [`ExecRequest`] rather than on the driver's timer arm, which needs
/// a socket and which `whole_frames` declines to expose — the classification is the part that
/// decides, and it is pure.
mod request_timeouts {
    use super::*;
    use polysim::adapters::exec::TimeoutFallout;

    /// A request that names an order leaves it in doubt, to be reconciled and never retried.
    ///
    /// **The two that name no order are NOT the same case**, and spelling both "no instrument" is
    /// what made a single slow open-orders answer tear the session down — reconnect, `StreamReset`,
    /// full resync, requote — while reporting it as an unanswered subscribe.
    ///
    /// Nothing is blocked on an open-orders answer and nothing is left in doubt by its absence: the
    /// hot side's silence detector asks again on its own cadence, and a `SnapshotEnd` that never
    /// arrives retires no slot. A subscribe is the opposite — no account stream means no fills, and
    /// quoting without one is trading blind.
    #[test]
    fn only_the_subscribe_costs_the_connection() {
        let cases = [
            (
                place_request(ORDER_A),
                TimeoutFallout::OrderInDoubt {
                    instrument: InstrumentId(0),
                    client_id: ORDER_A,
                },
            ),
            (
                ExecRequest::OpenOrders {
                    instrument: InstrumentId(0),
                },
                TimeoutFallout::ReadAbandoned,
            ),
            (
                ExecRequest::SubscribeUserStream,
                TimeoutFallout::StreamUnusable,
            ),
        ];
        for (request, fallout) in cases {
            assert_eq!(
                request.timeout_fallout(),
                fallout,
                "unanswered {request:?} — a late read is waited out, not answered with a reconnect"
            );
        }
    }
}

/// Binance publishes NO running amend count — not on the amend response, not on the `REPLACED`
/// execution report, not on any order-bearing payload (checked against `binance-spot-api-docs`
/// `web-socket-api.md` / `user-data-stream.md`, 2026-07-28; `order.amendments` is a separate paid
/// query, not a field). So the only honest thing almost every event can say about the budget is
/// NOTHING — and zero does not mean nothing. A consumer reads zero as a definitive ceiling and
/// retires the amend primitive for that order for good, which is the strongest claim available
/// rather than the absence of one.
///
/// Exactly one payload earns it, and the CODE does not identify it. `-2038 ORDER_AMEND_REJECTED`
/// is a many-messages-one-code error documented in the same `errors.md` table as `-2010`, covering
/// amends refused for reasons that have nothing to do with the limit.
mod amend_budget {
    use super::*;

    /// An amend acknowledgement makes no claim about the budget, whichever transport carries it.
    /// Reading one as "exhausted" retires the amend primitive after a single use, and every later
    /// size reduction unnecessarily loses queue priority.
    #[test]
    fn an_amend_acknowledgement_claims_nothing_about_the_budget() {
        let venue = venue();
        let acked = respond(
            fixture!("ack_order_amend_keep_priority"),
            ExecRequest::AmendQty {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
                qty: Qty(6_000),
            },
            &venue,
        );
        assert_eq!(acked[0].kind, ExecKind::AckAmended);
        assert_eq!(acked[0].amends_remaining, ExecEvent::AMENDS_UNKNOWN);

        let reported = decode_exec(fixture!("report_replaced_by_amend"), &venue);
        assert_eq!(reported.kind, ExecKind::ReportAmended);
        assert_eq!(reported.amends_remaining, ExecEvent::AMENDS_UNKNOWN);
    }

    fn amend_refusal(fixture: &str, venue: &FakeVenue) -> ExecEvent {
        let events = respond(
            fixture,
            ExecRequest::AmendQty {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
                qty: Qty(6_000),
            },
            venue,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].kind, ExecKind::AckFailed);
        assert_eq!(events[0].reject_code, -2038);
        // Every -2038 says the same thing about the ORDER: nothing happened to it. Only the budget
        // claim below varies, which is why the class cannot be what discriminates.
        assert_eq!(
            events[0].reject,
            Some(RejectClass::StillLive),
            "a refused amend changes nothing about the order"
        );
        events[0]
    }

    /// The MESSAGE names the filter, and it is the only payload entitled to claim an exhausted
    /// budget. Left unclassified — as `-2038` was — the code falls through to `Fatal` and halts the
    /// run the first time an order legitimately runs out of amends.
    #[test]
    fn only_the_amend_limit_message_claims_an_exhausted_budget() {
        let venue = venue();
        assert_eq!(
            amend_refusal(fixture!("error_2038_amend_budget_spent"), &venue).amends_remaining,
            0,
            "zero is EXHAUSTED, and this is the one message entitled to say it"
        );
    }

    /// The same code, a different condition, and the reason the code alone cannot be the test.
    /// `-2038` is a many-messages-one-code error — `errors.md` documents it in the same table as
    /// `-2010`, which is exactly why `insufficient_balance_or_cross` reads the message too.
    ///
    /// Read as "budget spent", this transient refusal would retire the amend primitive for the order
    /// permanently: every later shrink would lose queue priority.
    #[test]
    fn another_amend_refusal_on_the_same_code_claims_nothing() {
        let venue = venue();
        assert_eq!(
            amend_refusal(fixture!("error_2038_amend_quantity_increase"), &venue).amends_remaining,
            ExecEvent::AMENDS_UNKNOWN,
            "an unrecognised refusal makes NO claim — the loud direction, since a budget that really \
             is spent keeps answering -2038 and those count toward the reject-streak halt"
        );
    }

    /// Only a rejection OF AN AMEND may speak about an amend budget. Filter-failure messages are a
    /// table of their own in `errors.md` and belong to no single code, so the same string arrives
    /// under `-1013 FILTER_FAILURE` too — where it is `Fatal`, halts the run, and is in no position
    /// to be making claims about what an order may still do.
    #[test]
    fn the_same_filter_message_under_another_code_claims_nothing() {
        let venue = venue();
        let events = respond(
            fixture!("error_1013_amend_filter_failure"),
            ExecRequest::AmendQty {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
                qty: Qty(6_000),
            },
            &venue,
        );
        assert_eq!(events[0].reject, Some(RejectClass::Fatal));
        assert_eq!(events[0].amends_remaining, ExecEvent::AMENDS_UNKNOWN);
    }
}

mod acks_and_reconciliation {
    use super::*;

    #[test]
    fn a_place_ack_adopts_the_order() {
        let venue = venue();
        let events = respond(fixture!("ack_order_place"), place_request(ORDER_A), &venue);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ExecKind::AckPlaced);
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].status, Some(VenueOrderStatus::New));
        assert_eq!(events[0].price, usdt(118_000));
    }

    /// A cancel ack returns the acting request's own `clientOrderId` and the order's id as
    /// `origClientOrderId`. Reading the former addresses no slot at all.
    #[test]
    fn a_cancel_ack_is_addressed_by_the_original_id() {
        let venue = venue();
        let events = respond(
            fixture!("ack_order_cancel"),
            ExecRequest::Cancel {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
            },
            &venue,
        );
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].kind, ExecKind::AckCanceled);
    }

    /// The amend response spells the quantity `qty` where every other response says `origQty`, and
    /// the cumulative quote with one `m` rather than two. A reader reusing the place-response shape
    /// gets null for both.
    #[test]
    fn an_amend_ack_reads_the_venues_other_spellings() {
        let venue = venue();
        let events = respond(
            fixture!("ack_order_amend_keep_priority"),
            ExecRequest::AmendQty {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
                qty: Qty(6_000),
            },
            &venue,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ExecKind::AckAmended);
        assert_eq!(
            events[0].client_id, ORDER_A,
            "addressed by origClientOrderId"
        );
        assert_eq!(events[0].qty, Qty(6_000), "read from `qty`, not `origQty`");
    }

    /// The definitive answer to an ambiguous cancel. `-2011` said "unknown order"; the query says
    /// it filled, and the cumulative totals are what the ledger folds.
    #[test]
    fn an_order_status_answer_carries_the_venues_absolute_totals() {
        let venue = venue();
        let events = respond(
            fixture!("ack_order_status_filled"),
            ExecRequest::OrderStatus {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
            },
            &venue,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ExecKind::SnapshotOrder);
        assert_eq!(events[0].status, Some(VenueOrderStatus::Filled));
        assert_eq!(events[0].cumulative_qty, Qty(10_000));
        assert_eq!(
            events[0].cumulative_quote, 1_180_000_000,
            "11.80 USDT at 1e-8"
        );
        assert_eq!(
            events[0].recon_seq, 7,
            "the pass that asked is carried back"
        );
    }

    /// A pass is N orders closed by exactly one end marker, and a human's resting order is reported
    /// with its own provenance rather than filtered out — the sweep has to know it is there.
    #[test]
    fn an_open_orders_pass_ends_with_exactly_one_marker() {
        let venue = venue();
        let events = respond(
            fixture!("ack_open_orders"),
            ExecRequest::OpenOrders {
                instrument: InstrumentId(0),
            },
            &venue,
        );
        assert_eq!(events.len(), 3, "two orders plus the end marker");
        assert_eq!(events[0].kind, ExecKind::SnapshotOrder);
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].provenance, Provenance::Mine);
        assert_eq!(events[1].provenance, Provenance::Foreign);
        assert_eq!(events[2].kind, ExecKind::SnapshotEnd);
        assert_eq!(events[2].recon_seq, 7);
    }

    /// An empty answer still ends the pass. It is the answer that says everything the engine
    /// believes is live is gone, so swallowing it would leave phantom orders on the books forever.
    #[test]
    fn an_empty_open_orders_pass_still_ends() {
        let venue = venue();
        let events = respond(
            fixture!("ack_open_orders_empty"),
            ExecRequest::OpenOrders {
                instrument: InstrumentId(0),
            },
            &venue,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ExecKind::SnapshotEnd);
    }

    /// A failed reconciliation emits NOTHING. No end marker means no sweep, so the engine keeps
    /// believing what it already believed rather than declaring live orders gone on a request that
    /// never ran.
    #[test]
    fn a_failed_reconciliation_never_ends_the_pass() {
        let venue = venue();
        let events = respond(
            fixture!("error_1021_timestamp_outside_recv_window"),
            ExecRequest::OpenOrders {
                instrument: InstrumentId(0),
            },
            &venue,
        );
        assert!(events.is_empty(), "no marker, no sweep");
    }
}

mod account_and_edge_drops {
    use super::*;

    /// The account stream is account-wide. `ExecEvent::instrument` promises a configured
    /// instrument, so a symbol this engine does not track is dropped here rather than given an id.
    #[test]
    fn an_untracked_symbol_is_dropped_at_the_edge() {
        assert_eq!(
            decode(fixture!("report_untracked_symbol"), &venue()),
            StreamEvent::Ignored(IgnoredReason::UntrackedSymbol)
        );
    }

    /// `userDataStream.subscribe` wraps every event. The legacy listen-key stream delivered the
    /// inner object bare, and a decoder written against that shape reads null for every field and
    /// reports NOTHING — no error, no event. This must fail loudly instead.
    #[test]
    fn the_legacy_bare_envelope_is_refused_rather_than_read_as_nulls() {
        let error = decode_stream_event(
            fixture!("report_new_legacy_bare"),
            &venue().decode_context(NOW),
        )
        .expect_err("an unwrapped event must not decode");
        assert!(
            matches!(error, WireError::Decode(DecimalFault::Json { .. })),
            "got {error}"
        );
    }

    /// A fee is charged in the asset RECEIVED. An asset no configured instrument names books to
    /// `UNKNOWN` rather than being misattributed to one the engine does trade — which is exactly
    /// what a BNB-discount account produces on every fill.
    #[test]
    fn commission_resolves_to_an_asset_index_and_bnb_is_unknown() {
        let venue = venue();
        let base_fee = decode_exec(fixture!("report_trade_filled"), &venue);
        assert_eq!(base_fee.commission, 6, "0.00000006 BTC at 1e-8");
        assert_eq!(
            base_fee.commission_asset,
            venue.assets().id("BTC"),
            "a buy is charged in the asset received"
        );
        assert_ne!(base_fee.commission_asset, AssetId::UNKNOWN);

        let discounted = decode_exec(
            fixture!("report_trade_filled_unknown_commission_asset"),
            &venue,
        );
        assert_eq!(discounted.commission_asset, AssetId::UNKNOWN);
    }

    /// `outboundAccountPosition` names only what changed, so it OVERWRITES rather than replaces.
    /// Unregistered assets cross rather than being filtered, because a balance dropped at the edge
    /// is one nobody can later explain.
    #[test]
    fn account_balances_chunk_as_an_overwriting_update() {
        let venue = venue();
        let StreamEvent::Account(chunks) = decode(fixture!("account_position"), &venue) else {
            panic!("expected balances");
        };
        assert_eq!(chunks.len(), 1, "four assets fit one chunk");
        assert_eq!(chunks[0].kind, AccountChunkKind::Update);
        assert!(chunks[0].is_last_chunk);
        assert_eq!(chunks[0].len, 4);

        let balances = chunks[0].active_balances();
        assert_eq!(balances[0].asset, venue.assets().id("BTC"));
        assert_eq!(balances[0].free, 135_871);
        assert_eq!(balances[0].locked, 10_000);
        assert_eq!(balances[1].asset, venue.assets().id("USDT"));
        assert_eq!(balances[1].free, 17_114_535_000, "171.14535 USDT at 1e-8");
        assert_eq!(
            (balances[2].asset, balances[3].asset),
            (AssetId::UNKNOWN, AssetId::UNKNOWN),
            "BNB and the dust asset are counted, not misattributed"
        );
    }

    /// `balanceUpdate` carries a DELTA, and a delta stream that loses one frame is permanently
    /// wrong about money. Its value is never read — the event is only a trigger for an absolute
    /// snapshot. The negative fixture also proves the decimal parser is never handed the `-`.
    #[test]
    fn a_balance_delta_is_a_trigger_and_never_a_value() {
        assert_eq!(
            decode(fixture!("balance_update_negative"), &venue()),
            StreamEvent::BalanceChanged
        );
    }
}

mod encoders {
    use super::*;

    /// Reads the params back through the real signer, which is the only thing that renders them —
    /// so this pins what actually reaches the wire, not an intermediate the driver might reshape.
    fn signed_params(
        request: ExecRequest,
        venue: &FakeVenue,
    ) -> (&'static str, Vec<(String, String)>) {
        let encoded = encode_request(request, &venue.encode_context()).expect("encodes");
        let signer = RequestSigner::new(&Secret::new("test-secret"));
        // A stamp is mintable only through a clock offset, which is the point: nothing reaches the
        // venue with an uncorrected host clock on it.
        let signed = signer
            .sign(
                encoded.params.set_recv_window(RecvWindow::DEFAULT),
                ClockOffset::NONE.stamp(NOW),
            )
            .expect("params are url-safe");
        let params = signed
            .signed_params()
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        (encoded.method, params)
    }

    fn value<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.as_str())
    }

    /// A post-only order is `LIMIT_MAKER` and carries NO `timeInForce` — the venue lists only price
    /// and quantity as mandatory for it, and a post-only order rests or dies.
    #[test]
    fn a_post_only_placement_is_limit_maker_with_no_time_in_force() {
        let venue = venue();
        let (method, params) = signed_params(place_request(ORDER_A), &venue);
        assert_eq!(method, "order.place");
        assert_eq!(
            value(&params, "symbol"),
            Some("BTCUSDT"),
            "upper case on the wire"
        );
        assert_eq!(value(&params, "type"), Some("LIMIT_MAKER"));
        assert_eq!(value(&params, "timeInForce"), None);
        // Defaulted, `LIMIT_MAKER` answers `ACK` — no price, quantity, status or side — and every
        // placement's own acknowledgement then fails to decode, leaving the order to resolve on its
        // stream report or its timeout. The default differs by order TYPE, so this cannot be left
        // to the venue.
        assert_eq!(value(&params, "newOrderRespType"), Some("RESULT"));
        assert_eq!(value(&params, "side"), Some("BUY"));
        assert_eq!(value(&params, "price"), Some("118000"));
        assert_eq!(value(&params, "quantity"), Some("0.0001"));
        assert_eq!(
            value(&params, "newClientOrderId"),
            Some(format_client_order_id(venue.identity().te_tag, ORDER_A).as_str())
        );
    }

    #[test]
    fn an_order_that_may_cross_is_a_limit_with_a_time_in_force() {
        let venue = venue();
        let request = match place_request(ORDER_A) {
            ExecRequest::Place {
                instrument,
                client_id,
                side,
                price,
                qty,
                ..
            } => ExecRequest::Place {
                instrument,
                client_id,
                side,
                price,
                qty,
                style: OrderStyle::Immediate,
            },
            other => other,
        };
        let (_, params) = signed_params(request, &venue);
        assert_eq!(value(&params, "type"), Some("LIMIT"));
        assert_eq!(value(&params, "timeInForce"), Some("GTC"));
    }

    /// The amend ECHOES the id back. Omitting `newClientOrderId` makes the venue mint a random one,
    /// and the order would then be known to the venue by a name no slot holds — every later cancel
    /// by client id would answer -2011, which reads identically to "it filled".
    #[test]
    fn an_amend_keeps_the_order_addressable_by_echoing_its_id() {
        let venue = venue();
        let (method, params) = signed_params(
            ExecRequest::AmendQty {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
                qty: Qty(6_000),
            },
            &venue,
        );
        assert_eq!(method, "order.amend.keepPriority");
        let ours = format_client_order_id(venue.identity().te_tag, ORDER_A);
        assert_eq!(value(&params, "origClientOrderId"), Some(ours.as_str()));
        assert_eq!(value(&params, "newClientOrderId"), Some(ours.as_str()));
        assert_eq!(value(&params, "newQty"), Some("0.00006"));
    }

    /// A wrong method fails silently rather than loudly: `userDataStream.subscribe` needs an
    /// Ed25519 session logon this HMAC account cannot perform, so picking the plain form over the
    /// signature form leaves the account stream simply absent.
    #[test]
    fn each_request_names_its_own_venue_method() {
        let venue = venue();
        let cases = [
            (
                ExecRequest::SubscribeUserStream,
                "userDataStream.subscribe.signature",
            ),
            (
                ExecRequest::OpenOrders {
                    instrument: InstrumentId(0),
                },
                "openOrders.status",
            ),
            (
                ExecRequest::OrderStatus {
                    instrument: InstrumentId(0),
                    client_id: ORDER_A,
                },
                "order.status",
            ),
        ];
        for (request, method) in cases {
            assert_eq!(signed_params(request, &venue).0, method, "{request:?}");
        }
    }

    proptest! {
        /// The encoded price has to be the exact mantissa the engine decided on. A float round trip
        /// here would send an order at a price nobody chose.
        #[test]
        fn every_price_round_trips_through_the_wire_form(mantissa in 1i64..1_000_000_000_000_000i64) {
            let venue = venue();
            let (_, params) = signed_params(
                ExecRequest::Place {
                    instrument: InstrumentId(0),
                    client_id: ORDER_A,
                    side: Side::Buy,
                    price: Price(mantissa),
                    qty: Qty(mantissa),
                    style: OrderStyle::PostOnly,
                },
                &venue,
            );
            let price = value(&params, "price").expect("a price is always sent");
            prop_assert_eq!(Price::parse_decimal(price).expect("wire form parses"), Price(mantissa));
            let quantity = value(&params, "quantity").expect("a quantity is always sent");
            prop_assert_eq!(Qty::parse_decimal(quantity).expect("wire form parses"), Qty(mantissa));
        }
    }
}

mod fake_venue_convergence {
    use super::*;

    /// Every event the fake venue emits was produced by the real decoders, so a scenario driving it
    /// exercises the classification layer rather than a hand-built copy of what it should say.
    #[test]
    fn a_placement_and_its_fill_come_back_as_decoded_events() {
        let mut venue = venue();
        let placed = venue.submit(place_request(ORDER_A), NOW);
        let events = exec_events(&placed);
        assert_eq!(events.len(), 2, "the ack and the report");
        assert_eq!(events[0].kind, ExecKind::AckPlaced);
        assert_eq!(events[1].kind, ExecKind::ReportNew);
        assert!(venue.is_resting(ORDER_A));

        let filled = exec_events(&venue.fill(ORDER_A, Qty(4_000), NOW));
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0].kind, ExecKind::ReportTrade);
        assert_eq!(filled[0].status, Some(VenueOrderStatus::PartiallyFilled));
        assert_eq!(filled[0].cumulative_qty, Qty(4_000));

        let rest = exec_events(&venue.fill(ORDER_A, Qty(6_000), NOW));
        assert_eq!(rest[0].status, Some(VenueOrderStatus::Filled));
        assert_eq!(
            rest[0].cumulative_qty,
            Qty(10_000),
            "cumulative, so a duplicate folds to nothing"
        );
        assert!(!venue.is_resting(ORDER_A), "a filled order stops resting");
    }

    /// The ack and its report race on the network, and the venue's story about the order is the
    /// same whichever lands first. What varies is only how many messages arrive.
    #[test]
    fn every_delivery_order_tells_the_same_story() {
        let deliveries = [
            (Delivery::AckFirst, 2),
            (Delivery::ReportFirst, 2),
            (Delivery::AckOnly, 1),
            (Delivery::ReportOnly, 1),
            (Delivery::Ambiguous, 0),
            (Delivery::Duplicated, 4),
            (Delivery::Reordered, 2),
        ];
        for (delivery, expected) in deliveries {
            let mut venue = venue();
            venue.set_delivery(delivery);
            let events = exec_events(&venue.submit(place_request(ORDER_A), NOW));
            assert_eq!(events.len(), expected, "{delivery:?} message count");
            for event in &events {
                assert_eq!(event.client_id, ORDER_A, "{delivery:?} addresses one order");
                assert_eq!(event.provenance, Provenance::Mine);
            }
            assert!(
                venue.is_resting(ORDER_A),
                "{delivery:?} — the venue's own state never depends on what we were told"
            );
        }
    }

    /// A post-only quote that would cross is rejected, not filled — and both the request answer and
    /// the account report say so.
    #[test]
    fn a_crossing_post_only_quote_is_rejected_by_the_model() {
        let mut venue = venue();
        venue.set_book(usdt(117_000), usdt(117_500));
        let events = exec_events(&venue.submit(place_request(ORDER_A), NOW));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, ExecKind::AckFailed);
        assert_eq!(events[0].reject, Some(RejectClass::Refused));
        assert_eq!(events[1].kind, ExecKind::ReportRejected);
        assert_eq!(events[1].reject, Some(RejectClass::Refused));
        assert_eq!(venue.resting_count(), 0, "a rejected order never rested");
    }

    /// The reconcile pass reflects exactly what the model holds, which is what makes it usable as
    /// the answer to an ambiguous cancel.
    #[test]
    fn open_orders_reports_what_the_model_is_actually_holding() {
        let mut venue = venue();
        venue.submit(place_request(ORDER_A), NOW);
        venue.fill(ORDER_A, Qty(4_000), NOW);

        let events = exec_events(&venue.submit(
            ExecRequest::OpenOrders {
                instrument: InstrumentId(0),
            },
            NOW,
        ));
        assert_eq!(events.len(), 2, "one resting order plus the end marker");
        assert_eq!(events[0].client_id, ORDER_A);
        assert_eq!(events[0].cumulative_qty, Qty(4_000));
        assert_eq!(events[1].kind, ExecKind::SnapshotEnd);
    }

    /// Balances reach the hot path as chunks through the same decoder, so a scenario can watch money
    /// move without a second way of building them. The model names the instrument it trades, which
    /// is what a scenario addresses its quotes at.
    #[test]
    fn the_venue_publishes_balances_as_account_chunks() {
        let venue = venue();
        assert_eq!(venue.instrument(), InstrumentId(0));

        let messages = venue.account_update(NOW);
        let chunks: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                polysim::msg::inbound::InboundMessage::Account(chunk) => Some(*chunk),
                _ => None,
            })
            .collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].kind,
            AccountChunkKind::Update,
            "the venue names only what changed, so it overwrites rather than replaces"
        );
        assert!(chunks[0].is_last_chunk);
        assert_eq!(
            chunks[0].active_balances()[0].asset,
            venue.assets().id("BTC")
        );
    }

    proptest! {
        /// The exact decimal form the model writes into every payload it patches, so a rounding
        /// change would fail here rather than as a mispriced order in a scenario.
        #[test]
        fn the_models_decimal_form_is_exact(mantissa in 0i64..1_000_000_000_000_000i64) {
            prop_assert_eq!(
                Price::parse_decimal(&decimal(mantissa)).expect("parses"),
                Price(mantissa)
            );
        }
    }
}

/// The REST reconcilers reach the same decoders by a different route. That is the whole point of
/// them living in the codec: two implementations of the provenance test would disagree silently, and
/// the disagreement is either the engine cancelling a human's order or abandoning one of its own.
mod rest_reconcilers {
    use super::*;
    use polysim::adapters::binance::exec::decode_order_record;
    use polysim::adapters::binance::rest::OrderRecord;

    fn open_order_record(client_order_id: &str) -> OrderRecord {
        serde_json::from_str(&format!(
            r#"{{"symbol":"BTCUSDT","orderId":12510053279,"clientOrderId":"{client_order_id}",
                "price":"118000.00000000","origQty":"0.00010000","executedQty":"0.00004000",
                "cummulativeQuoteQty":"4.72000000","status":"PARTIALLY_FILLED","timeInForce":"GTC",
                "type":"LIMIT_MAKER","side":"BUY","time":1785000000600,"updateTime":1785000000657}}"#
        ))
        .expect("record parses")
    }

    /// A sweep row decodes to the same shape the WS answer does, addressed by the same id, with the
    /// venue's absolute totals intact for the fold.
    #[test]
    fn an_open_orders_row_decodes_like_its_websocket_twin() {
        let venue = venue();
        let ours = format_client_order_id(venue.identity().te_tag, ORDER_A);
        let event = decode_order_record(
            &open_order_record(&ours),
            ExecKind::SnapshotOrder,
            9,
            &venue.decode_context(NOW),
        )
        .expect("decodes")
        .expect("BTCUSDT is configured");

        assert_eq!(event.client_id, ORDER_A);
        assert_eq!(event.provenance, Provenance::Mine);
        assert_eq!(event.kind, ExecKind::SnapshotOrder);
        assert_eq!(event.status, Some(VenueOrderStatus::PartiallyFilled));
        assert_eq!(event.cumulative_qty, Qty(4_000));
        assert_eq!(event.cumulative_quote, 472_000_000, "4.72 USDT at 1e-8");
        assert_eq!(event.recon_seq, 9);
    }

    proptest! {
        /// The ONE property the single-implementation ruling exists to protect: whichever route a
        /// client id arrives by, it classifies the same. `Foreign` is never cancelled and `Mine`
        /// always is, so a split verdict is either a human's order pulled or one of ours abandoned.
        #[test]
        fn provenance_never_depends_on_which_transport_carried_the_id(
            raw in any::<u64>(),
            use_ours in any::<bool>(),
        ) {
            let venue = venue();
            let identity = venue.identity();
            let text = match use_ours {
                true => format_client_order_id(identity.te_tag, ClientOrderId(raw)),
                false => format!("web_{raw:016x}"),
            };
            let rest = decode_order_record(
                &open_order_record(&text),
                ExecKind::SnapshotOrder,
                0,
                &venue.decode_context(NOW),
            )
            .expect("decodes")
            .expect("configured");
            let direct = classify_client_order_id(&text, identity);
            prop_assert_eq!(rest.provenance, direct.provenance);
            prop_assert_eq!(rest.client_id, direct.client_id);
        }
    }

    /// The sweep is account-wide, so a symbol this engine does not track is an ordinary outcome and
    /// not an error — but it must never be given an instrument id it has no right to.
    #[test]
    fn an_untracked_symbol_yields_no_event_rather_than_a_wrong_instrument() {
        let venue = venue();
        let mut record = open_order_record("web_a1b2c3d4e5f6");
        record.symbol = "ETHUSDT".into();
        let decoded = decode_order_record(
            &record,
            ExecKind::SnapshotOrder,
            0,
            &venue.decode_context(NOW),
        )
        .expect("an untracked symbol is not a failure");
        assert!(decoded.is_none());
    }
}

/// Whole frames: committed fixture JSON through the real decoders, pinning the judgements the
/// driver's frame router depends on but cannot itself expose — that router is `pub(super)` inside
/// the actor, and opening it for a test would be worse than not testing it. What is provable
/// without exposing anything is that the two correlation paths are separate BY TYPE rather than by
/// the driver remembering to keep them apart.
mod whole_frames {
    use super::*;
    use polysim::adapters::binance::exec::DecodedResponse;

    fn decoded(fixture: &str, request: ExecRequest, venue: &FakeVenue) -> DecodedResponse {
        decode_response(
            fixture,
            &ResponseContext {
                decode: venue.decode_context(NOW),
                request,
                recon_seq: 0,
            },
        )
        .expect("committed fixture decodes")
    }

    /// The two correlation paths are separate BY TYPE, which is stronger than separate by
    /// convention. A response correlates by numeric request id; a stream event carries none and is
    /// addressed by client id — and `decode_stream_event` takes no `ExecRequest` at all, so it
    /// *cannot* be routed by a request key even by mistake.
    ///
    /// This is the seam most likely to be "simplified" into one map by a later reader, so both
    /// directions are pinned: neither decoder accepts the other's frame.
    #[test]
    fn a_response_and_a_stream_event_can_never_be_routed_by_each_others_key() {
        let venue = venue();

        // A response fed to the stream decoder: no `event` wrapper, so it fails rather than
        // reading nulls for every field.
        let as_stream =
            decode_stream_event(fixture!("ack_order_place"), &venue.decode_context(NOW));
        assert!(
            matches!(as_stream, Err(WireError::Decode(DecimalFault::Json { .. }))),
            "a request answer is not a stream event, got {as_stream:?}"
        );

        // A stream event fed to the response decoder: no `result` and no `error`.
        let as_response = decode_response(
            fixture!("report_new"),
            &ResponseContext {
                decode: venue.decode_context(NOW),
                request: place_request(ORDER_A),
                recon_seq: 0,
            },
        );
        assert!(
            matches!(as_response, Err(WireError::EmptyResponse)),
            "a stream event is not a request answer, got {as_response:?}"
        );

        // Fed correctly and concurrently, both describe the SAME order by the same client id, and
        // the response's `"id"` correlation key appears nowhere in what crosses the boundary.
        let from_stream = decode_exec(fixture!("report_new"), &venue);
        let from_response = decoded(fixture!("ack_order_place"), place_request(ORDER_A), &venue);
        assert_eq!(from_stream.client_id, ORDER_A);
        assert_eq!(from_response.events[0].client_id, ORDER_A);
    }

    /// The split that makes a post-only kill switch possible at all. Three venue answers read as one
    /// concept — "the order is not there" — and mean three different things to a counter, so the EDGE
    /// decides which is which and `hot/` reads only the class.
    ///
    /// Drives the real decoders rather than a copy of their rule: a copy keeps passing after the
    /// shipped one drifts, which is the failure this test exists to prevent.
    #[test]
    fn a_post_only_cross_is_a_different_class_from_every_other_gone() {
        let venue = venue();

        // Off the account stream, where a REASON string arrives and no numeric code does.
        let streamed = decode_exec(fixture!("report_rejected_would_match_immediately"), &venue);
        assert_eq!(streamed.reject, Some(RejectClass::Refused));
        assert_eq!(
            streamed.reject_code, 0,
            "the stream reports a reason, not a code"
        );

        // An expiry is a FOURTH reading of "not there". Counting one as routine parks the engine
        // exactly when the market is interesting.
        for payload in [
            fixture!("report_expired"),
            fixture!("report_trade_prevention"),
        ] {
            assert_eq!(
                decode_exec(payload, &venue).reject,
                None,
                "an expiry is the venue taking the order away, not a rejection"
            );
        }

        // Off an ack, where the code and the message together discriminate.
        let acked = decoded(
            fixture!("error_2010_would_match_immediately"),
            place_request(ORDER_A),
            &venue,
        );
        assert_eq!(acked.events[0].reject, Some(RejectClass::Refused));

        // `-2013` from a status query is the DEFINITIVE absence: an answer, not a rejection. It kept
        // the `Gone` name, and it is the one that must reach no reject counter — a limit wired to a
        // counter it ticked would fire precisely while the engine was reconciling.
        let resolved = decoded(
            fixture!("error_2013_no_such_order"),
            ExecRequest::OrderStatus {
                instrument: InstrumentId(0),
                client_id: ORDER_A,
            },
            &venue,
        );
        assert_eq!(resolved.events[0].reject, Some(RejectClass::Gone));

        // Out of money halts, and is routine on neither path. It shares `-2010` with the cross, so
        // only the message separates them.
        for event in [
            decode_exec(fixture!("report_rejected_insufficient_balance"), &venue),
            decoded(
                fixture!("error_2010_insufficient_balance"),
                place_request(ORDER_A),
                &venue,
            )
            .events[0],
        ] {
            assert_eq!(event.reject, Some(RejectClass::Fatal));
        }
    }
}
