//! Actor-originated events (no venue payload). Hot needs them or slots block.

use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use crate::msg::exec::{ExecEvent, ExecKind, Provenance, RejectClass};
use crate::time::TsUs;

// Timeout -> ambiguous; reconcile.
pub(crate) fn request_timed_out(
    instrument: InstrumentId,
    client_id: ClientOrderId,
    side: Side,
    at: TsUs,
) -> ExecEvent {
    ExecEvent {
        reject: Some(RejectClass::Ambiguous),
        side,
        ..blank(ExecKind::AckFailed, instrument, client_id, at)
    }
}

// Never sent; hot unwinds (not venue rejection).
pub(crate) fn place_not_sent(
    instrument: InstrumentId,
    client_id: ClientOrderId,
    side: Side,
    at: TsUs,
) -> ExecEvent {
    ExecEvent {
        side,
        ..blank(ExecKind::PlaceNotSent, instrument, client_id, at)
    }
}

// Never sent; the order is still resting unchanged, so hot returns the slot to it. Carries no side:
// the slot this folds onto already exists and keeps its own.
pub(crate) fn amend_not_sent(
    instrument: InstrumentId,
    client_id: ClientOrderId,
    at: TsUs,
) -> ExecEvent {
    blank(ExecKind::AmendNotSent, instrument, client_id, at)
}

// Resync marker; readiness arms on SnapshotEnd.
pub fn open_orders_snapshot_end(instrument: InstrumentId, at: TsUs) -> ExecEvent {
    blank(ExecKind::SnapshotEnd, instrument, ClientOrderId(0), at)
}

// Shared by live and simulated subscription paths.
pub(crate) fn stream_ready(at: TsUs) -> ExecEvent {
    blank(
        ExecKind::StreamReady,
        InstrumentId::NOT_APPLICABLE,
        ClientOrderId(0),
        at,
    )
}

pub(crate) fn stream_reset(at: TsUs) -> ExecEvent {
    blank(
        ExecKind::StreamReset,
        InstrumentId::NOT_APPLICABLE,
        ClientOrderId(0),
        at,
    )
}

// No venue payload; fold/branch on nothing. `kind` is a parameter rather than a default so a new
// constructor cannot inherit one silently — the natural default here would be AckFailed, the one
// kind that tells the hot path an order failed.
fn blank(
    kind: ExecKind,
    instrument: InstrumentId,
    client_id: ClientOrderId,
    at: TsUs,
) -> ExecEvent {
    ExecEvent {
        instrument,
        client_id,
        venue_order_id: None,
        trade_id: None,
        kind,
        status: None,
        reject: None,
        provenance: Provenance::Mine,
        side: Side::Buy,
        liquidity: None,
        price: Price(0),
        qty: Qty(0),
        last_price: Price(0),
        last_qty: Qty(0),
        cumulative_qty: Qty(0),
        cumulative_quote: 0,
        commission: 0,
        commission_asset: AssetId::UNKNOWN,
        reject_code: 0,
        amends_remaining: ExecEvent::AMENDS_UNKNOWN,
        recon_seq: 0,
        exchange_ts_us: at,
        request_sent_ts_us: None,
        received_ts_us: at,
        queued_ts_us: at,
    }
}
