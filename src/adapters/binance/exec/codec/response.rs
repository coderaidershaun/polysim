//! Response decoder + error classification. Error response = {code, msg} only.
//! Context from request (which is why each fn takes it).
//! Key: -2011 cancel = Ambiguous (FILLED collision). Unknown code = Fatal (no retry).

use crate::adapters::exec::ExecRequest;
use crate::adapters::venue_clock::clamp_exchange_ts;
use crate::ids::{ClientOrderId, InstrumentId, Price, Qty, Side, VenueOrderId};
use crate::msg::exec::{ExecEvent, ExecKind};
use crate::time::TsUs;

use super::client_id::classify_client_order_id;
use super::reject::{RejectSubject, amend_budget_remaining, classify_error};
use super::wire::{AmendResult, OrderResponse, ResponseEnvelope};
use super::{DecodeContext, FRAME, WireError, money_field, order_side, venue_status};
use crate::adapters::decode::{price_field, qty_field};

pub struct ResponseContext<'a> {
    pub decode: DecodeContext<'a>,
    pub request: ExecRequest,
    pub recon_seq: u64,
}

/// Every normalised event carried by one response.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedResponse {
    pub events: Vec<ExecEvent>,
}

/// # Errors: EmptyResponse, decimal/enum, MantissaOverflow (fatal).
pub fn decode_response(
    json: &str,
    context: &ResponseContext<'_>,
) -> Result<DecodedResponse, WireError> {
    let envelope: ResponseEnvelope = FRAME.decode(json)?;
    decode_single(envelope, context).map(|events| DecodedResponse { events })
}

fn decode_single(
    envelope: ResponseEnvelope,
    context: &ResponseContext<'_>,
) -> Result<Vec<ExecEvent>, WireError> {
    if let Some(error) = envelope.error {
        return Ok(failure_events(error.code, &error.msg, context));
    }
    let Some(result) = envelope.result else {
        return Err(WireError::EmptyResponse);
    };
    match context.request {
        ExecRequest::Place { .. } => {
            let order: OrderResponse = FRAME.decode_value(result)?;
            Ok(vec![order_event(&order, ExecKind::AckPlaced, context)?])
        }
        ExecRequest::Cancel { .. } => {
            let order: OrderResponse = FRAME.decode_value(result)?;
            Ok(vec![order_event(&order, ExecKind::AckCanceled, context)?])
        }
        ExecRequest::AmendQty { .. } => {
            let amended: AmendResult = FRAME.decode_value(result)?;
            Ok(vec![order_event(
                &amended.amended_order,
                ExecKind::AckAmended,
                context,
            )?])
        }
        ExecRequest::OrderStatus { .. } => {
            let order: OrderResponse = FRAME.decode_value(result)?;
            Ok(vec![order_event(&order, ExecKind::SnapshotOrder, context)?])
        }
        ExecRequest::OpenOrders { instrument } => decode_open_orders(result, instrument, context),
        ExecRequest::SubscribeUserStream => Ok(vec![lifecycle_event(
            ExecKind::StreamReady,
            InstrumentId(0),
            context,
        )]),
    }
}

fn decode_open_orders(
    result: serde_json::Value,
    instrument: InstrumentId,
    context: &ResponseContext<'_>,
) -> Result<Vec<ExecEvent>, WireError> {
    let orders: Vec<OrderResponse> = FRAME.decode_value(result)?;
    let mut events = Vec::with_capacity(orders.len() + 1);
    for order in &orders {
        // The venue answers per symbol, but an untracked one would have no instrument to name and
        // `ExecEvent::instrument` promises a configured one.
        if context.decode.symbols.instrument(&order.symbol).is_none() {
            continue;
        }
        events.push(order_event(order, ExecKind::SnapshotOrder, context)?);
    }
    events.push(lifecycle_event(ExecKind::SnapshotEnd, instrument, context));
    Ok(events)
}

fn failure_events(code: i32, message: &str, context: &ResponseContext<'_>) -> Vec<ExecEvent> {
    let subject_kind = match context.request {
        ExecRequest::Place { .. } => RejectSubject::Placement,
        ExecRequest::Cancel { .. } => RejectSubject::Cancellation,
        ExecRequest::AmendQty { .. } => RejectSubject::Amendment,
        ExecRequest::OrderStatus { .. } => RejectSubject::StatusQuery,
        ExecRequest::OpenOrders { .. } | ExecRequest::SubscribeUserStream => return Vec::new(),
    };
    let Some(client_id) = request_client_id(context.request) else {
        return Vec::new();
    };
    vec![ExecEvent {
        reject: Some(classify_error(code, message, subject_kind)),
        reject_code: code,
        amends_remaining: amend_budget_remaining(code, message),
        ..failed_event(client_id, context)
    }]
}

