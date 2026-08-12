//! ONE-SHOT sell-only recovery. A live proving run bought a real Polymarket long and a decode bug
//! stopped its sequence before the auto-reverse fired, so a position rides on the venue. This tool
//! flattens THAT ONE position and nothing else, through the same production signing, encoding and
//! transport the reverse leg would have used.
//!
//! WHY IT IS SEPARATE FROM THE TEST. The reverse lives inside `tests/integration/poly_exec.rs`,
//! which is `#[ignore]`d forever and re-runs the whole place/cancel/replace sequence — sending
//! fresh orders to answer questions already answered. Recovery needs the SELL half alone, so it is
//! lifted here rather than by re-arming a run that would open a new position first.
//!
//! THE RULES ARE STRUCTURAL. It encodes SELL orders only — a Buy is an assertion failure, not a
//! branch. It sends at most ONE order: a marketable fill-and-kill of the exact live balance. It
//! does not rest a follow-up if that sell leaves a residue — these windows resolve in minutes,
//! so a stranded remainder self-resolves, and resting into a tearing-down book is fresh tail
//! risk for a
//! few cents. It reads the exact held balance and never sells more than that. It refuses to sell
//! before the CLOB allowance cache is warmed. It reads the venue as ground truth at every step: a
//! balance of zero means the position already settled and nothing is sent. The placement answer is
//! decoded from raw JSON on purpose — `decode_place` mis-parses this venue's decimal-dollar
//! amount fields (`"2.549999"`), which is the very bug that stranded the position, so
//! success/status/orderID are read straight off the wire and the fill is confirmed by re-reading
//! the balance instead.
//!
//! Run: `cargo run --example poly-recover`

use std::fmt::Display;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};

use polysim::adapters::exec::ExecRequest;
use polysim::adapters::polymarket::exec::codec::{
    DecodeContext, EncodeContext, EncodedRequest, OrderIndex, OrderSigner, OrderSignerSetup,
    TokenBinding, TokenTable, VenueAnswer, collateral_balance, conditional_allowance_refresh,
    conditional_balance, decode_balance, encode_request,
};
use polysim::adapters::polymarket::exec::handle::{WalletIdentity, preflight_polymarket};
use polysim::adapters::polymarket::exec::rest::{ClobHttp, ClobResponse};
use polysim::adapters::polymarket::exec::sign::key::SigningKey;
use polysim::adapters::polymarket::exec::sign::l2::{ApiCredentials, RequestSigner};
use polysim::adapters::polymarket::rest::{PolyRest, book_url};
use polysim::config::PolySeries;
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::OrderStyle;
use polysim::time::TsUs;

/// The same trail the live test appends to, so a hand recovery and the run that stranded the
/// position read as one story in one file.
const JOURNAL_PATH: &str = ".work/poly-exec-live-run.md";

/// The stranded long, named by the account owner's mandate: the Up token of one resolved-ish
/// up/down window, held ~5.42553 shares against a collapsed book.
const TOKEN_ID: &str =
    "403775984311567090546314745286282446513391197278476249786729827485793360335";
const INSTRUMENT: InstrumentId = InstrumentId(0);

/// This venue's grid for the market: a one-cent tick, and a five-share floor below which no
/// order is accepted — which is also the floor below which a residual holding is unflattenable
/// by rule.
const TICK_USD: f64 = 0.01;
const MIN_SHARES: f64 = 5.0;

/// How far through the touch the marketable exit reaches. Two ticks, not one: the book moves
/// between the read and the send, and a sell that fails to cross recovers nothing.
const TAKER_TICKS_THROUGH: i64 = 2;

/// Collateral before this position was opened, from the run that opened it — the number the
/// delta is measured against.
const BASELINE_COLLATERAL_USD: f64 = 74.368099;

const ORDER_INDEX_CAPACITY: usize = 8;
const RETIRED_BINDING_CAPACITY: usize = 2;

