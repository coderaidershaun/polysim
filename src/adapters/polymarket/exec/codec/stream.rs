//! User-stream decoder. It is account-wide and replays nothing after a disconnect; reads rebuild
//! state and only then resume.
//!
//! Maker fills fold from order UPDATE, whose size_matched is cumulative, so duplicate or reordered
//! frames are harmless. Trade frames are never folded. The same trade id arrives once per
//! settlement step carrying lineage, fee, and progress only.

use crate::adapters::venue_clock::clamp_exchange_ts;
use crate::ids::{ClientOrderId, InstrumentId, Price, Qty, TradeId};
use crate::msg::exec::{ExecEvent, ExecKind, Liquidity};
use crate::time::TsUs;

use super::correlation::{trade_id_digest, venue_order_id_digest};
use super::response::blank_event;
use super::wire::{OrderEvent, StreamFrame, TradeRecord};
use super::{
    DecodeContext, FRAME, WireError, optional_qty, order_side, price_of, qty_of, status_with_fill,
    venue_status,
};

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Order(ExecEvent),
    Trade(TradeLineage),
    Ignored(IgnoredReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IgnoredReason {
    /// Dropped at edge because ExecEvent promises an instrument. May not discard mid-run.
    UntrackedToken,
    /// Order id never recorded. Not discardable; the answer may still arrive.
    UnknownOrder,
    UnhandledEvent,
}

/// Failed is terminal. An already-folded fill did not settle on-chain, so the position the
/// engine believes it holds does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradeSettlement {
    Matched,
    MatchedNotBroadcast,
    Mined,
    Confirmed,
    Retrying,
    Failed,
}

impl TradeSettlement {
    pub const fn is_terminal(self) -> bool {
        matches!(self, TradeSettlement::Confirmed | TradeSettlement::Failed)
    }

    /// Whether the venue has put the trade on chain. Only then have the balances behind it actually
    /// moved: a match is an off-chain promise, and the balance endpoint answers pre-fill numbers
    /// until the transfer lands.
    pub const fn is_on_chain(self) -> bool {
        matches!(self, TradeSettlement::Mined | TradeSettlement::Confirmed)
    }
}

/// Carries no quantity; it was already folded from the order UPDATE. Dedupe on venue_trade_id,
/// not on the digest.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeLineage {
    pub instrument: InstrumentId,
    pub trade_id: TradeId,
    /// Dedupe on this, not on digest.
    pub venue_trade_id: Box<str>,
    pub settlement: TradeSettlement,
    /// Whether the venue attributes the trade to this engine's own credential. Decided from the
    /// payload alone, so it still holds for a settlement step arriving long after the order it
    /// names went terminal and stopped being resolvable.
    pub is_ours: bool,
    /// The record's side is the taker's side regardless of who we are. Only role tells us which we played.
    pub role: Option<Liquidity>,
    pub price: Price,
    pub size: Qty,
    /// Our own resting orders that this trade filled, filtered by owner. Empty when we took.
    pub maker_fills: Vec<MakerFill>,
    /// Our order, when we took. The taker_order_id is only ours if trader_side says so.
    pub taker_order: Option<ClientOrderId>,
    /// The venue charges takers only, and publishes a rate rather than an amount. Only the rate
    /// travels; turning it into money here would put a float where an exact amount belongs.
    pub fee_rate_bps: i32,
    pub exchange_ts_us: TsUs,
    pub received_ts_us: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MakerFill {
    pub client_id: ClientOrderId,
    pub price: Price,
    pub matched: Qty,
    pub fee_rate_bps: i32,
}

/// # Errors
/// Malformed JSON, a decimal out of scale, or an unknown status, side or event spelling.
pub fn decode_stream_frame(
    json: &str,
    context: &DecodeContext<'_>,
) -> Result<StreamEvent, WireError> {
    let frame: StreamFrame = FRAME.decode(json)?;
    match frame {
        StreamFrame::Order(event) => decode_order_event(&event, context),
        StreamFrame::Trade(record) => Ok(match trade_lineage(&record, context)? {
            Some(lineage) => StreamEvent::Trade(lineage),
            None => StreamEvent::Ignored(IgnoredReason::UntrackedToken),
        }),
        StreamFrame::Unhandled => Ok(StreamEvent::Ignored(IgnoredReason::UnhandledEvent)),
    }
}

fn decode_order_event(
    event: &OrderEvent,
    context: &DecodeContext<'_>,
) -> Result<StreamEvent, WireError> {
    let Some(instrument) = context.tokens.instrument(&event.asset_id) else {
        return Ok(StreamEvent::Ignored(IgnoredReason::UntrackedToken));
    };
    let Some(known) = context.orders.resolve(&event.id) else {
        return Ok(StreamEvent::Ignored(IgnoredReason::UnknownOrder));
    };

    let kind = order_event_kind(&event.event)?;
    let qty = qty_of("original_size", &event.original_size)?;
    let filled = optional_qty("size_matched", &event.size_matched)?;
    let price = price_of("price", &event.price)?;

    Ok(StreamEvent::Order(ExecEvent {
        instrument,
        venue_order_id: Some(venue_order_id_digest(&event.id)),
        kind,
        status: Some(status_with_fill(
            venue_status("status", &event.status)?,
            filled,
            qty,
        )),
        // Only UPDATE reports matched quantity, and it does so as a maker. The taker's own fill is
        // reported by placement instead, so we don't double-count.
        liquidity: matches!(kind, ExecKind::ReportTrade).then_some(Liquidity::Maker),
        side: order_side("side", &event.side)?,
        price,
        qty,
        cumulative_qty: filled,
        // A resting order fills at its own limit, exactly.
        cumulative_quote: price.notional(filled),
        exchange_ts_us: stream_stamp(&event.timestamp, context),
        ..blank_event(instrument, known.client_id, context)
    }))
}