fn order_event(
    order: &OrderResponse,
    kind: ExecKind,
    context: &ResponseContext<'_>,
) -> Result<ExecEvent, WireError> {
    // Amend/cancel: use orig (fresh id names no slot).
    let subject = order
        .orig_client_order_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(&order.client_order_id);
    let ownership = classify_client_order_id(subject, context.decode.identity);
    let instrument = context
        .decode
        .symbols
        .instrument(&order.symbol)
        .unwrap_or_else(|| request_instrument(context.request));

    let executed = qty_field("executedQty", &order.executed_qty)?;
    Ok(ExecEvent {
        instrument,
        client_id: ownership.client_id,
        venue_order_id: (order.order_id >= 0).then_some(VenueOrderId(order.order_id)),
        trade_id: None,
        kind,
        status: Some(venue_status("status", &order.status)?),
        reject: None,
        provenance: ownership.provenance,
        side: order_side("side", &order.side)?,
        liquidity: None,
        price: price_field("price", &order.price)?,
        qty: qty_field("origQty", &order.qty)?,
        last_price: Price(0),
        last_qty: Qty(0),
        cumulative_qty: executed,
        cumulative_quote: money_field("cummulativeQuoteQty", &order.cumulative_quote)?,
        commission: 0,
        commission_asset: crate::ids::AssetId::UNKNOWN,
        reject_code: 0,
        amends_remaining: ExecEvent::AMENDS_UNKNOWN,
        recon_seq: context.recon_seq,
        exchange_ts_us: order_stamp(order, context),
        request_sent_ts_us: None,
        received_ts_us: context.decode.received_ts_us,
        queued_ts_us: context.decode.received_ts_us,
    })
}

fn failed_event(client_id: ClientOrderId, context: &ResponseContext<'_>) -> ExecEvent {
    ExecEvent {
        client_id,
        ..lifecycle_event(
            ExecKind::AckFailed,
            request_instrument(context.request),
            context,
        )
    }
}

fn lifecycle_event(
    kind: ExecKind,
    instrument: InstrumentId,
    context: &ResponseContext<'_>,
) -> ExecEvent {
    ExecEvent {
        instrument,
        client_id: ClientOrderId(0),
        venue_order_id: None,
        trade_id: None,
        kind,
        status: None,
        reject: None,
        provenance: crate::msg::exec::Provenance::Mine,
        side: request_side(context.request).unwrap_or(Side::Buy),
        liquidity: None,
        price: Price(0),
        qty: Qty(0),
        last_price: Price(0),
        last_qty: Qty(0),
        cumulative_qty: Qty(0),
        cumulative_quote: 0,
        commission: 0,
        commission_asset: crate::ids::AssetId::UNKNOWN,
        reject_code: 0,
        amends_remaining: ExecEvent::AMENDS_UNKNOWN,
        recon_seq: context.recon_seq,
        exchange_ts_us: context.decode.received_ts_us,
        request_sent_ts_us: None,
        received_ts_us: context.decode.received_ts_us,
        queued_ts_us: context.decode.received_ts_us,
    }
}

fn order_stamp(order: &OrderResponse, context: &ResponseContext<'_>) -> TsUs {
    match order.transact_ts_ms.or(order.update_ts_ms) {
        Some(venue_ms) => clamp_exchange_ts(venue_ms, context.decode.received_ts_us),
        None => context.decode.received_ts_us,
    }
}

fn request_instrument(request: ExecRequest) -> InstrumentId {
    match request {
        ExecRequest::Place { instrument, .. }
        | ExecRequest::Cancel { instrument, .. }
        | ExecRequest::AmendQty { instrument, .. }
        | ExecRequest::OrderStatus { instrument, .. }
        | ExecRequest::OpenOrders { instrument } => instrument,
        ExecRequest::SubscribeUserStream => InstrumentId(0),
    }
}

fn request_side(request: ExecRequest) -> Option<Side> {
    match request {
        ExecRequest::Place { side, .. } => Some(side),
        _ => None,
    }
}

fn request_client_id(request: ExecRequest) -> Option<ClientOrderId> {
    match request {
        ExecRequest::Place { client_id, .. }
        | ExecRequest::Cancel { client_id, .. }
        | ExecRequest::AmendQty { client_id, .. }
        | ExecRequest::OrderStatus { client_id, .. } => Some(client_id),
        ExecRequest::OpenOrders { .. } | ExecRequest::SubscribeUserStream => None,
    }
}