#[tokio::main]
async fn main() -> Result<()> {
    let mut journal = Journal::open();
    journal.banner();
    let outcome = recover(&mut journal).await;
    if let Err(failure) = &outcome {
        journal.record("recover", format!("STOPPED before flat: {failure:#}"));
        eprintln!("poly-recover stopped: {failure:#}");
    }
    outcome
}

async fn recover(journal: &mut Journal) -> Result<()> {
    let preflight = match preflight_polymarket().await {
        Ok(preflight) => preflight,
        Err(failure) => {
            // A preflight failure has sent nothing, so there is nothing to undo — report and stop
            // clean rather than surface as a crash needing intervention.
            journal.record(
                "0/preflight",
                format!("preflight failed, NOTHING SENT: {failure:#}"),
            );
            println!("preflight failed — nothing sent, nothing to recover: {failure:#}");
            return Ok(());
        }
    };
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
        ClobHttp::new(Duration::from_secs(10), Duration::from_secs(30))
            .context("building the polymarket http client")?,
        preflight.credentials,
        preflight.key,
        preflight.wallet,
        preflight.venue_clock_offset.micros(),
    )?;
    let tick = Price((TICK_USD * FIXED_SCALE as f64).round() as i64);
    let min_size = Qty((MIN_SHARES * FIXED_SCALE as f64).round() as i64);
    venue.bind(TokenBinding {
        instrument: INSTRUMENT,
        token_id: TOKEN_ID.into(),
        tick,
        is_neg_risk: false,
    });

    let collateral_before = venue.collateral().await.context("collateral before")?;
    journal.record(
        "0/collateral-before",
        format!(
            "collateral ${:.6} (baseline ${BASELINE_COLLATERAL_USD:.6}, delta ${:+.6})",
            usd(collateral_before),
            usd(collateral_before) - BASELINE_COLLATERAL_USD
        ),
    );

    let held = venue
        .share_balance(TOKEN_ID)
        .await
        .context("share balance of the stranded token")?;
    journal.record(
        "1/held",
        format!("the venue says {} shares held", held.to_f64()),
    );
    if held.0 <= 0 {
        journal.record(
            "1/flat",
            "already flat, nothing to sell — the position settled or resolved",
        );
        println!("already flat: 0 shares held for {TOKEN_ID} — nothing to sell");
        return Ok(());
    }
    if held < min_size {
        journal.record(
            "1/STRANDED",
            format!(
                "*** {} shares held, below the venue's {MIN_SHARES}-share minimum — UNFLATTENABLE \
                 BY RULE; the position rides to resolution ***",
                held.to_f64()
            ),
        );
        println!(
            "STRANDED: {} shares is below the {MIN_SHARES}-share venue minimum — cannot be sold, \
             rides to resolution",
            held.to_f64()
        );
        return Ok(());
    }

    // The chain being approved is not enough: the CLOB caches allowances per token, and an
    // unrefreshed cache rejects a sell as an empty wallet. A SELL must never be sent before
    // it wins.
    let refresh = conditional_allowance_refresh(TOKEN_ID, venue.wallet.signature_type);
    let refreshed = venue
        .signed(&refresh)
        .await
        .context("warming the conditional allowance cache")?;
    journal.answer("2/allowance", &refresh.path, &refreshed);
    if !refreshed.is_success() {
        bail!(
            "the conditional allowance refresh failed (http {}): {} — refusing to sell",
            refreshed.status,
            refreshed.excerpt()
        );
    }

    let book = match fetch_book(TOKEN_ID).await {
        Ok(book) => book,
        Err(failure) => {
            // A book that will not read is a book that cannot be sold into — most likely the
            // market has torn down. Nothing has been sent; report and stop clean.
            journal.record(
                "3/book",
                format!(
                    "/book unreadable ({failure:#}) — market resolved or torn down; NOTHING SENT"
                ),
            );
            println!("/book unreadable — market resolved or torn down, nothing sent: {failure:#}");
            return Ok(());
        }
    };
    journal.record("3/book", book.describe());
    let Some(best_bid) = book.best_bid else {
        journal.record(
            "3/STRANDED",
            format!(
                "*** {} shares held and no bid to sell into — the position rides to resolution ***",
                held.to_f64()
            ),
        );
        println!(
            "no bid on the book — nothing to sell into; {} shares ride to resolution",
            held.to_f64()
        );
        return Ok(());
    };

    let sell_price = Price((best_bid.0 - tick.0 * TAKER_TICKS_THROUGH).max(tick.0));
    let sold = venue
        .place_sell(journal, "3/sell", sell_price, held, OrderStyle::Immediate)
        .await?;
    journal.record(
        "3/sell",
        format!(
            "marketable FAK sell of {} shares @ {:.4}: success={} status={} orderID={}",
            held.to_f64(),
            sell_price.to_f64(),
            sold.success,
            sold.status,
            sold.order_id
        ),
    );

    let remaining = venue
        .share_balance(TOKEN_ID)
        .await
        .context("share balance after the marketable sell")?;
    journal.record(
        "3/after-sell",
        format!("the venue says {} shares left", remaining.to_f64()),
    );
    match remaining.0 > 0 {
        // One order only. A residue is left to self-resolve rather than chased with a resting sell:
        // these windows settle in minutes, so there is no overnight risk to contain, and resting
        // into a tearing-down book is fresh tail risk for a few cents of recovery.
        true => journal.record(
            "3/STRANDED",
            format!(
                "*** {} shares REMAIN after the marketable FAK sell — NO follow-up order by design; \
                 the position rides to resolution (this window settles in minutes) ***",
                remaining.to_f64()
            ),
        ),
        false => journal.record("3/flat", "the position is flat"),
    }

    finalise(&venue, journal, remaining).await
}