fn order_event_kind(event: &str) -> Result<ExecKind, WireError> {
    Ok(match event {
        "PLACEMENT" => ExecKind::ReportNew,
        "UPDATE" => ExecKind::ReportTrade,
        "CANCELLATION" => ExecKind::ReportCanceled,
        unknown => {
            return Err(WireError::UnknownEnum {
                field: "type",
                value: unknown.into(),
            });
        }
    })
}

/// Shared by the stream and `GET /data/trades` — the payload is the same shape on both.
///
/// `Ok(None)` for a token no binding names.
pub(super) fn trade_lineage(
    record: &TradeRecord,
    context: &DecodeContext<'_>,
) -> Result<Option<TradeLineage>, WireError> {
    let Some(instrument) = context.tokens.instrument(&record.asset_id) else {
        return Ok(None);
    };
    let role = trader_role(&record.trader_side);

    // Filter maker_orders to only those where owner matches our api key. Taking the whole array
    // would credit the counterparty's fills to us on any trade we took.
    let mut maker_fills = Vec::new();
    let mut has_our_maker_leg = false;
    for maker in &record.maker_orders {
        if maker.owner != context.api_key {
            continue;
        }
        // Kept apart from the fills below because the order id stops resolving once the order goes
        // terminal, and the credential on the leg does not.
        has_our_maker_leg = true;
        let Some(known) = context.orders.resolve(&maker.order_id) else {
            continue;
        };
        maker_fills.push(MakerFill {
            client_id: known.client_id,
            price: price_of("maker price", &maker.price)?,
            matched: qty_of("matched_amount", &maker.matched_amount)?,
            fee_rate_bps: bps(&maker.fee_rate_bps),
        });
    }

    // taker_order_id names our order only when we were taker; on maker trade it's counterparty's.
    let taker_order = match role {
        Some(Liquidity::Taker) => context
            .orders
            .resolve(&record.taker_order_id)
            .map(|known| known.client_id),
        _ => None,
    };

    Ok(Some(TradeLineage {
        instrument,
        trade_id: trade_id_digest(&record.id),
        venue_trade_id: record.id.clone().into(),
        settlement: settlement(&record.status)?,
        is_ours: is_ours(role, has_our_maker_leg),
        role,
        price: price_of("price", &record.price)?,
        size: qty_of("size", &record.size)?,
        maker_fills,
        taker_order,
        fee_rate_bps: bps(&record.fee_rate_bps),
        // match_time is in seconds here, unlike the order event which stamps milliseconds.
        exchange_ts_us: seconds_stamp(&record.match_time, context),
        received_ts_us: context.received_ts_us,
    }))
}

/// Whether this trade belongs to the credential this engine signs with.
///
/// Every maker leg names the api key that placed it, so a trade the venue says we MADE with none of
/// ours among those legs is the venue stating it belongs to someone else — the same wallet is
/// reachable from the website under different credentials. Nothing names the credential behind the
/// taker leg, and an unstated role names nothing at all, so neither can be shown to belong to a
/// stranger and both count as ours. That asymmetry is deliberate: the answer decides whether a
/// failed settlement stops the run, and a fill this engine cannot place must stop it rather than be
/// waved through as somebody else's.
fn is_ours(role: Option<Liquidity>, has_our_maker_leg: bool) -> bool {
    match role {
        Some(Liquidity::Maker) => has_our_maker_leg,
        Some(Liquidity::Taker) | None => true,
    }
}

fn trader_role(trader_side: &str) -> Option<Liquidity> {
    match trader_side {
        "MAKER" => Some(Liquidity::Maker),
        "TAKER" => Some(Liquidity::Taker),
        _ => None,
    }
}

/// The REST surface sends bare names where the stream and the TypeScript SDK prefix them.
fn settlement(status: &str) -> Result<TradeSettlement, WireError> {
    Ok(
        match status.strip_prefix("TRADE_STATUS_").unwrap_or(status) {
            "MATCHED" => TradeSettlement::Matched,
            "MATCHED_NOT_BROADCASTED" => TradeSettlement::MatchedNotBroadcast,
            "MINED" => TradeSettlement::Mined,
            "CONFIRMED" => TradeSettlement::Confirmed,
            "RETRYING" => TradeSettlement::Retrying,
            "FAILED" => TradeSettlement::Failed,
            unknown => {
                return Err(WireError::UnknownEnum {
                    field: "trade status",
                    value: unknown.into(),
                });
            }
        },
    )
}

/// Fee rate, not money. An absent field reads as zero because the venue omits it on free markets.
fn bps(text: &str) -> i32 {
    text.trim().parse::<i32>().unwrap_or(0)
}

fn stream_stamp(millis: &str, context: &DecodeContext<'_>) -> TsUs {
    match millis.trim().parse::<i64>() {
        Ok(venue_ms) => clamp_exchange_ts(venue_ms, context.received_ts_us),
        Err(_) => context.received_ts_us,
    }
}

fn seconds_stamp(seconds: &str, context: &DecodeContext<'_>) -> TsUs {
    match seconds.trim().parse::<i64>() {
        Ok(venue_secs) => {
            clamp_exchange_ts(venue_secs.saturating_mul(1_000), context.received_ts_us)
        }
        Err(_) => context.received_ts_us,
    }
}
