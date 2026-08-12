//! ONE-SHOT live Polymarket execution proof: place, verify, cancel, cancel-and-replace, then a
//! taker fill reversed immediately — driven through the PRODUCTION signing, encoding and decoding
//! path against the real venue, with real funds.
//!
//! PERMISSION RULE — BINDING, DO NOT DELETE. After the single proving run this test stays
//! `#[ignore]`d FOREVER, and NO agent re-runs it without explicit permission from the team lead,
//! who asks Shaun if in doubt. It sends real orders and spends real money: a re-run is a
//! funds-moving action, not a test invocation. The `POLY_EXEC_LIVE` gate below is a second lock on
//! the same door, never a substitute for asking.
//!
//! WHY IT EXISTS. Eleven facts no fixture can reach are decided here, and each is a premise some
//! shipped code already leans on: that the venue accepts a signatureType-2 order at all, that the
//! `orderID` in a placement answer is the same string the user stream names it by (correlation's
//! whole basis), that a cancel inside the taker hold is refused, that the keepalive reply is
//! literally `PONG`. The run prints a verdict per item — VERIFIED, NOT-OBSERVED or CONTRADICTED —
//! because "the test passed" is not the deliverable; the answers are.
//!
//! THE MONEY RULES ARE STRUCTURAL. Nothing above [`MAX_SHARES`] shares, nothing above
//! [`MAX_ORDER_NOTIONAL_USD`] per order, never more than [`MAX_OPEN_ORDERS`] resting at once, and a
//! hard abort into teardown the moment collateral reads [`BALANCE_TRIPWIRE_USD`] below baseline.
//! Teardown runs on EVERY path including a panic — the sequence is caught, swept, and only then
//! re-raised — and the venue's dead man's switch is armed BEFORE the first placement, so even a
//! killed process has its book cancelled within ~15s.
//!
//! Teardown CANCELS AND THEN FLATTENS. It is not merely a cancel sweep, because a deep post-only
//! bid can be reached by a market that was measured moving 0.42 -> 0.20 in seventy seconds, and a
//! position nobody exits rides to resolution. So a surviving holding gets ONE marketable exit under
//! the same money rules — and the balance tripwire deliberately does not gate a SELL, since a guard
//! that blocks the order recovering the balance is a guard defeating itself.
//!
//! Run: `POLY_EXEC_LIVE=1 cargo test --test integration poly_exec -- --ignored --nocapture`

use std::fmt::Display;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{FutureExt, SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use polysim::adapters::exec::ExecRequest;
use polysim::adapters::polymarket::discovery::PolySchedule;
use polysim::adapters::polymarket::exec::codec::{
    DecodeContext, EncodeContext, EncodedRequest, IgnoredReason, KnownOrder, OrderIndex,
    OrderSigner, OrderSignerSetup, PlaceRequestContext, PlacementStatus, StreamEvent, TokenBinding,
    TokenTable, VenueAnswer, cancel_market_orders, clob_market_request, collateral_balance,
    conditional_allowance_refresh, conditional_balance, decode_balance, decode_cancel,
    decode_clob_market, decode_heartbeat, decode_neg_risk, decode_place, decode_single_order,
    decode_stream_frame, encode_request, heartbeat, neg_risk_request, open_orders_page,
    protocol_version, server_time, subscribe_user_stream, trades_page,
};
use polysim::adapters::polymarket::exec::handle::{WalletIdentity, preflight_polymarket};
use polysim::adapters::polymarket::exec::rest::{ClobHttp, ClobResponse, GEOBLOCK_URL};
use polysim::adapters::polymarket::exec::sign::key::SigningKey;
use polysim::adapters::polymarket::exec::sign::l2::{ApiCredentials, RequestSigner};
use polysim::adapters::polymarket::rest::{GammaMarket, PolyRest, book_url};
use polysim::config::PolySeries;
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::log::{self, LogConfig};
use polysim::msg::exec::{ExecKind, OrderStyle};
use polysim::time::TsUs;

const LIVE_GATE: &str = "POLY_EXEC_LIVE";

const JOURNAL_PATH: &str = ".work/poly-exec-live-run.md";

/// The venue's own minimum order size, and therefore also our maximum: there is no smaller trade to
/// prove the path with, and a larger one proves nothing extra.
const MAX_SHARES: f64 = 5.0;

/// A share cannot settle above $1, so five cap at $5. This floor under that is what stops the taker
/// leg firing once the outcome has drifted expensive.
const MAX_ORDER_NOTIONAL_USD: f64 = 3.50;

const MAX_OPEN_ORDERS: usize = 2;

const BALANCE_TRIPWIRE_USD: f64 = 5.0;
const RESTING_BID_CEILING: f64 = 0.20;
const RESTING_BID_TICKS_BELOW_TOUCH: i64 = 10;
const TAKER_TICKS_THROUGH: i64 = 2;
const DEPTH_COVER_MULTIPLE: f64 = 2.0;
const DEPTH_BAND_TICKS: i64 = 2;
const RUN_WINDOW_MIN_REMAINING_S: i64 = 210;
const WINDOW_SECS: i64 = 300;
const POST_BOUNDARY_SETTLE_S: u64 = 5;
const MIN_WINDOW_REMAINING_S: i64 = 90;
const STREAM_WAIT: Duration = Duration::from_secs(8);
const STREAM_POLL: Duration = Duration::from_millis(100);
const PING_INTERVAL: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const REVERSE_REST_TIMEOUT: Duration = Duration::from_secs(30);
const DOCUMENTED_TAKER_HOLD: Duration = Duration::from_millis(250);
const USER_STREAM_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/user";
const INSTRUMENT: InstrumentId = InstrumentId(0);
const ORDER_INDEX_CAPACITY: usize = 32;
const RETIRED_BINDING_CAPACITY: usize = 4;

// ---------------------------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------------------------

#[ignore = "LIVE ORDERS, REAL MONEY, ONE-SHOT — needs the team lead's permission before any run: POLY_EXEC_LIVE=1 cargo test --test integration poly_exec -- --ignored --nocapture"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn poly_exec_places_cancels_and_reverses_against_the_live_venue() {
    assert!(
        std::env::var(LIVE_GATE).is_ok_and(|value| value.trim() == "1"),
        "this test sends real orders and spends real funds — it runs only with {LIVE_GATE}=1, and \
         only with the team lead's explicit permission for THIS run"
    );

    let log_handle = log::init(&LogConfig::default());
    log::register_thread("poly-exec-live");

    let mut journal = Journal::open();
    journal.banner();

    // Step 0 is read-only in its entirety, so a failure here needs no teardown: nothing has been
    // sent that could rest, fill, or cost anything.
    let prepared = match prepare(&mut journal).await {
        Ok(prepared) => prepared,
        Err(failure) => {
            journal.record("0/abort", format!("preparation failed: {failure:#}"));
            log_handle.drain();
            panic!("live-run preparation failed before anything was sent: {failure:#}");
        }
    };

    let mut run = Run::new(prepared, journal);

    // Everything past here can leave an order resting or a position open, so the sweep is
    // unconditional: a panic is caught, swept, and only then re-raised.
    let sequence = AssertUnwindSafe(run_sequence(&mut run))
        .catch_unwind()
        .await;
    let stopped = match sequence {
        Ok(Ok(())) => None,
        Ok(Err(failure)) => Some(format!("{failure:#}")),
        Err(_) => Some("the sequence PANICKED — teardown ran anyway".to_owned()),
    };
    if let Some(failure) = &stopped {
        run.journal
            .record("!!", format!("sequence stopped: {failure}"));
    }

    let teardown = teardown(&mut run).await;
    if let Err(failure) = &teardown {
        run.journal
            .record("7/FAILED", format!("teardown itself failed: {failure:#}"));
    }
    run.stop_background_tasks();
    run.print_report();
    log_handle.drain();

    let report = run.teardown.clone();
    if let Err(failure) = teardown {
        panic!(
            "TEARDOWN FAILED — a position may still be live on the venue (resting orders should \
             die with the heartbeat within ~15s). Read {JOURNAL_PATH} and check the account by \
             hand: {failure:#}"
        );
    }
    let report = report.expect("a successful teardown records its report");
    assert!(
        report.open_orders == 0,
        "{} orders were still resting after the sweep — cancel them by hand",
        report.open_orders
    );
    assert!(
        report.collateral_drop_usd < BALANCE_TRIPWIRE_USD,
        "collateral fell ${:.2} against a ${BALANCE_TRIPWIRE_USD:.2} tripwire — investigate before \
         any further live work",
        report.collateral_drop_usd
    );
    let contradicted = run.checklist.contradicted_ids();
    assert!(
        contradicted.is_empty(),
        "the venue CONTRADICTED premises this engine relies on: items {contradicted:?} — see the \
         checklist table above"
    );
    assert!(
        stopped.is_none(),
        "the live sequence did not complete: {}",
        stopped.unwrap_or_default()
    );
}

