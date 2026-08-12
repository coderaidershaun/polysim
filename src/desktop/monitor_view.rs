//! Pure projections: quote summary -> tick distances, feature freshness, channel histories newest-first.

use super::dom_view::{delta_from_mid, snapshot_mid, tick_index};
use super::exec_model::{BalanceCell, OrderStatus, RejectCell};
use super::model::UiModel;
use super::monitor_model::FeatureCell;
use crate::hot::exec::ExecHalt;
use crate::ids::{FIXED_SCALE, InstrumentId, Price, Qty, Side};
use crate::msg::persist::FeatureId;
use crate::time::{DurationUs, TsUs};

/// Feature row recently-changed highlight duration. Two spins = perceptible beat without flashing.
const CHANGED_WITHIN_SPINS: i64 = 2;

/// Stale threshold (spins). Recorder emits every spin; silence = lagging feed. Five tolerates jitter (event-time only).
const STALE_AFTER_SPINS: i64 = 5;

/// Three quote cells (independently absent). Mid in half-ticks; deltas = unsigned distances (Guéant: no minus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuoteSummary {
    pub bid_delta_half_ticks: Option<i64>,
    pub mid_half_ticks: Option<i64>,
    pub ask_delta_half_ticks: Option<i64>,
}

/// Feature row for live list. Value = None (never emitted) or Some (including legitimate 0.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeatureRowView {
    pub feature: FeatureId,
    pub value: Option<f64>,
    pub changed: bool,
    pub stale: bool,
}

/// Quote -> summary cells. Mid from book best bid/ask; deltas on-grid. Show when live + valid mid.
pub fn quote_summary(model: &UiModel, instrument: InstrumentId, tick: Price) -> QuoteSummary {
    let mid = model
        .book(instrument)
        .and_then(|book| snapshot_mid(book, tick));
    let live = model.is_quote_live(instrument);
    let quote = model.quote(instrument).map(|(quote, _)| quote);
    let delta = |leg: Option<(Price, Qty)>| -> Option<i64> {
        if !live {
            return None;
        }
        let mid = mid?;
        let (price, _) = leg?;
        Some(delta_from_mid(tick_index(price, tick)?, mid))
    };
    QuoteSummary {
        bid_delta_half_ticks: delta(
            quote.and_then(|quote| quote.bids.iter().flatten().next().copied()),
        ),
        mid_half_ticks: mid,
        ask_delta_half_ticks: delta(
            quote.and_then(|quote| quote.asks.iter().flatten().next().copied()),
        ),
    }
}

/// Instrument's feature rows in catalog order (dense, never re-sorted). Freshness vs newest feed time.
pub fn feature_rows(
    model: &UiModel,
    instrument: InstrumentId,
) -> impl Iterator<Item = FeatureRowView> + '_ {
    let monitor = model.monitor();
    let freshest = monitor.latest_feed_ts_us(instrument);
    let spin_interval = monitor.spin_interval();
    (0..monitor.feature_count()).map(move |index| {
        let feature = FeatureId(index as u16);
        feature_row(
            feature,
            monitor.feature(instrument, feature),
            freshest,
            spin_interval,
        )
    })
}

/// Asset row of account band. Label empty = catalog had no name (painter falls back to role).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetRowView<'a> {
    pub label: &'a str,
    pub role: AssetRole,
    /// None = never emitted (absent ≠ zero, relevant to trading decisions).
    pub balance: Option<BalanceCell>,
    /// Total holding (free + locked) in QUOTE-asset mantissas at [`FIXED_SCALE`]. None = no balance,
    /// or — base only — no mid to value it at.
    pub value: Option<i64>,
}

/// Asset row leg (both shown before balance arrives so band shape doesn't move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetRole {
    Base,
    Quote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SideCountView {
    pub open: usize,
    pub in_flight: usize,
    /// Orders whose venue truth was lost (vs in_flight).
    pub lost: usize,
    /// Orders model couldn't retain (band undercount marker). Non-zero = partial truth.
    pub leaked: u64,
}

/// Account band state (shape stable, fields independently absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountView<'a> {
    pub base: AssetRowView<'a>,
    pub quote: AssetRowView<'a>,
    pub bid: SideCountView,
    pub ask: SideCountView,
    pub last_reject: Option<RejectCell>,
    /// None until engine's first execution frame.
    pub halt: Option<ExecHalt>,
    /// Unknown asset count (collapsed id; show count).
    pub unknown_asset_balances: u64,
    /// Exposure frames a peer sent unusable. Its sibling on screen, so neither loss is silent.
    pub rejected_position_frames: u64,
}