async fn finalise(venue: &Venue, journal: &mut Journal, shares_left: Qty) -> Result<()> {
    let collateral = venue.collateral().await.context("collateral after")?;
    journal.record(
        "5/final",
        format!(
            "collateral ${:.6} (baseline ${BASELINE_COLLATERAL_USD:.6}, delta ${:+.6}); {} shares held",
            usd(collateral),
            usd(collateral) - BASELINE_COLLATERAL_USD,
            shares_left.to_f64()
        ),
    );
    match shares_left.0 <= 0 {
        true => println!(
            "FLAT: 0 shares held; collateral ${:.6} (delta ${:+.6} vs baseline)",
            usd(collateral),
            usd(collateral) - BASELINE_COLLATERAL_USD
        ),
        false => println!(
            "NOT FLAT: {} shares still held; collateral ${:.6} (delta ${:+.6} vs baseline)",
            shares_left.to_f64(),
            usd(collateral),
            usd(collateral) - BASELINE_COLLATERAL_USD
        ),
    }
    Ok(())
}

struct Venue {
    http: ClobHttp,
    signer: RequestSigner,
    order_signer: OrderSigner,
    wallet: WalletIdentity,
    clock_offset_us: i64,
    tokens: TokenTable,
    orders: OrderIndex,
    next_client_id: u64,
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
            wallet,
            clock_offset_us,
            tokens: TokenTable::with_retired_capacity(RETIRED_BINDING_CAPACITY),
            orders: OrderIndex::with_capacity(ORDER_INDEX_CAPACITY),
            next_client_id: 1,
        })
    }

    fn bind(&mut self, binding: TokenBinding) {
        self.tokens.bind(binding);
    }

    /// The venue's clock, not ours: the signed order's millisecond stamp is this venue's uniqueness
    /// key and `POLY_TIMESTAMP` has no documented staleness tolerance.
    fn venue_now(&self) -> TsUs {
        TsUs::from_micros(local_micros() + self.clock_offset_us)
    }

    fn encode(&self, request: ExecRequest) -> Result<EncodedRequest> {
        encode_request(request, &self.encode_context(self.venue_now())).context("encoding")
    }

    /// Every order this tool sends goes through here, where the SELL-only rule is a hard assertion
    /// rather than a branch a caller could forget. The placement answer is returned raw: this venue
    /// reports amounts as decimal-dollar strings the production decoder rejects, and the fill is
    /// confirmed by re-reading the balance, never by trusting these fields.
    async fn place_sell(
        &mut self,
        journal: &mut Journal,
        step: &str,
        price: Price,
        qty: Qty,
        style: OrderStyle,
    ) -> Result<PlacedRaw> {
        assert!(
            qty.0 > 0,
            "a sell of {} shares is not an order",
            qty.to_f64()
        );
        let client_id = ClientOrderId(self.next_client_id);
        self.next_client_id += 1;

        let request = self.encode(ExecRequest::Place {
            instrument: INSTRUMENT,
            client_id,
            side: Side::Sell,
            price,
            qty,
            style,
        })?;
        journal.record(
            step,
            format!(
                "SELL {} @ {:.4} as {} (${:.4})",
                qty.to_f64(),
                price.to_f64(),
                style.as_str(),
                price.to_f64() * qty.to_f64()
            ),
        );
        let answer = self.signed(&request).await.context("sending the sell")?;
        journal.answer(step, &request.path, &answer);
        Ok(PlacedRaw::parse(&answer.body))
    }

    async fn signed(&self, request: &EncodedRequest) -> Result<ClobResponse> {
        self.http
            .send_signed(&self.signer, request, self.venue_now().micros() / 1_000_000)
            .await
            .with_context(|| format!("signed call to {}", request.path))
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
        match decode_balance(answer.answer(), AssetId(0), &self.decode_context())
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
        match decode_balance(answer.answer(), AssetId(1), &self.decode_context())
            .context("decoding the share balance")?
        {
            VenueAnswer::Answered(balance) => Ok(Qty(balance.free)),
            VenueAnswer::Unavailable(state) => Err(anyhow!(
                "the venue would not report the share balance: {state:?}"
            )),
        }
    }
}