// ---------------------------------------------------------------------------------------------
// Step 0 — safety pre-flight, entirely read-only
// ---------------------------------------------------------------------------------------------

struct Prepared {
    venue: Venue,
    market: BoundMarket,
    baseline_collateral: i64,
    is_geoblocked: bool,
}

/// The window, token and grid this run trades, resolved once and never re-derived.
struct BoundMarket {
    slug: String,
    token_id: String,
    tick: Price,
    min_size: Qty,
    has_taker_delay: bool,
}

async fn prepare(journal: &mut Journal) -> Result<Prepared> {
    let http = ClobHttp::new(Duration::from_secs(10), Duration::from_secs(30))
        .context("building the polymarket http client")?;

    // The geoblock flag is a website signal and is READ, never a gate: the venue's answer to an
    // actual placement is the ground truth, and capturing this first makes the two comparable.
    let geoblock = http
        .send_unsigned(GEOBLOCK_URL)
        .await
        .context("reading the geoblock status")?;
    journal.answer("0/geoblock", GEOBLOCK_URL, &geoblock);
    let is_geoblocked = geoblock.body.contains("\"blocked\":true");

    let version = http
        .send_public(&protocol_version())
        .await
        .context("reading the clob protocol version")?;
    journal.answer("0/version", "/version", &version);
    let venue_time = http
        .send_public(&server_time())
        .await
        .context("reading the clob server time")?;
    journal.answer("0/time", "/time", &venue_time);

    // The startup gate the engine itself runs: it mints the L2 credentials, proves the protocol
    // version, refuses a closed-only account, and hands back the wallet triangle to sign with.
    let preflight = preflight_polymarket()
        .await
        .context("the engine's own polymarket startup gate")?;
    journal.record(
        "0/preflight",
        format!(
            "maker {} signer {} signatureType {} clock offset {}us",
            preflight.wallet.maker.to_checksum_hex(),
            preflight.wallet.signer.to_checksum_hex(),
            preflight.wallet.signature_type.code(),
            preflight.venue_clock_offset.micros()
        ),
    );

    let mut venue = Venue::new(
        http,
        preflight.credentials,
        preflight.key,
        preflight.wallet,
        preflight.venue_clock_offset.micros(),
    )?;

    let collateral = venue.collateral().await.context("baseline collateral")?;
    journal.record(
        "0/balance-before",
        format!("collateral ${:.6}", usd(collateral)),
    );

    let open = venue
        .open_order_ids()
        .await
        .context("baseline open orders")?;
    journal.record(
        "0/open-before",
        format!("{} open orders {open:?}", open.len()),
    );
    if !open.is_empty() {
        bail!(
            "{} orders are already resting on these credentials — this run refuses to start where \
             it cannot tell its own orders from someone else's",
            open.len()
        );
    }

    let gamma = resolve_trading_window(journal).await?;
    let market = bind_market(&mut venue, journal, &gamma).await?;

    Ok(Prepared {
        venue,
        market,
        baseline_collateral: collateral,
        is_geoblocked,
    })
}

/// The opening minute of a window, or a wait for the next boundary. Two binding reasons: an up/down
/// outcome trades near even money only before the underlying has moved, and the whole sequence —
/// including a 30s resting fallback and a sweep — has to finish well before the market resolves.
async fn resolve_trading_window(journal: &mut Journal) -> Result<GammaMarket> {
    let remaining = WINDOW_SECS - unix_now_s().rem_euclid(WINDOW_SECS);
    if remaining < RUN_WINDOW_MIN_REMAINING_S {
        let wait = remaining as u64 + POST_BOUNDARY_SETTLE_S;
        journal.record(
            "0/window",
            format!(
                "{remaining}s left in the current window (< {RUN_WINDOW_MIN_REMAINING_S}s) — \
                 waiting {wait}s for the next boundary so the taker leg trades near even money"
            ),
        );
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }

    let now = TsUs::from_micros(unix_now_s() * 1_000_000);
    let window = PolySchedule::BTC_5M.current_window(now);
    let rest = PolyRest::new(PolySeries::BtcUpDown5m).context("building the gamma rest client")?;
    let market = rest
        .resolve_slug(window.window_start_ts_us)
        .await
        .context("resolving the current up/down window through gamma")?;

    let closes_in = market.window_close_ts_us.micros() / 1_000_000 - unix_now_s();
    journal.record(
        "0/window",
        format!(
            "{} condition {} closes in {closes_in}s (tick {} min size {})",
            market.slug,
            market.condition_id,
            market.tick_size.to_f64(),
            market.min_order_size.to_f64()
        ),
    );
    if closes_in < MIN_WINDOW_REMAINING_S {
        bail!(
            "only {closes_in}s left in {} — refusing to start",
            market.slug
        );
    }
    Ok(market)
}

/// The three reads a rotation binding needs, made exactly as the edge makes them. `/neg-risk` is the
/// load-bearing one: it names which exchange contract signs this token, and the wrong contract
/// invalidates every signature with an error that mentions neither.
async fn bind_market(
    venue: &mut Venue,
    journal: &mut Journal,
    gamma: &GammaMarket,
) -> Result<BoundMarket> {
    let market_request = clob_market_request(&gamma.condition_id);
    let market_answer = venue
        .public(&market_request)
        .await
        .context("reading /clob-markets")?;
    journal.answer("0/clob-markets", &market_request.path, &market_answer);
    let market =
        decode_clob_market(&market_answer.body).context("decoding the /clob-markets answer")?;

    let token_id = gamma.token_up.to_string();
    let neg_risk = neg_risk_request(&token_id);
    let neg_risk_answer = venue.public(&neg_risk).await.context("reading /neg-risk")?;
    journal.answer("0/neg-risk", &neg_risk.path, &neg_risk_answer);
    let is_neg_risk =
        decode_neg_risk(&neg_risk_answer.body).context("decoding the /neg-risk answer")?;

    journal.record(
        "0/binding",
        format!(
            "token {token_id} tick {} min {} neg_risk {is_neg_risk} taker_delay {} accepting {} \
             fee bps maker/taker {}/{}",
            market.tick_size.to_f64(),
            market.min_order_size.to_f64(),
            market.has_taker_delay,
            market.is_accepting_orders,
            market.maker_fee_bps,
            market.taker_fee_bps
        ),
    );
    if !market.is_accepting_orders {
        bail!("{} is not accepting orders", gamma.slug);
    }

    venue.bind(TokenBinding {
        instrument: INSTRUMENT,
        token_id: token_id.clone().into_boxed_str(),
        tick: market.tick_size,
        is_neg_risk,
    });

    let book = fetch_book(&token_id)
        .await
        .context("pre-check /book read")?;
    journal.record("0/book", book.describe());

    Ok(BoundMarket {
        slug: gamma.slug.to_string(),
        token_id,
        tick: market.tick_size,
        min_size: market.min_order_size,
        has_taker_delay: market.has_taker_delay,
    })
}

// ---------------------------------------------------------------------------------------------
// Steps 1-6 — the guarded sequence
// ---------------------------------------------------------------------------------------------

async fn run_sequence(run: &mut Run) -> Result<()> {
    start_heartbeat(run).await?;
    open_user_stream(run).await?;

    let Some(resting) = place_the_enforcement_probe(run).await? else {
        // Venue-blocked. The probe answered the one question no fixture can, and that IS the
        // result — teardown follows, and the run reports rather than fails.
        return Ok(());
    };

    verify_resting_order(run, &resting).await?;
    cancel_resting_order(run, &resting, "4/cancel").await?;
    cancel_and_replace(run).await?;
    fill_and_reverse(run).await
}

/// Step 1 — the dead man's switch, armed BEFORE anything can rest. An empty id starts the chain and
/// the venue answers with the id the next beat must echo.
async fn start_heartbeat(run: &mut Run) -> Result<()> {
    let request = heartbeat("").context("encoding the opening heartbeat")?;
    let answer = run
        .venue
        .signed(&request)
        .await
        .context("starting the heartbeat chain")?;
    run.journal.answer("1/heartbeat", &request.path, &answer);
    if !answer.is_success() {
        bail!(
            "the heartbeat chain would not start (http {}): {} — refusing to place an order \
             without the venue's dead man's switch",
            answer.status,
            answer.excerpt()
        );
    }
    let first = decode_heartbeat(&answer.body).context("decoding the opening heartbeat id")?;
    run.checklist.verified(
        7,
        format!("an empty id started the chain; the venue answered id {first}"),
    );

    let state = run.heartbeat_log.clone();
    let http = run.venue.http.clone();
    let signer = RequestSigner::new(&run.venue.credentials, run.venue.wallet.signer)
        .context("a second request signer for the heartbeat lane")?;
    let offset_us = run.venue.clock_offset_us;
    let mut current = first.to_string();
    run.heartbeat_task = Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Ok(request) = heartbeat(&current) else {
                return;
            };
            let seconds = (local_micros() + offset_us) / 1_000_000;
            let Ok(answer) = http.send_signed(&signer, &request, seconds).await else {
                continue;
            };
            let mut log = state.lock().expect("heartbeat log");
            log.beats += 1;
            match decode_heartbeat(&answer.body) {
                Ok(next) => {
                    if !answer.is_success() {
                        log.stale_recoveries.push(format!(
                            "http {} carried the expected id {next} where we sent {current}",
                            answer.status
                        ));
                    }
                    current = next.to_string();
                    log.last_id = current.clone();
                }
                Err(_) => {
                    log.failures
                        .push(format!("http {}: {}", answer.status, answer.excerpt()))
                }
            }
        }
    }));
    Ok(())
}

