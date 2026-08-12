//! Account stream decoder. Unsolicited + account-wide. First: track symbol? Ours?

use crate::adapters::venue_clock::clamp_exchange_ts;
use crate::ids::{AssetId, TradeId, VenueOrderId};
use crate::msg::exec::{
    ACCOUNT_CHUNK_ASSETS, AccountChunk, AccountChunkKind, AssetBalance, ExecEvent, ExecKind,
    Liquidity, RejectClass,
};
use crate::registry::AssetDictionary;
use crate::time::TsUs;

use super::client_id::classify_client_order_id;
use super::wire::{ExecutionReport, StreamEnvelope, StreamPayload};
use super::{DecodeContext, FRAME, WireError, money_field, order_side, venue_status};
use crate::adapters::decode::{mantissa_field, price_field, qty_field};

/// Frame type. Nothing silent. Driver counts, operator sees.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Exec(ExecEvent),
    /// Absolute balances; commit on is_last_chunk.
    Account(Vec<AccountChunk>),
    /// Delta stream loses frame -> wrong forever. Snapshot required.
    BalanceChanged,
    Ignored(IgnoredReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IgnoredReason {
    /// Untracked symbol. Drop at edge (promise: configured only).
    UntrackedSymbol,
    UnhandledEvent,
}

/// Borrowed balance (share chunker).
/// The two payloads that reach here spell the same two numbers differently — the WS event uses `f`
/// and `l`, REST uses `free` and `locked` — so each view carries the names its own source used and a
/// decode failure can name a field the operator will find in the payload.
struct BalanceView<'a> {
    asset: &'a str,
    free: &'a str,
    locked: &'a str,
    free_field: &'static str,
    locked_field: &'static str,
}

/// # Errors: Json (bare), decimal/enum (fatal on overflow).
pub fn decode_stream_event(
    json: &str,
    context: &DecodeContext<'_>,
) -> Result<StreamEvent, WireError> {
    let envelope: StreamEnvelope = FRAME.decode(json)?;
    match envelope.event {
        StreamPayload::ExecutionReport(report) => decode_execution_report(&report, context),
        StreamPayload::AccountPosition(position) => {
            let balances: Vec<BalanceView<'_>> = position
                .balances
                .iter()
                .map(|balance| BalanceView {
                    asset: &balance.asset,
                    free: &balance.free,
                    locked: &balance.locked,
                    free_field: "f",
                    locked_field: "l",
                })
                .collect();
            account_chunks(
                &balances,
                AccountChunkKind::Update,
                position.last_update_ms.max(0) as u64,
                clamp_exchange_ts(position.event_ts_ms, context.received_ts_us),
                context,
            )
            .map(StreamEvent::Account)
        }
        StreamPayload::BalanceUpdate => Ok(StreamEvent::BalanceChanged),
        StreamPayload::Unhandled => Ok(StreamEvent::Ignored(IgnoredReason::UnhandledEvent)),
    }
}

/// Absolute snapshot. Chunks REPLACE table. # Errors: As decode_stream_event.
pub fn account_snapshot_chunks(
    balances: &[crate::adapters::binance::rest::Balance],
    venue_update_ts_ms: i64,
    context: &DecodeContext<'_>,
) -> Result<Vec<AccountChunk>, WireError> {
    let views: Vec<BalanceView<'_>> = balances
        .iter()
        .map(|balance| BalanceView {
            asset: &balance.asset,
            free: &balance.free,
            locked: &balance.locked,
            free_field: "free",
            locked_field: "locked",
        })
        .collect();
    account_chunks(
        &views,
        AccountChunkKind::Snapshot,
        venue_update_ts_ms.max(0) as u64,
        clamp_exchange_ts(venue_update_ts_ms, context.received_ts_us),
        context,
    )
}

fn decode_execution_report(
    report: &ExecutionReport,
    context: &DecodeContext<'_>,
) -> Result<StreamEvent, WireError> {
    let Some(instrument) = context.symbols.instrument(&report.symbol) else {
        return Ok(StreamEvent::Ignored(IgnoredReason::UntrackedSymbol));
    };

    let kind = execution_kind(&report.execution_type)?;
    let ownership = classify_client_order_id(subject_id(report, kind), context.identity);

    Ok(StreamEvent::Exec(ExecEvent {
        instrument,
        client_id: ownership.client_id,
        venue_order_id: (report.order_id >= 0).then_some(VenueOrderId(report.order_id)),
        trade_id: (report.trade_id >= 0).then_some(TradeId(report.trade_id)),
        kind,
        status: Some(venue_status("X", &report.order_status)?),
        reject: report_reject(kind, &report.reject_reason),
        provenance: ownership.provenance,
        side: order_side("S", &report.side)?,
        liquidity: matches!(kind, ExecKind::ReportTrade).then(|| match report.is_maker {
            true => Liquidity::Maker,
            false => Liquidity::Taker,
        }),
        price: price_field("p", &report.price)?,
        qty: qty_field("q", &report.qty)?,
        last_price: price_field("L", &report.last_price)?,
        last_qty: qty_field("l", &report.last_qty)?,
        cumulative_qty: qty_field("z", &report.cumulative_qty)?,
        cumulative_quote: money_field("Z", &report.cumulative_quote)?,
        commission: mantissa_field("n", &report.commission)?,
        commission_asset: commission_asset(report.commission_asset.as_deref(), context.assets),
        // Stream reports reason, not code (code from request). reject = meaning either way.
        reject_code: 0,
        // REPLACED (amend) has no count. See amend_budget_remaining().
        amends_remaining: ExecEvent::AMENDS_UNKNOWN,
        recon_seq: 0,
        exchange_ts_us: clamp_exchange_ts(report.transact_ts_ms, context.received_ts_us),
        request_sent_ts_us: None,
        received_ts_us: context.received_ts_us,
        queued_ts_us: context.received_ts_us,
    }))
}

