//! The center panel's fixed bands: the account, the feature list and the latency grid. Each is a
//! whole snapshot, since they are small and the page keeps nothing of them between ticks.

use serde::Serialize;

use crate::desktop::format;
use crate::desktop::model::UiModel;
use crate::desktop::monitor_view::{AssetRowView, SideCountView, account, feature_rows};
use crate::desktop::text::{
    BALANCE_DECIMALS, LATENCY_COLUMNS, LATENCY_ROWS, LATENCY_TITLE, VALUE_DECIMALS,
    account_captions, account_notes, asset_label, feature_hover, feature_name, latency_cells,
    reject_display, write_amount, write_feature_cell,
};
use crate::hot::exec::ExecHalt;
use crate::ids::Price;

use super::Styled;

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    notes: Vec<Styled>,
    captions: AccountCaptions,
    base: AssetLine,
    quote: AssetLine,
    sides: Sides,
    gate: Option<&'static str>,
    halt_reason: Option<&'static str>,
    reject: Styled,
}

#[derive(Serialize, Clone, PartialEq)]
struct AccountCaptions {
    free: String,
    locked: String,
    value: String,
}

#[derive(Serialize, Clone, PartialEq)]
struct AssetLine {
    label: String,
    free: String,
    locked: String,
    value: String,
}

#[derive(Serialize, Clone, PartialEq)]
struct Sides {
    bid: SideCounts,
    ask: SideCounts,
}

#[derive(Serialize, Clone, PartialEq)]
struct SideCounts {
    open: usize,
    inflight: usize,
    lost: usize,
    leaked: u64,
}

#[derive(Serialize, Clone, PartialEq)]
pub(super) struct FeatureLine {
    name: String,
    value: String,
    #[serde(rename = "changed")]
    has_changed: bool,
    #[serde(rename = "stale")]
    is_stale: bool,
    hover: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(super) struct Latency {
    header: &'static str,
    columns: Vec<&'static str>,
    rows: Vec<LatencyRow>,
}

#[derive(Serialize, Clone, PartialEq)]
struct LatencyRow {
    label: &'static str,
    cells: Vec<Styled>,
}

pub(super) fn account_section(model: &UiModel, tick: Price) -> Account {
    let view = account(model, model.selected(), tick);
    let [free, locked, value] = account_captions(&view);
    let (gate, halt_reason) = match view.halt {
        Some(ExecHalt::Armed) => (Some("armed"), None),
        Some(ExecHalt::Halted { reason, .. }) => (Some("halted"), Some(reason.label())),
        None => (None, None),
    };
    let (reject, reject_tone) = reject_display(view.last_reject.map(|reject| reject.origin));
    Account {
        notes: account_notes(&view)
            .into_iter()
            .map(|(text, tone)| Styled::new(text, tone))
            .collect(),
        captions: AccountCaptions {
            free,
            locked,
            value,
        },
        base: asset_line(view.base),
        quote: asset_line(view.quote),
        sides: Sides {
            bid: side_counts(view.bid),
            ask: side_counts(view.ask),
        },
        gate,
        halt_reason,
        reject: Styled::new(reject, reject_tone),
    }
}

fn asset_line(asset: AssetRowView<'_>) -> AssetLine {
    let amount = |mantissa, decimals| format::render(|out| write_amount(out, mantissa, decimals));
    AssetLine {
        label: asset_label(asset).to_owned(),
        free: amount(asset.balance.map(|balance| balance.free), BALANCE_DECIMALS),
        locked: amount(
            asset.balance.map(|balance| balance.locked),
            BALANCE_DECIMALS,
        ),
        value: amount(asset.value, VALUE_DECIMALS),
    }
}

fn side_counts(counts: SideCountView) -> SideCounts {
    SideCounts {
        open: counts.open,
        inflight: counts.in_flight,
        lost: counts.lost,
        leaked: counts.leaked,
    }
}

pub(super) fn features(model: &UiModel) -> Vec<FeatureLine> {
    let names: &[Box<str>] = model
        .catalog()
        .map_or(&[], |catalog| &catalog.feature_names);
    feature_rows(model, model.selected())
        .map(|row| FeatureLine {
            name: feature_name(&row, names).to_owned(),
            value: format::render(|out| write_feature_cell(out, &row)),
            has_changed: row.changed,
            is_stale: row.stale,
            hover: feature_hover(&row, names),
        })
        .collect()
}

pub(super) fn latency(model: &UiModel) -> Latency {
    let summary = model.latency();
    Latency {
        header: LATENCY_TITLE,
        columns: LATENCY_COLUMNS.to_vec(),
        rows: LATENCY_ROWS
            .iter()
            .map(|(label, flow)| LatencyRow {
                label,
                cells: latency_cells(*flow, summary.as_ref())
                    .into_iter()
                    .map(|(text, tone)| Styled::new(text, tone))
                    .collect(),
            })
            .collect(),
    }
}