/// Step 1b — the user stream, subscribed UNFILTERED and BEFORE any order exists, so nothing the
/// venue says about our orders can be missed while we are still setting up.
async fn open_user_stream(run: &mut Run) -> Result<()> {
    let frame = subscribe_user_stream(
        run.venue.credentials.api_key(),
        run.venue.credentials.secret(),
        run.venue.credentials.passphrase(),
    )
    .context("encoding the user-stream subscription")?;

    let (socket, response) = connect_async(USER_STREAM_URL)
        .await
        .with_context(|| format!("connecting to {USER_STREAM_URL}"))?;
    let (mut writer, reader) = socket.split();
    writer
        .send(Message::Text(frame.into()))
        .await
        .context("sending the user-stream subscription")?;
    run.subscribed_at = Some(TsUs::from_micros(local_micros()));
    run.journal.record(
        "1/stream",
        format!(
            "subscribed unfiltered to {USER_STREAM_URL} (handshake http {})",
            response.status().as_u16()
        ),
    );

    run.stream_task = Some(tokio::spawn(serve_user_stream(
        writer,
        reader,
        run.frames.clone(),
    )));
    Ok(())
}

/// Step 2 — THE ENFORCEMENT PROBE. Whether this host may open a position is decided here and
/// nowhere else: the geoblock flag is a website signal, a placement is the venue speaking.
/// `Ok(None)` means the venue refused on region grounds, which is a successful probe.
async fn place_the_enforcement_probe(run: &mut Run) -> Result<Option<RestingOrder>> {
    let book = fetch_book(&run.market.token_id)
        .await
        .context("book read before the resting bid")?;
    let price = run.resting_bid_price(&book);
    run.journal.record(
        "2/plan",
        format!(
            "post-only GTC BUY {MAX_SHARES} @ {:.2} (${:.2}), deep and unfillable by construction; \
             {}",
            price.to_f64(),
            price.to_f64() * MAX_SHARES,
            book.describe_touch()
        ),
    );

    match run
        .place(
            "2/place",
            Side::Buy,
            price,
            shares(MAX_SHARES),
            OrderStyle::PostOnly,
        )
        .await?
    {
        Placement::Accepted(resting) => {
            run.checklist.verified(
                1,
                format!(
                    "a signatureType-{} order was accepted; venue id {}",
                    run.venue.wallet.signature_type.code(),
                    resting.venue_order_id
                ),
            );
            Ok(Some(resting))
        }
        Placement::Refused { detail } if reads_as_region_block(&detail) => {
            run.is_region_blocked = true;
            run.journal.record(
                "2/VENUE-BLOCKED",
                format!(
                    "the venue refused the placement on region grounds: {detail} — this is the \
                     answer the probe existed to get, not a failure"
                ),
            );
            run.checklist.verified(
                1,
                format!("a region refusal, not a signature refusal: {detail}"),
            );
            Ok(None)
        }
        Placement::Refused { detail } => {
            run.checklist.contradicted(
                1,
                format!("the venue refused a signatureType-2 order: {detail}"),
            );
            bail!("the venue refused the opening placement: {detail}");
        }
    }
}

/// Step 3 — the same order seen three ways. REST agrees with the placement answer, and the stream
/// names it by the very id that answer minted (item 2, correlation's premise).
async fn verify_resting_order(run: &mut Run, resting: &RestingOrder) -> Result<()> {
    let request = run.encode(ExecRequest::OrderStatus {
        instrument: INSTRUMENT,
        client_id: resting.client_id,
    })?;
    let answer = run
        .venue
        .signed(&request)
        .await
        .context("reading the order back")?;
    run.journal.answer("3/order-read", &request.path, &answer);
    let decoded = {
        let context = run.venue.decode_context();
        decode_single_order(answer.answer(), 0, &context)
            .context("decoding the single-order read")?
    };
    match decoded {
        VenueAnswer::Answered(Some(event)) => run.journal.record(
            "3/order-read",
            format!(
                "REST agrees: status {:?} price {} qty {} filled {}",
                event.status,
                event.price.to_f64(),
                event.qty.to_f64(),
                event.cumulative_qty.to_f64()
            ),
        ),
        VenueAnswer::Answered(None) => bail!(
            "the single-order read named an order this run cannot map — the correlation index and \
             the venue disagree about {}",
            resting.venue_order_id
        ),
        VenueAnswer::Unavailable(state) => run
            .journal
            .record("3/order-read", format!("venue unavailable: {state:?}")),
    }

    // The account-wide page proves the read reaches our own order. What it cannot show is whether a
    // SECOND api key on this maker would appear too — that needs a second key (item 10).
    let open = run
        .venue
        .open_order_ids()
        .await
        .context("open-orders page")?;
    let visible = open.contains(&resting.venue_order_id);
    run.journal.record(
        "3/open-orders",
        format!("{} open, ours visible: {visible} {open:?}", open.len()),
    );
    run.checklist.note(
        10,
        format!(
            "our own order is visible on our api key ({visible}); whether the page is maker-scoped \
             rather than key-scoped cannot be told with one api key on the account"
        ),
    );
    assert_open_order_cap(open.len())?;

    let seen = run
        .await_stream_event(
            STREAM_WAIT,
            |event| matches!(event, StreamEvent::Order(order) if order.kind == ExecKind::ReportNew),
        )
        .await;
    match seen.event {
        Some(_) => run.checklist.verified(
            2,
            format!(
                "the user stream named the order by the placement answer's own id ({}) — it \
                 resolved through the correlation index built from that answer",
                resting.venue_order_id
            ),
        ),
        None if seen.unresolved > 0 => run.checklist.contradicted(
            2,
            format!(
                "{} order frames arrived and NONE resolved against the id the placement answer \
                 minted — correlation's premise does not hold",
                seen.unresolved
            ),
        ),
        None => run.checklist.note(
            2,
            format!("no order frame arrived within {STREAM_WAIT:?} — the id was never compared"),
        ),
    }
    run.assess_subscribe_ack();
    Ok(())
}

/// Step 4 — the cancel, confirmed on both surfaces.
async fn cancel_resting_order(run: &mut Run, resting: &RestingOrder, step: &str) -> Result<()> {
    let request = run.encode(ExecRequest::Cancel {
        instrument: INSTRUMENT,
        client_id: resting.client_id,
    })?;
    let started = Instant::now();
    let answer = run.venue.signed(&request).await.context("cancelling")?;
    let elapsed = started.elapsed();
    run.journal.answer(step, &request.path, &answer);
    run.journal
        .record(step, format!("cancel round trip {}ms", elapsed.as_millis()));
    run.note_rate_limit(&answer);

    let decoded = {
        let context = run.venue.decode_context();
        decode_cancel(answer.answer(), &context).context("decoding the cancel")?
    };
    match decoded {
        VenueAnswer::Answered(events) => {
            let canceled = events
                .iter()
                .filter(|event| event.kind == ExecKind::AckCanceled)
                .count();
            let refusals: Vec<String> = events
                .iter()
                .filter(|event| event.kind == ExecKind::AckFailed)
                .map(|event| format!("{:?}", event.reject))
                .collect();
            run.journal
                .record(step, format!("{canceled} canceled, refusals {refusals:?}"));
            if canceled == 0 {
                bail!("the venue canceled nothing: refusals {refusals:?}");
            }
        }
        VenueAnswer::Unavailable(state) => {
            bail!("the venue was unavailable for the cancel: {state:?}")
        }
    }
    run.resting.retain(|id| *id != resting.venue_order_id);

    let seen = run
        .await_stream_event(STREAM_WAIT, |event| {
            matches!(event, StreamEvent::Order(order) if order.kind == ExecKind::ReportCanceled)
        })
        .await;
    match seen.event {
        Some(_) => run
            .journal
            .record(step, "the user stream reported the CANCELLATION"),
        None => run
            .journal
            .record(step, "no CANCELLATION frame arrived within the wait"),
    }
    Ok(())
}

