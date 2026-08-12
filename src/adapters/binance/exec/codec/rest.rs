//! REST decoders. Decode HERE (not driver). Provenance test parity critical.

use crate::adapters::binance::rest::OrderRecord;
use crate::adapters::venue_clock::clamp_exchange_ts;
use crate::ids::{AssetId, Price, Qty, VenueOrderId};
use crate::msg::exec::{ExecEvent, ExecKind};
use crate::time::TsUs;

use super::client_id::classify_client_order_id;
use super::{DecodeContext, WireError, money_field, order_side, venue_status};
use crate::adapters::decode::{price_field, qty_field};

/// Order (kind caller's). Fold on status (resolves -2011).
/// # Errors: Decimal/enum fatal, untracked = Ok(None).
pub fn decode_order_record(
    record: &OrderRecord,
    kind: ExecKind,
    recon_seq: u64,
    context: &DecodeContext<'_>,
) -> Result<Option<ExecEvent>, WireError> {
    let Some(instrument) = context.symbols.instrument(&record.symbol) else {
        return Ok(None);
    };
    // Use orig (fresh id names no slot).
    let subject = record
        .orig_client_order_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&record.client_order_id);
    let ownership = classify_client_order_id(subject, context.identity);

    Ok(Some(ExecEvent {
        instrument,
        client_id: ownership.client_id,
        venue_order_id: (record.order_id >= 0).then_some(VenueOrderId(record.order_id)),
        trade_id: None,
        kind,
        status: Some(venue_status("status", &record.status)?),
        reject: None,
        provenance: ownership.provenance,
        side: order_side("side", &record.side)?,
        liquidity: None,
        price: price_field("price", &record.price)?,
        qty: qty_field("origQty", &record.orig_qty)?,
        last_price: Price(0),
        last_qty: Qty(0),
        cumulative_qty: qty_field("executedQty", &record.executed_qty)?,
        cumulative_quote: money_field("cummulativeQuoteQty", &record.cumulative_quote_qty)?,
        commission: 0,
        commission_asset: AssetId::UNKNOWN,
        reject_code: 0,
        amends_remaining: ExecEvent::AMENDS_UNKNOWN,
        recon_seq,
        exchange_ts_us: record_stamp(record, context),
        request_sent_ts_us: None,
        received_ts_us: context.received_ts_us,
        queued_ts_us: context.received_ts_us,
    }))
}

fn record_stamp(record: &OrderRecord, context: &DecodeContext<'_>) -> TsUs {
    match record
        .transact_time_ms
        .or(record.update_time_ms)
        .or(record.time_ms)
    {
        Some(venue_ms) => clamp_exchange_ts(venue_ms, context.received_ts_us),
        None => context.received_ts_us,
    }
}