/// Cancel/amend: c=REQUEST, C=order. Engine addresses order slot. c=fresh id (no slot).
fn subject_id(report: &ExecutionReport, kind: ExecKind) -> &str {
    let acts_on_another_order = matches!(kind, ExecKind::ReportCanceled | ExecKind::ReportAmended);
    match acts_on_another_order && !report.orig_client_order_id.is_empty() {
        true => &report.orig_client_order_id,
        false => &report.client_order_id,
    }
}

/// x = what happened. REPLACED = amend (order.amend.keepPriority).
fn execution_kind(execution_type: &str) -> Result<ExecKind, WireError> {
    Ok(match execution_type {
        "NEW" => ExecKind::ReportNew,
        "TRADE" => ExecKind::ReportTrade,
        "CANCELED" => ExecKind::ReportCanceled,
        "REPLACED" => ExecKind::ReportAmended,
        "REJECTED" => ExecKind::ReportRejected,
        // EXPIRED/TRADE_PREVENTION both kill order. EXPIRED_IN_MATCH says account crossed self.
        "EXPIRED" | "TRADE_PREVENTION" => ExecKind::ReportExpired,
        unknown => {
            return Err(WireError::UnknownEnum {
                field: "x",
                value: unknown.into(),
            });
        }
    })
}

/// Reject by reason (EXPIRED NOT rejection; order existed, venue took it).
fn report_reject(kind: ExecKind, reason: &str) -> Option<RejectClass> {
    if !matches!(kind, ExecKind::ReportRejected) {
        return None;
    }
    Some(match reason {
        // Post-only outcome. Book moved. Order never rests.
        "WOULD_MATCH_IMMEDIATELY" => RejectClass::Refused,
        // Out of money. Retry = pointless.
        "INSUFFICIENT_BALANCES" => RejectClass::Fatal,
        // Unknown = unhandled. Retry = duplicate flood.
        _ => RejectClass::Fatal,
    })
}

/// Fee charged in asset received. Unregistered -> UNKNOWN (not misattribute). Null = zero fee.
fn commission_asset(asset: Option<&str>, assets: &AssetDictionary) -> AssetId {
    asset.map_or(AssetId::UNKNOWN, |name| assets.id(name))
}

fn account_chunks(
    balances: &[BalanceView<'_>],
    kind: AccountChunkKind,
    venue_update_ts_ms: u64,
    exchange_ts_us: TsUs,
    context: &DecodeContext<'_>,
) -> Result<Vec<AccountChunk>, WireError> {
    let blank = AssetBalance {
        asset: AssetId::UNKNOWN,
        free: 0,
        locked: 0,
    };
    let mut chunk = AccountChunk {
        kind,
        balances: [blank; ACCOUNT_CHUNK_ASSETS],
        len: 0,
        is_last_chunk: false,
        venue_update_ts_ms,
        exchange_ts_us,
        received_ts_us: context.received_ts_us,
        queued_ts_us: context.received_ts_us,
    };
    let mut chunks = Vec::new();
    for group in balances.chunks(ACCOUNT_CHUNK_ASSETS) {
        chunk.balances = [blank; ACCOUNT_CHUNK_ASSETS];
        for (slot, balance) in chunk.balances.iter_mut().zip(group) {
            *slot = AssetBalance {
                // Unregistered cross (don't filter). Hot counts. Dropped = unexplained.
                asset: context.assets.id(balance.asset),
                free: mantissa_field(balance.free_field, balance.free)?,
                locked: mantissa_field(balance.locked_field, balance.locked)?,
            };
        }
        chunk.len = group.len() as u8;
        chunks.push(chunk);
    }
    // Empty account still commits. No snapshot = silent stale. Commit empty -> empties table.
    if chunks.is_empty() {
        chunks.push(chunk);
    }
    if let Some(last) = chunks.last_mut() {
        last.is_last_chunk = true;
    }
    Ok(chunks)
}