/// Step 5 — cancel-and-replace, which is what this venue offers INSTEAD of an amend. The engine
/// stamps a zero amend budget for exactly this reason, so what has to work is the pair, twice.
async fn cancel_and_replace(run: &mut Run) -> Result<()> {
    let book = fetch_book(&run.market.token_id)
        .await
        .context("book read before the replace pair")?;
    let first = run.resting_bid_price(&book);
    let second = Price((first.0 - run.market.tick.0).max(run.market.tick.0));
    let size = shares(MAX_SHARES);

    let Placement::Accepted(order) = run
        .place("5/place-a", Side::Buy, first, size, OrderStyle::PostOnly)
        .await?
    else {
        bail!("the replace pair's first placement was refused");
    };
    cancel_resting_order(run, &order, "5/cancel-a").await?;

    let Placement::Accepted(replacement) = run
        .place("5/place-b", Side::Buy, second, size, OrderStyle::PostOnly)
        .await?
    else {
        bail!("the replace pair's second placement was refused");
    };
    cancel_resting_order(run, &replacement, "5/cancel-b").await?;

    run.journal.record(
        "5/replace",
        format!(
            "cancel+replace proven at {:.2} then {:.2} — this venue's substitute for an amend",
            first.to_f64(),
            second.to_f64()
        ),
    );
    Ok(())
}

/// Step 6 — the only leg that intends to trade. A taker buy through the touch, then the same size
/// straight back out, with the book checked for cover on BOTH sides first: an unreversed position
/// rides to resolution, and below the venue minimum it cannot be reversed at all.
async fn fill_and_reverse(run: &mut Run) -> Result<()> {
    let book = fetch_book(&run.market.token_id)
        .await
        .context("depth pre-check before the taker leg")?;
    let cover = MAX_SHARES * DEPTH_COVER_MULTIPLE;
    let band = run.market.tick.0 * DEPTH_BAND_TICKS;
    let bid_depth = book.depth_within(Side::Buy, band);
    let ask_depth = book.depth_within(Side::Sell, band);
    run.journal.record(
        "6/depth",
        format!(
            "{} — within {DEPTH_BAND_TICKS} ticks: bid {bid_depth:.2}, ask {ask_depth:.2} shares \
             (need {cover:.2} each side)",
            book.describe_touch()
        ),
    );
    let (Some(best_bid), Some(best_ask)) = (book.best_bid, book.best_ask) else {
        run.journal.record(
            "6/SKIPPED",
            "the book is one-sided — a reverse could not be priced",
        );
        return Ok(());
    };
    if bid_depth < cover || ask_depth < cover {
        run.journal.record(
            "6/SKIPPED",
            format!(
                "insufficient cover (bid {bid_depth:.2}, ask {ask_depth:.2}, need {cover:.2}) — a \
                 partial fill below the {}-share minimum is unflattenable",
                run.market.min_size.to_f64()
            ),
        );
        return Ok(());
    }

    let buy_price = Price(best_ask.0 + run.market.tick.0 * TAKER_TICKS_THROUGH);
    let notional = buy_price.to_f64() * MAX_SHARES;
    if notional > MAX_ORDER_NOTIONAL_USD {
        run.journal.record(
            "6/SKIPPED",
            format!(
                "a taker buy at {:.2} costs ${notional:.2}, over the ${MAX_ORDER_NOTIONAL_USD:.2} \
                 cap — the outcome has drifted too expensive to round-trip inside the money rules",
                buy_price.to_f64()
            ),
        );
        return Ok(());
    }

    // The chain being approved is not enough: the CLOB caches allowances per token, and an
    // unrefreshed cache rejects a sell as an empty wallet. Warmed BEFORE the buy so the reverse is
    // never the call that discovers it.
    let refresh =
        conditional_allowance_refresh(&run.market.token_id, run.venue.wallet.signature_type);
    let refreshed = run
        .venue
        .signed(&refresh)
        .await
        .context("warming the conditional allowance cache")?;
    run.journal.answer("6/allowance", &refresh.path, &refreshed);
    if !refreshed.is_success() {
        bail!(
            "the conditional allowance refresh failed (http {}): {} — a SELL must never be sent \
             before it succeeds",
            refreshed.status,
            refreshed.excerpt()
        );
    }

    let started = Instant::now();
    let bought = run
        .place(
            "6/buy",
            Side::Buy,
            buy_price,
            shares(MAX_SHARES),
            OrderStyle::Immediate,
        )
        .await?;
    let blocked = started.elapsed();
    run.journal.record(
        "6/buy",
        format!("POST /order blocked {}ms", blocked.as_millis()),
    );
    match run.market.has_taker_delay && blocked >= DOCUMENTED_TAKER_HOLD {
        true => run.checklist.verified(
            3,
            format!(
                "POST blocked {}ms on an itode market, at or beyond the documented {}ms hold",
                blocked.as_millis(),
                DOCUMENTED_TAKER_HOLD.as_millis()
            ),
        ),
        false => run.checklist.note(
            3,
            format!(
                "POST blocked {}ms (itode {}) — under the documented {}ms hold, so the withheld \
                 cancel never had to trigger",
                blocked.as_millis(),
                run.market.has_taker_delay,
                DOCUMENTED_TAKER_HOLD.as_millis()
            ),
        ),
    }

    let filled = match bought {
        Placement::Accepted(order) => {
            run.journal.record(
                "6/buy",
                format!(
                    "status {:?} venue id {} filled {}",
                    order.status,
                    order.venue_order_id,
                    order.filled.to_f64()
                ),
            );
            if order.status == PlacementStatus::Delayed {
                probe_cancel_inside_the_hold(run, &order).await?;
            }
            order
        }
        Placement::Refused { detail } => {
            run.journal.record(
                "6/buy",
                format!("the taker buy was refused: {detail} — nothing to reverse"),
            );
            return Ok(());
        }
    };

    run.inspect_trade_lineage().await;

    let held = run
        .venue
        .share_balance(&run.market.token_id)
        .await
        .context("share balance after the taker buy")?;
    run.position = held;
    run.journal.record(
        "6/position",
        format!(
            "the venue says {} shares held; the placement answer reported {} filled",
            held.to_f64(),
            filled.filled.to_f64()
        ),
    );
    if held.0 <= 0 {
        run.journal
            .record("6/reverse", "nothing filled — no reverse needed");
        return Ok(());
    }
    reverse_position(run, held, best_bid).await
}

/// The reverse. A marketable sell first; if that finds nothing, a resting sell inside the spread for
/// a bounded wait; and if THAT finds nothing, the position is reported loudly and rides to
/// resolution — the only honest outcome once the venue's minimum makes a smaller exit impossible.
async fn reverse_position(run: &mut Run, held: Qty, best_bid: Price) -> Result<()> {
    if held < run.market.min_size {
        run.journal.record(
            "6/STRANDED",
            format!(
                "{} shares is below the venue's {}-share minimum — UNFLATTENABLE BY RULE; the \
                 position rides to resolution and the balance delta will show it",
                held.to_f64(),
                run.market.min_size.to_f64()
            ),
        );
        return Ok(());
    }

    let sell_price =
        Price((best_bid.0 - run.market.tick.0 * TAKER_TICKS_THROUGH).max(run.market.tick.0));
    let sold = run
        .place(
            "6/sell",
            Side::Sell,
            sell_price,
            held,
            OrderStyle::Immediate,
        )
        .await?;
    let remaining = run
        .venue
        .share_balance(&run.market.token_id)
        .await
        .context("share balance after the reverse")?;
    run.position = remaining;
    run.journal.record(
        "6/sell",
        format!(
            "{sold:?}; the venue says {} shares left",
            remaining.to_f64()
        ),
    );
    if remaining.0 <= 0 {
        run.journal.record("6/flat", "the position is flat");
        return Ok(());
    }

    let book = fetch_book(&run.market.token_id)
        .await
        .context("book read before the resting reverse")?;
    let Some(inside) = book.inside_spread_sell(run.market.tick) else {
        run.journal.record(
            "6/STRANDED",
            format!(
                "{} shares held and no spread to rest inside — the position rides to resolution",
                remaining.to_f64()
            ),
        );
        return Ok(());
    };
    let Placement::Accepted(resting) = run
        .place(
            "6/sell-rest",
            Side::Sell,
            inside,
            remaining,
            OrderStyle::PostOnly,
        )
        .await?
    else {
        run.journal.record(
            "6/STRANDED",
            format!(
                "{} shares held and the resting reverse was refused — the position rides to \
                 resolution",
                remaining.to_f64()
            ),
        );
        return Ok(());
    };

    let deadline = Instant::now() + REVERSE_REST_TIMEOUT;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let left = run
            .venue
            .share_balance(&run.market.token_id)
            .await
            .unwrap_or(remaining);
        if left.0 <= 0 {
            run.journal.record("6/flat", "the resting reverse filled");
            break;
        }
    }
    cancel_resting_order(run, &resting, "6/sell-rest-cancel").await?;

    let left = run
        .venue
        .share_balance(&run.market.token_id)
        .await
        .context("share balance after the resting reverse")?;
    run.position = left;
    if left.0 > 0 {
        run.journal.record(
            "6/STRANDED",
            format!(
                "*** {} shares of {} REMAIN HELD after both exits — the position rides to \
                 resolution and the balance delta will show it ***",
                left.to_f64(),
                run.market.token_id
            ),
        );
    }
    Ok(())
}