/// A placement answer read straight off the wire. `decode_place` is deliberately not used — it
/// rejects this venue's decimal-dollar amount fields, which is the bug that stranded the position.
struct PlacedRaw {
    success: bool,
    status: String,
    order_id: String,
}

impl PlacedRaw {
    fn parse(body: &str) -> Self {
        let value: serde_json::Value =
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        let string = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            success: value
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            status: string("status"),
            order_id: string("orderID"),
        }
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
        let show = |price: Option<Price>| {
            price.map_or_else(|| "—".to_owned(), |price| format!("{:.3}", price.to_f64()))
        };
        format!(
            "touch {} / {} — {} bid levels, {} ask levels",
            show(self.best_bid),
            show(self.best_ask),
            self.bids.len(),
            self.asks.len()
        )
    }
}

/// The venue sends bids ascending and asks descending, so neither best is the first element. Taken
/// by extremum instead, which is right whatever order the venue chooses next.
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
            "\n## sell-only recovery at unix {} (pid {})\n",
            unix_now_s(),
            std::process::id()
        ));
        println!("== polymarket sell-only recovery — journal at {JOURNAL_PATH} ==");
    }

    fn record(&mut self, step: &str, detail: impl Display) {
        let line = format!(
            "- `{:>7.2}s` **{step}** {detail}",
            self.started.elapsed().as_secs_f64()
        );
        println!("{line}");
        self.append(&format!("{line}\n"));
    }

    /// A venue answer, VERBATIM. Truncating here would lose exactly the wording a future reader
    /// needs — a refusal's own text is what tells one failure from another.
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

fn usd(mantissa: i64) -> f64 {
    mantissa as f64 / FIXED_SCALE as f64
}