/// Project account state (asset rows from catalog by id). The tick prices the base holding from the
/// same book mid the quote summary reads, so the band and the MID cell cannot disagree.
pub fn account(model: &UiModel, instrument: InstrumentId, tick: Price) -> AccountView<'_> {
    let row = model
        .catalog()
        .and_then(|catalog| catalog.instrument(instrument));
    let exec = model.exec();
    let mid = model
        .book(instrument)
        .and_then(|book| snapshot_mid(book, tick));
    let base = row.and_then(|row| exec.balance(row.base_asset));
    let quote = row.and_then(|row| exec.balance(row.quote_asset));
    AccountView {
        base: AssetRowView {
            label: row.map_or("", |row| row.base.as_ref()),
            role: AssetRole::Base,
            balance: base,
            value: holding(base)
                .zip(mid)
                .and_then(|(holding, mid)| value_at_mid(holding, mid, tick)),
        },
        quote: AssetRowView {
            label: row.map_or("", |row| row.quote.as_ref()),
            role: AssetRole::Quote,
            balance: quote,
            // The quote asset's worth in quote terms is its own total.
            value: holding(quote),
        },
        bid: side_counts(model, instrument, Side::Buy),
        ask: side_counts(model, instrument, Side::Sell),
        last_reject: exec.last_reject(),
        halt: exec.halt(),
        unknown_asset_balances: exec.unknown_asset_balances(),
        rejected_position_frames: model.positions().rejected_frames(),
    }
}

/// Free plus locked: a coin held against a resting order is still one you own.
fn holding(balance: Option<BalanceCell>) -> Option<i64> {
    let balance = balance?;
    i64::try_from(i128::from(balance.free) + i128::from(balance.locked)).ok()
}

/// Holding valued at the mid, in quote mantissas. The tick and the mid's half fold into ONE division
/// so the value never double-rounds, and a result past `i64` is absent rather than clamped — a
/// clamped money figure is a wrong money figure, and nothing on screen would say so.
fn value_at_mid(holding: i64, mid_half_ticks: i64, tick: Price) -> Option<i64> {
    if tick.0 <= 0 {
        return None;
    }
    let scaled = i128::from(holding)
        .checked_mul(i128::from(mid_half_ticks))?
        .checked_mul(i128::from(tick.0))?;
    i64::try_from(scaled / (2 * i128::from(FIXED_SCALE))).ok()
}

fn side_counts(model: &UiModel, instrument: InstrumentId, side: Side) -> SideCountView {
    let Some(orders) = model.exec().side(instrument, side) else {
        return SideCountView::default();
    };
    SideCountView {
        open: orders.count(OrderStatus::Confirmed),
        in_flight: orders.count(OrderStatus::InFlight),
        lost: orders.count(OrderStatus::Lost),
        leaked: orders.leaked(),
    }
}

/// Rows appended but unseen (appended - watermark, saturating). Basis for unseen badge + resume-follow.
pub fn unseen(appended_total: u64, seen_watermark: u64) -> u64 {
    appended_total.saturating_sub(seen_watermark)
}

fn feature_row(
    feature: FeatureId,
    cell: Option<FeatureCell>,
    freshest: Option<TsUs>,
    spin_interval: DurationUs,
) -> FeatureRowView {
    let Some(cell) = cell else {
        return FeatureRowView {
            feature,
            value: None,
            changed: false,
            stale: false,
        };
    };
    let (changed, stale) = match freshest {
        Some(freshest) => {
            let since_change_us = freshest.diff(cell.last_changed_ts).micros();
            let since_update_us = freshest.diff(cell.last_update_ts).micros();
            let changed = since_change_us <= spins(spin_interval, CHANGED_WITHIN_SPINS);
            let stale = since_update_us > spins(spin_interval, STALE_AFTER_SPINS);
            (changed, stale)
        }
        None => (false, false),
    };
    FeatureRowView {
        feature,
        value: Some(cell.value),
        changed,
        stale,
    }
}

fn spins(spin_interval: DurationUs, count: i64) -> i64 {
    spin_interval.micros().saturating_mul(count)
}