/// Item 4 — a cancel sent inside the taker hold. Sent ONCE and expected to be refused; the
/// never-retry rule is exactly why this must not become a loop.
async fn probe_cancel_inside_the_hold(run: &mut Run, order: &RestingOrder) -> Result<()> {
    let request = run.encode(ExecRequest::Cancel {
        instrument: INSTRUMENT,
        client_id: order.client_id,
    })?;
    let answer = run
        .venue
        .signed(&request)
        .await
        .context("the in-hold cancel probe")?;
    run.journal.answer("6/hold-cancel", &request.path, &answer);
    match !answer.is_success() || answer.body.contains("not_canceled") {
        true => run.checklist.verified(
            4,
            format!(
                "a cancel inside the taker hold was refused (http {}): {}",
                answer.status,
                answer.excerpt()
            ),
        ),
        false => run.checklist.contradicted(
            4,
            format!(
                "a cancel inside the taker hold SUCCEEDED — the withheld-cancel machinery delays a \
                 cancel the venue would have taken: {}",
                answer.excerpt()
            ),
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Step 7 — teardown, which runs on every path
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TeardownReport {
    open_orders: usize,
    collateral_drop_usd: f64,
    shares_left: Qty,
}

async fn teardown(run: &mut Run) -> Result<TeardownReport> {
    run.journal
        .record("7/teardown", "sweeping — this runs on every path");

    // Every resting order on the one token this run bound, in one call. It is the production
    // rotation sweep, and it cannot touch an order on a token this run never traded.
    let sweep = cancel_market_orders(&run.market.token_id).context("encoding the sweep")?;
    match run.venue.signed(&sweep).await {
        Ok(answer) => {
            run.journal.answer("7/cancel-market", &sweep.path, &answer);
            run.note_rate_limit(&answer);
        }
        Err(failure) => run.journal.record(
            "7/cancel-market",
            format!("the sweep call failed: {failure:#}"),
        ),
    }

    let survivors = run
        .venue
        .open_order_ids()
        .await
        .context("open orders after the sweep")?;
    if !survivors.is_empty() {
        run.journal.record(
            "7/residual",
            format!(
                "{} orders survived the sweep {survivors:?}",
                survivors.len()
            ),
        );
        for id in survivors {
            let Some(known) = run.venue.orders.resolve(&id) else {
                run.journal.record(
                    "7/residual",
                    format!("{id} is not an order this run placed — LEFT ALONE, cancel by hand"),
                );
                continue;
            };
            let request = match run.encode(ExecRequest::Cancel {
                instrument: INSTRUMENT,
                client_id: known.client_id,
            }) {
                Ok(request) => request,
                Err(failure) => {
                    run.journal
                        .record("7/residual-cancel", format!("{id}: {failure:#}"));
                    continue;
                }
            };
            match run.venue.signed(&request).await {
                Ok(answer) => run
                    .journal
                    .answer("7/residual-cancel", &request.path, &answer),
                Err(failure) => run
                    .journal
                    .record("7/residual-cancel", format!("{id}: {failure:#}")),
            }
        }
    }

    let held = run
        .venue
        .share_balance(&run.market.token_id)
        .await
        .unwrap_or(Qty(0));
    let shares_left = flatten_residual(run, held).await;
    run.position = shares_left;

    let open = run
        .venue
        .open_order_ids()
        .await
        .context("open orders at the end of the teardown")?;

    let collateral = run
        .venue
        .collateral()
        .await
        .context("collateral after the run")?;

    let trades = trades_page(None);
    match run.venue.signed(&trades).await {
        Ok(answer) => run.journal.answer("7/trades", &trades.path, &answer),
        Err(failure) => run
            .journal
            .record("7/trades", format!("the trades read failed: {failure:#}")),
    }

    run.journal.record(
        "7/balance-after",
        format!(
            "collateral ${:.6} (baseline ${:.6}, delta ${:+.6}); {} open orders; {} shares held",
            usd(collateral),
            usd(run.baseline_collateral),
            usd(collateral) - usd(run.baseline_collateral),
            open.len(),
            shares_left.to_f64()
        ),
    );

    let report = TeardownReport {
        open_orders: open.len(),
        collateral_drop_usd: usd(run.baseline_collateral) - usd(collateral),
        shares_left,
    };
    run.teardown = Some(report.clone());
    Ok(report)
}

/// A position the sequence left behind — a deep bid that filled while nobody was watching, or a
/// reverse that found no liquidity. ONE marketable exit is attempted, under the same money rules as
/// every other order, because Shaun's directive that the balance must not deplete outranks the
/// tidier rule that a teardown sends nothing.
///
/// Never fails: a teardown that gives up half way is worse than one that reports what it could not
/// do, so every failure lands in the journal and the last known holding comes back.
async fn flatten_residual(run: &mut Run, held: Qty) -> Qty {
    if held.0 <= 0 {
        return held;
    }
    if held < run.market.min_size {
        run.journal.record(
            "7/STRANDED",
            format!(
                "*** {} shares held, below the venue's {}-share minimum — UNFLATTENABLE BY RULE; \
                 the position rides to resolution ***",
                held.to_f64(),
                run.market.min_size.to_f64()
            ),
        );
        return held;
    }

    let book = match fetch_book(&run.market.token_id).await {
        Ok(book) => book,
        Err(failure) => {
            run.journal.record(
                "7/flatten",
                format!(
                    "*** {} shares held and /book failed: {failure:#} ***",
                    held.to_f64()
                ),
            );
            return held;
        }
    };
    let Some(bid) = book.best_bid else {
        run.journal.record(
            "7/flatten",
            format!(
                "*** {} shares held and there is no bid to sell into ***",
                held.to_f64()
            ),
        );
        return held;
    };

    let price = Price((bid.0 - run.market.tick.0 * TAKER_TICKS_THROUGH).max(run.market.tick.0));
    match run
        .place("7/flatten", Side::Sell, price, held, OrderStyle::Immediate)
        .await
    {
        Ok(outcome) => run
            .journal
            .record("7/flatten", format!("marketable exit: {outcome:?}")),
        Err(failure) => run.journal.record(
            "7/flatten",
            format!("the exit could not be sent: {failure:#}"),
        ),
    }

    let left = run
        .venue
        .share_balance(&run.market.token_id)
        .await
        .unwrap_or(held);
    if left.0 > 0 {
        run.journal.record(
            "7/STRANDED",
            format!(
                "*** {} shares of {} REMAIN HELD after the teardown exit — the position rides to \
                 resolution and the collateral delta will show it ***",
                left.to_f64(),
                run.market.token_id
            ),
        );
    }
    left
}

// ---------------------------------------------------------------------------------------------
// Run state
// ---------------------------------------------------------------------------------------------

struct Run {
    venue: Venue,
    market: BoundMarket,
    journal: Journal,
    checklist: Checklist,
    frames: Arc<Mutex<Vec<CapturedFrame>>>,
    stream_cursor: usize,
    subscribed_at: Option<TsUs>,
    heartbeat_log: Arc<Mutex<HeartbeatLog>>,
    heartbeat_task: Option<JoinHandle<()>>,
    stream_task: Option<JoinHandle<()>>,
    baseline_collateral: i64,
    is_geoblocked: bool,
    is_region_blocked: bool,
    resting: Vec<String>,
    position: Qty,
    next_client_id: u64,
    teardown: Option<TeardownReport>,
    mutating_answers: u32,
    rate_limited_answers: u32,
}

/// A placement the venue accepted, named by the id it minted for it.
#[derive(Debug, Clone)]
struct RestingOrder {
    client_id: ClientOrderId,
    venue_order_id: String,
    status: PlacementStatus,
    filled: Qty,
}

#[derive(Debug)]
enum Placement {
    Accepted(RestingOrder),
    Refused { detail: String },
}

/// What a stream wait saw. `unresolved` is the load-bearing half: frames that arrived and named an
/// order the correlation index could not map are not the same as no frames at all.
struct StreamWait {
    event: Option<StreamEvent>,
    unresolved: usize,
}

impl Run {
    fn new(prepared: Prepared, journal: Journal) -> Self {
        Self {
            venue: prepared.venue,
            market: prepared.market,
            journal,
            checklist: Checklist::new(),
            frames: Arc::new(Mutex::new(Vec::new())),
            stream_cursor: 0,
            subscribed_at: None,
            heartbeat_log: Arc::new(Mutex::new(HeartbeatLog::default())),
            heartbeat_task: None,
            stream_task: None,
            baseline_collateral: prepared.baseline_collateral,
            is_geoblocked: prepared.is_geoblocked,
            is_region_blocked: false,
            resting: Vec::new(),
            position: Qty(0),
            next_client_id: 1,
            teardown: None,
            mutating_answers: 0,
            rate_limited_answers: 0,
        }
    }

    fn encode(&self, request: ExecRequest) -> Result<EncodedRequest> {
        let context = self.venue.encode_context(self.venue.venue_now());
        encode_request(request, &context).context("encoding the request")
    }

    async fn place(
        &mut self,
        step: &str,
        side: Side,
        price: Price,
        qty: Qty,
        style: OrderStyle,
    ) -> Result<Placement> {
        enforce_money_rules(side, price, qty)?;
        assert_open_order_cap(self.resting.len() + 1)?;
        // A sell REDUCES exposure, and the tripwire exists to stop the balance falling — gating the
        // order that recovers the balance behind it would be the guard defeating its own purpose,
        // and would disarm the teardown flatten in exactly the case that needs it most.
        if side == Side::Buy {
            self.check_balance_tripwire().await?;
        }

        let client_id = ClientOrderId(self.next_client_id);
        self.next_client_id += 1;

        let request = self.encode(ExecRequest::Place {
            instrument: INSTRUMENT,
            client_id,
            side,
            price,
            qty,
            style,
        })?;
        self.journal.record(
            step,
            format!(
                "{side:?} {} @ {:.4} as {} (${:.4})",
                qty.to_f64(),
                price.to_f64(),
                style.as_str(),
                price.to_f64() * qty.to_f64()
            ),
        );
        let answer = self
            .venue
            .signed(&request)
            .await
            .context("sending the placement")?;
        self.journal.answer(step, &request.path, &answer);
        self.note_rate_limit(&answer);

        let outcome = {
            let context = self.venue.decode_context();
            decode_place(
                answer.answer(),
                &PlaceRequestContext {
                    instrument: INSTRUMENT,
                    client_id,
                    side,
                    price,
                    qty,
                },
                &context,
            )
            .context("decoding the placement answer")?
        };
        let outcome = match outcome {
            VenueAnswer::Unavailable(state) => {
                return Ok(Placement::Refused {
                    detail: format!("venue unavailable: {state:?}"),
                });
            }
            VenueAnswer::Answered(outcome) => outcome,
        };
        let Some(placed) = outcome.placed else {
            return Ok(Placement::Refused {
                detail: format!(
                    "http {} reject {:?}: {}",
                    answer.status,
                    outcome.event.reject,
                    answer.excerpt()
                ),
            });
        };
        self.venue
            .orders
            .record(
                &placed.venue_order_id,
                KnownOrder {
                    client_id,
                    instrument: INSTRUMENT,
                },
            )
            .context("recording the venue order id")?;
        let resting = RestingOrder {
            client_id,
            venue_order_id: placed.venue_order_id.to_string(),
            status: placed.status,
            filled: outcome.event.last_qty,
        };
        // Only a post-only order can rest. An `Immediate` fills or dies, so counting one against
        // the concurrency cap would refuse a later exit for a book entry that never existed —
        // including, in the worst ordering, the resting reverse that recovers a position.
        if style == OrderStyle::PostOnly && placed.status != PlacementStatus::Matched {
            self.resting.push(resting.venue_order_id.clone());
        }
        Ok(Placement::Accepted(resting))
    }

    fn resting_bid_price(&self, book: &BookSnapshot) -> Price {
        let tick = self.market.tick.0;
        let ceiling = Price((RESTING_BID_CEILING * FIXED_SCALE as f64).round() as i64);
        let deep = book.best_bid.map_or(ceiling, |bid| {
            Price((bid.0 - tick * RESTING_BID_TICKS_BELOW_TOUCH).min(ceiling.0))
        });
        let non_crossing = book
            .best_ask
            .map_or(deep, |ask| Price(deep.0.min(ask.0 - tick)));
        Price(non_crossing.0.max(tick))
    }

    async fn check_balance_tripwire(&mut self) -> Result<()> {
        let collateral = self
            .venue
            .collateral()
            .await
            .context("the balance tripwire read")?;
        let drop = usd(self.baseline_collateral) - usd(collateral);
        if drop >= BALANCE_TRIPWIRE_USD {
            self.journal.record(
                "!!TRIPWIRE",
                format!(
                    "collateral fell ${drop:.2} against a ${BALANCE_TRIPWIRE_USD:.2} limit — \
                     halting into teardown"
                ),
            );
            bail!("balance tripwire: collateral fell ${drop:.2}");
        }
        Ok(())
    }

    fn note_rate_limit(&mut self, answer: &ClobResponse) {
        self.mutating_answers += 1;
        if !answer.rate_limit.is_absent() {
            self.rate_limited_answers += 1;
        }
    }

    /// Decode captured frames through the PRODUCTION stream decoder and hand back the first that
    /// matches, from the cursor onward. An `UnknownOrder` frame is counted rather than ignored: the
    /// driver holds such frames because a mapping may still land, and a wait that only ever saw
    /// those has learned something quite different from one that saw nothing.
    async fn await_stream_event(
        &mut self,
        timeout: Duration,
        matches: impl Fn(&StreamEvent) -> bool,
    ) -> StreamWait {
        let deadline = Instant::now() + timeout;
        loop {
            let texts: Vec<String> = self
                .frames
                .lock()
                .expect("stream frames")
                .iter()
                .skip(self.stream_cursor)
                .map(|frame| frame.text.clone())
                .collect();
            let mut unresolved = 0;
            for (offset, text) in texts.iter().enumerate() {
                if text.trim() == "PONG" {
                    continue;
                }
                let context = self.venue.decode_context();
                match decode_stream_frame(text, &context) {
                    Ok(event) if matches(&event) => {
                        self.stream_cursor += offset + 1;
                        return StreamWait {
                            event: Some(event),
                            unresolved,
                        };
                    }
                    Ok(StreamEvent::Ignored(IgnoredReason::UnknownOrder)) => unresolved += 1,
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return StreamWait {
                    event: None,
                    unresolved,
                };
            }
            tokio::time::sleep(STREAM_POLL).await;
        }
    }

    /// Item 5: the subscription is marked ready on SEND, so what matters is whether the venue said
    /// anything at all — the keepalive answer aside — before the first order frame.
    fn assess_subscribe_ack(&mut self) {
        let before_order: Vec<String> = self
            .frames
            .lock()
            .expect("stream frames")
            .iter()
            .take_while(|frame| !frame.text.contains("\"event_type\""))
            .filter(|frame| frame.text.trim() != "PONG")
            .map(|frame| frame.text.clone())
            .collect();
        match before_order.is_empty() {
            true => self.checklist.verified(
                5,
                "the venue sent no acknowledgement between the subscribe and the first order \
                 frame — marking the subscription ready on send is correct",
            ),
            false => self.checklist.note(
                5,
                format!("the venue sent {before_order:?} before the first order frame"),
            ),
        }
    }

    /// Item 11: the owner filter that decides which entries of a trade's `maker_orders` are ours.
    async fn inspect_trade_lineage(&mut self) {
        let seen = self
            .await_stream_event(STREAM_WAIT, |event| matches!(event, StreamEvent::Trade(_)))
            .await;
        let Some(StreamEvent::Trade(lineage)) = seen.event else {
            self.checklist.note(
                11,
                "no trade frame arrived — the owner filter was never exercised",
            );
            return;
        };
        let raw = self.raw_frame_containing("\"event_type\":\"trade\"");
        self.journal
            .record("6/trade", format!("raw trade frame: {raw}"));
        self.journal.record(
            "6/trade",
            format!(
                "role {:?} settlement {:?} size {} price {} our maker fills {} taker order {:?} \
                 fee rate {}bps",
                lineage.role,
                lineage.settlement,
                lineage.size.to_f64(),
                lineage.price.to_f64(),
                lineage.maker_fills.len(),
                lineage.taker_order,
                lineage.fee_rate_bps
            ),
        );
        // We took, so the owner filter should attribute NOTHING to us as maker while still naming
        // our order as the taker. Any other shape means the filter reads the wrong field.
        match (lineage.maker_fills.is_empty(), lineage.taker_order) {
            (true, Some(_)) => self.checklist.verified(
                11,
                "owner-filtered maker_orders attributed nothing to us on a trade we took, and \
                 taker_order_id resolved to our own order",
            ),
            (false, _) => self.checklist.contradicted(
                11,
                format!(
                    "{} maker fills were attributed to us on a trade we TOOK — the owner filter \
                     matches the counterparty",
                    lineage.maker_fills.len()
                ),
            ),
            (true, None) => self.checklist.note(
                11,
                format!(
                    "the trade named neither our maker orders nor our taker order (role {:?})",
                    lineage.role
                ),
            ),
        }
    }

    fn raw_frame_containing(&self, needle: &str) -> String {
        self.frames
            .lock()
            .expect("stream frames")
            .iter()
            .rev()
            .find(|frame| frame.text.contains(needle))
            .map_or_else(|| "<none>".to_owned(), |frame| frame.text.clone())
    }

    fn stop_background_tasks(&mut self) {
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
    }

    fn print_report(&mut self) {
        let beats = self.heartbeat_log.lock().expect("heartbeat log").clone();
        let frames = self.frames.lock().expect("stream frames").len();
        let saw_pong = self
            .frames
            .lock()
            .expect("stream frames")
            .iter()
            .any(|frame| frame.text.trim() == "PONG");
        let first_frame_after_subscribe = self.first_frame_delay_ms();

        match saw_pong {
            true => self.checklist.verified(
                6,
                "the user channel answered PING with the literal text PONG",
            ),
            false => self
                .checklist
                .note(6, "no PONG arrived on the user channel within the run"),
        }
        match self.rate_limited_answers > 0 {
            true => self.checklist.verified(
                8,
                format!(
                    "{}/{} mutating answers carried Poly-RateLimit-* headers",
                    self.rate_limited_answers, self.mutating_answers
                ),
            ),
            false => self.checklist.note(
                8,
                format!(
                    "none of the {} mutating answers carried Poly-RateLimit-* headers — treating \
                     their absence as safe is correct",
                    self.mutating_answers
                ),
            ),
        }
        if !beats.stale_recoveries.is_empty() {
            self.checklist.verified(
                7,
                format!("stale-id recovery observed: {:?}", beats.stale_recoveries),
            );
        }
        self.checklist.note(
            9,
            "no order was left to reach resolution — the teardown cancels first, by design",
        );

        println!("\n==================== polymarket one-shot live run ====================");
        if self.is_region_blocked {
            println!(
                "VENUE-BLOCKED: the venue refused placement from this host. That is the probe's \
                 ANSWER, not a failure."
            );
        }
        println!(
            "window {} — the geoblock flag said blocked={}; the venue's own answer to a placement \
             is in the journal",
            self.market.slug, self.is_geoblocked
        );
        println!(
            "heartbeats: {} beats, last id {}, {} stale recoveries, {} failures",
            beats.beats,
            if beats.last_id.is_empty() { "-" } else { beats.last_id.as_str() },
            beats.stale_recoveries.len(),
            beats.failures.len()
        );
        println!(
            "user stream: {frames} frames captured, PONG seen: {saw_pong}, first frame {}ms after \
             subscribe",
            first_frame_after_subscribe.map_or_else(|| "n/a".to_owned(), |delay| delay.to_string())
        );
        if let Some(report) = &self.teardown {
            println!(
                "teardown: {} open orders, {} shares held, collateral delta ${:+.6}",
                report.open_orders,
                report.shares_left.to_f64(),
                -report.collateral_drop_usd
            );
            print_cost_decomposition(report, self.market.tick);
        }
        if self.position.0 > 0 {
            println!(
                "*** RESIDUAL POSITION: {} shares of {} — rides to resolution ***",
                self.position.to_f64(),
                self.market.token_id
            );
        }
        self.checklist.print();
        println!("journal: {JOURNAL_PATH}");
        println!("======================================================================\n");

        self.journal.record(
            "8/summary",
            format!(
                "{} verified, {} not observed, {} contradicted",
                self.checklist.count(Verdict::Verified),
                self.checklist.count(Verdict::NotObserved),
                self.checklist.count(Verdict::Contradicted)
            ),
        );
    }

    fn first_frame_delay_ms(&self) -> Option<i64> {
        let subscribed = self.subscribed_at?;
        let first = self
            .frames
            .lock()
            .expect("stream frames")
            .first()
            .map(|frame| frame.at)?;
        Some(first.diff(subscribed).micros() / 1_000)
    }
}

/// What the round trip SHOULD have cost, beside what it did. The venue publishes a fee rate that
/// contradicts itself on the wire (1000bps in one field, 0.07 in another), so the measured delta is
/// the only number here that is evidence — the rest is stated so the gap is visible.
fn print_cost_decomposition(report: &TeardownReport, tick: Price) {
    let spread_cost = tick.to_f64() * TAKER_TICKS_THROUGH as f64 * 2.0 * MAX_SHARES;
    println!(
        "cost decomposition: measured ${:.6}; crossing {TAKER_TICKS_THROUGH} ticks twice on \
         {MAX_SHARES} shares accounts for at most ~${spread_cost:.4} of spread, and the remainder \
         is venue fees — whose published rate is self-inconsistent, which is why this run measures \
         rather than predicts it",
        report.collateral_drop_usd
    );
    if report.shares_left.0 > 0 {
        println!(
            "  NOTE: {} shares are still held, so the delta is not a round-trip cost — it is a \
             cost plus an open position",
            report.shares_left.to_f64()
        );
    }
}

fn enforce_money_rules(side: Side, price: Price, qty: Qty) -> Result<()> {
    let size = qty.to_f64();
    let notional = price.to_f64() * size;
    if size > MAX_SHARES {
        bail!("{size} shares exceeds the {MAX_SHARES}-share cap");
    }
    if price.to_f64() <= 0.0 || price.to_f64() >= 1.0 {
        bail!("price {} is off the venue's (0,1) grid", price.to_f64());
    }
    // A sell converts inventory back to cash, so the notional cap guards the BUY side: it is the
    // only direction that can spend.
    if side == Side::Buy && notional > MAX_ORDER_NOTIONAL_USD {
        bail!("${notional:.2} exceeds the ${MAX_ORDER_NOTIONAL_USD:.2} per-order cap");
    }
    Ok(())
}

fn assert_open_order_cap(count: usize) -> Result<()> {
    match count > MAX_OPEN_ORDERS {
        true => bail!("{count} concurrent open orders exceeds the cap of {MAX_OPEN_ORDERS}"),
        false => Ok(()),
    }
}

fn reads_as_region_block(detail: &str) -> bool {
    let lowered = detail.to_ascii_lowercase();
    [
        "region",
        "geo",
        "blocked",
        "jurisdiction",
        "not available in",
        "restricted",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

struct Venue {
    http: ClobHttp,
    signer: RequestSigner,
    order_signer: OrderSigner,
    credentials: ApiCredentials,
    wallet: WalletIdentity,
    clock_offset_us: i64,
    tokens: TokenTable,
    orders: OrderIndex,
}

impl Venue {
    fn new(
        http: ClobHttp,
        credentials: ApiCredentials,
        key: SigningKey,
        wallet: WalletIdentity,
        clock_offset_us: i64,
    ) -> Result<Self> {
        let signer = RequestSigner::new(&credentials, wallet.signer)
            .context("building the l2 request signer")?;
        let order_signer = OrderSigner::new(OrderSignerSetup {
            key,
            maker: wallet.maker,
            signer: wallet.signer,
            signature_type: wallet.signature_type,
            api_key: credentials.api_key().to_owned(),
        });
        Ok(Self {
            http,
            signer,
            order_signer,
            credentials,
            wallet,
            clock_offset_us,
            tokens: TokenTable::with_retired_capacity(RETIRED_BINDING_CAPACITY),
            orders: OrderIndex::with_capacity(ORDER_INDEX_CAPACITY),
        })
    }

    fn bind(&mut self, binding: TokenBinding) {
        self.tokens.bind(binding);
    }

    fn venue_now(&self) -> TsUs {
        TsUs::from_micros(local_micros() + self.clock_offset_us)
    }

    async fn signed(&self, request: &EncodedRequest) -> Result<ClobResponse> {
        self.http
            .send_signed(&self.signer, request, self.venue_now().micros() / 1_000_000)
            .await
            .with_context(|| format!("signed call to {}", request.path))
    }

    async fn public(&self, request: &EncodedRequest) -> Result<ClobResponse> {
        self.http
            .send_public(request)
            .await
            .with_context(|| format!("public call to {}", request.path))
    }

    fn decode_context(&self) -> DecodeContext<'_> {
        DecodeContext {
            tokens: &self.tokens,
            orders: &self.orders,
            api_key: self.order_signer.api_key(),
            received_ts_us: self.venue_now(),
        }
    }

    fn encode_context(&self, sent: TsUs) -> EncodeContext<'_> {
        EncodeContext {
            tokens: &self.tokens,
            orders: &self.orders,
            signer: &self.order_signer,
            sent_ts_us: sent,
        }
    }

    async fn collateral(&self) -> Result<i64> {
        let request = collateral_balance(self.wallet.signature_type);
        let answer = self.signed(&request).await?;
        let context = self.decode_context();
        match decode_balance(answer.answer(), AssetId(0), &context)
            .context("decoding the collateral balance")?
        {
            VenueAnswer::Answered(balance) => Ok(balance.free),
            VenueAnswer::Unavailable(state) => {
                Err(anyhow!("the venue would not report collateral: {state:?}"))
            }
        }
    }

    async fn share_balance(&self, token_id: &str) -> Result<Qty> {
        let request = conditional_balance(token_id, self.wallet.signature_type);
        let answer = self.signed(&request).await?;
        let context = self.decode_context();
        match decode_balance(answer.answer(), AssetId(1), &context)
            .context("decoding the share balance")?
        {
            VenueAnswer::Answered(balance) => Ok(Qty(balance.free)),
            VenueAnswer::Unavailable(state) => Err(anyhow!(
                "the venue would not report the share balance: {state:?}"
            )),
        }
    }

    /// The venue's own ids, straight from the page. Deliberately NOT routed through the order
    /// decoder, which drops anything this run cannot name — and an order a teardown cannot name is
    /// precisely the one it must see.
    async fn open_order_ids(&self) -> Result<Vec<String>> {
        let request = open_orders_page(None);
        let answer = self.signed(&request).await?;
        if !answer.is_success() {
            bail!(
                "/data/orders answered http {}: {}",
                answer.status,
                answer.excerpt()
            );
        }
        let page: serde_json::Value =
            serde_json::from_str(&answer.body).context("parsing /data/orders")?;
        let rows = page
            .get("data")
            .and_then(serde_json::Value::as_array)
            .or_else(|| page.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows
            .iter()
            .filter_map(|row| row.get("id").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect())
    }
}

struct BookSnapshot {
    bids: Vec<(Price, f64)>,
    asks: Vec<(Price, f64)>,
    best_bid: Option<Price>,
    best_ask: Option<Price>,
}

impl BookSnapshot {
    fn describe(&self) -> String {
        format!(
            "{} — {} bid levels, {} ask levels",
            self.describe_touch(),
            self.bids.len(),
            self.asks.len()
        )
    }

    fn describe_touch(&self) -> String {
        let show = |price: Option<Price>| {
            price.map_or_else(|| "—".to_owned(), |price| format!("{:.3}", price.to_f64()))
        };
        format!("touch {} / {}", show(self.best_bid), show(self.best_ask))
    }

    fn depth_within(&self, side: Side, band: i64) -> f64 {
        match side {
            Side::Buy => self.best_bid.map_or(0.0, |best| {
                self.bids
                    .iter()
                    .filter(|(price, _)| price.0 >= best.0 - band)
                    .map(|(_, size)| size)
                    .sum()
            }),
            Side::Sell => self.best_ask.map_or(0.0, |best| {
                self.asks
                    .iter()
                    .filter(|(price, _)| price.0 <= best.0 + band)
                    .map(|(_, size)| size)
                    .sum()
            }),
        }
    }

    fn inside_spread_sell(&self, tick: Price) -> Option<Price> {
        let (bid, ask) = (self.best_bid?, self.best_ask?);
        let inside = Price(ask.0 - tick.0);
        (inside.0 > bid.0).then_some(inside)
    }
}

async fn fetch_book(token_id: &str) -> Result<BookSnapshot> {
    let rest = PolyRest::new(PolySeries::BtcUpDown5m).context("building the book client")?;
    let (status, body) = rest
        .fetch_status_and_text(&book_url(token_id))
        .await
        .context("reading /book")?;
    if status != 200 {
        bail!("/book answered http {status}");
    }
    let raw: RawBook = serde_json::from_str(&body).context("parsing /book")?;
    let levels = |rows: &[RawLevel]| -> Vec<(Price, f64)> {
        rows.iter()
            .filter_map(|row| {
                let price = row.price.parse::<f64>().ok()?;
                let size = row.size.parse::<f64>().ok()?;
                Some((Price((price * FIXED_SCALE as f64).round() as i64), size))
            })
            .collect()
    };
    let bids = levels(&raw.bids);
    let asks = levels(&raw.asks);
    let best_bid = bids.iter().map(|(price, _)| *price).max();
    let best_ask = asks.iter().map(|(price, _)| *price).min();
    Ok(BookSnapshot {
        bids,
        asks,
        best_bid,
        best_ask,
    })
}

#[derive(serde::Deserialize)]
struct RawBook {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
}

#[derive(serde::Deserialize)]
struct RawLevel {
    price: String,
    size: String,
}

type UserSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct CapturedFrame {
    at: TsUs,
    text: String,
}

async fn serve_user_stream(
    mut writer: SplitSink<UserSocket, Message>,
    mut reader: SplitStream<UserSocket>,
    frames: Arc<Mutex<Vec<CapturedFrame>>>,
) {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            frame = reader.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    println!("  [user stream] {text}");
                    frames.lock().expect("stream frames").push(CapturedFrame {
                        at: TsUs::from_micros(local_micros()),
                        text: text.to_string(),
                    });
                }
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(failure)) => {
                    println!("  [user stream] transport failure: {failure}");
                    return;
                }
            },
            _ = ping.tick() => {
                if writer.send(Message::Text("PING".into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct HeartbeatLog {
    beats: u64,
    last_id: String,
    stale_recoveries: Vec<String>,
    failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    NotObserved,
    Verified,
    Contradicted,
}

struct ChecklistRow {
    id: u8,
    question: &'static str,
    verdict: Verdict,
    evidence: String,
}

struct Checklist {
    rows: Vec<ChecklistRow>,
}

impl Checklist {
    fn new() -> Self {
        const QUESTIONS: [(u8, &str); 11] = [
            (1, "venue ACCEPTS a signatureType-2 order"),
            (2, "place-answer orderID == the id user-stream frames use"),
            (3, "POST /order blocks >=250ms and can answer `delayed`"),
            (4, "a cancel inside the taker hold is refused"),
            (5, "user-stream subscribe accepted with no ack"),
            (6, "PONG is the literal keepalive reply on the user channel"),
            (
                7,
                "heartbeat chain: an empty id starts it; a stale id carries the expected one",
            ),
            (8, "Poly-RateLimit-* headers on MUTATING responses"),
            (9, "auto-cancel at market resolution"),
            (10, "/data/orders scoping: api key vs maker"),
            (11, "maker_orders[].owner == our api key"),
        ];
        Self {
            rows: QUESTIONS
                .iter()
                .map(|(id, question)| ChecklistRow {
                    id: *id,
                    question,
                    verdict: Verdict::NotObserved,
                    evidence: String::new(),
                })
                .collect(),
        }
    }

    fn verified(&mut self, id: u8, evidence: impl Into<String>) {
        self.set(id, Verdict::Verified, evidence.into());
    }

    fn contradicted(&mut self, id: u8, evidence: impl Into<String>) {
        self.set(id, Verdict::Contradicted, evidence.into());
    }

    fn note(&mut self, id: u8, evidence: impl Into<String>) {
        let evidence = evidence.into();
        if let Some(row) = self.row(id)
            && row.verdict == Verdict::NotObserved
        {
            row.evidence = evidence;
        }
    }

    fn set(&mut self, id: u8, verdict: Verdict, evidence: String) {
        let Some(row) = self.row(id) else {
            return;
        };
        if row.verdict == Verdict::Contradicted && verdict != Verdict::Contradicted {
            return;
        }
        row.verdict = verdict;
        row.evidence = evidence;
    }

    fn row(&mut self, id: u8) -> Option<&mut ChecklistRow> {
        self.rows.iter_mut().find(|row| row.id == id)
    }

    fn count(&self, verdict: Verdict) -> usize {
        self.rows
            .iter()
            .filter(|row| row.verdict == verdict)
            .count()
    }

    fn contradicted_ids(&self) -> Vec<u8> {
        self.rows
            .iter()
            .filter(|row| row.verdict == Verdict::Contradicted)
            .map(|row| row.id)
            .collect()
    }

    fn print(&self) {
        println!("\n  what fixtures could not prove — eleven questions, one run:");
        for row in &self.rows {
            let verdict = match row.verdict {
                Verdict::Verified => "VERIFIED    ",
                Verdict::NotObserved => "NOT-OBSERVED",
                Verdict::Contradicted => "CONTRADICTED",
            };
            println!("   {:>2}. {verdict}  {}", row.id, row.question);
            if !row.evidence.is_empty() {
                println!("       {}", row.evidence);
            }
        }
    }
}

struct Journal {
    started: Instant,
}

impl Journal {
    fn open() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    fn banner(&mut self) {
        self.append(&format!(
            "\n## live run at unix {} (pid {})\n",
            unix_now_s(),
            std::process::id()
        ));
        println!("\n== polymarket one-shot live execution run — journal at {JOURNAL_PATH} ==");
    }

    fn record(&mut self, step: &str, detail: impl Display) {
        let line = format!(
            "- `{:>7.2}s` **{step}** {detail}",
            self.started.elapsed().as_secs_f64()
        );
        println!("{line}");
        self.append(&format!("{line}\n"));
    }

    fn answer(&mut self, step: &str, path: &str, response: &ClobResponse) {
        self.record(
            step,
            format!(
                "{path} -> http {} rate-limit[{} / {}] body: {}",
                response.status,
                response
                    .rate_limit
                    .remaining
                    .map_or_else(|| "-".to_owned(), |budget| budget.to_string()),
                response.rate_limit.warning.as_deref().unwrap_or("-"),
                response.body
            ),
        );
    }

    fn append(&self, text: &str) {
        let path = std::path::Path::new(JOURNAL_PATH);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(text.as_bytes());
        }
    }
}

fn local_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_micros() as i64
}

fn unix_now_s() -> i64 {
    local_micros() / 1_000_000
}

fn shares(count: f64) -> Qty {
    Qty((count * FIXED_SCALE as f64).round() as i64)
}

fn usd(mantissa: i64) -> f64 {
    mantissa as f64 / FIXED_SCALE as f64
}
