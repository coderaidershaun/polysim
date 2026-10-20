//! The right panel and the quote band: the view the operator chose, the DOM ladder and the summary.
//! Built the way the window builds them, so both read the same book the same way.

use serde::Serialize;

use crate::desktop::chart_view::ChartMode;
use crate::desktop::dom_view::{
    DomGrouping, DomRow, DomStatus, DomUnit, DomView, DomViewInput, FeedStatus, QuotePlacement,
    StrategyCell, build_dom_view,
};
use crate::desktop::format;
use crate::desktop::model::{InstrumentScales, UiModel};
use crate::desktop::monitor_view::quote_summary;
use crate::desktop::text::{
    Lane, Ring, Tone, cell_tone, delta_hover, grouping_label, mid_tone, offscreen_of, order_tone,
    ring, row_price, status_pill, summary_caption, write_delta,
};
use crate::ids::{Price, Side};

use super::{Styled, series_word};

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct View {
    instrument: u16,
    chart_mode: &'static str,
    dom_unit: &'static str,
    dom_grouping: GroupingView,
    dom_levels: usize,
    risk_series: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(tag = "unit", rename_all = "lowercase")]
enum GroupingView {
    Bps {
        numerator: i64,
        denominator: i64,
        label: &'static str,
    },
    Ticks {
        #[serde(rename = "perBucket")]
        per_bucket: i64,
        label: &'static str,
    },
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Ladder {
    status: &'static str,
    pill: Option<Styled>,
    #[serde(rename = "muted")]
    is_muted: bool,
    mid: Styled,
    mid_half_ticks: Option<i64>,
    ticks_per_bucket: i64,
    rows: usize,
    asks: Vec<LadderRow>,
    bids: Vec<LadderRow>,
    chevrons: Chevrons,
}

#[derive(Serialize, Clone, PartialEq)]
struct LadderRow {
    tick: i64,
    price: String,
    public: Option<String>,
    order: Option<OrderCell>,
    want: Option<String>,
    ring: Option<&'static str>,
}

#[derive(Serialize, Clone, PartialEq)]
struct OrderCell {
    qty: String,
    word: &'static str,
    tone: &'static str,
}

#[derive(Serialize, Clone, PartialEq, Default)]
struct Chevrons {
    up: Option<Chevron>,
    down: Option<Chevron>,
}

#[derive(Serialize, Clone, PartialEq)]
struct Chevron {
    text: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(super) struct Summary {
    captions: SummaryCaptions,
    bid: Styled,
    mid: Styled,
    ask: Styled,
    hover: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
struct SummaryCaptions {
    bid: &'static str,
    mid: &'static str,
    ask: &'static str,
}

fn mode_word(mode: ChartMode) -> &'static str {
    match mode {
        ChartMode::Line => "line",
        ChartMode::Candles => "candles",
    }
}

fn unit_word(unit: DomUnit) -> &'static str {
    match unit {
        DomUnit::Ticks => "ticks",
        DomUnit::Bps => "bps",
    }
}

fn status_word(status: DomStatus) -> &'static str {
    match status {
        DomStatus::Live => "live",
        DomStatus::Stale => "stale",
        DomStatus::AwaitingBook => "awaiting",
        DomStatus::Disconnected => "disconnected",
    }
}

fn ring_word(ring: Ring) -> &'static str {
    match ring {
        Ring::Confirmed => "confirmed",
        Ring::InFlight => "inflight",
        Ring::Lost => "lost",
        Ring::Desired => "desired",
    }
}

pub(super) fn view(model: &UiModel) -> View {
    let grouping = model.dom_grouping();
    let label = grouping_label(grouping);
    View {
        instrument: model.selected().0,
        chart_mode: mode_word(model.chart_mode()),
        dom_unit: unit_word(model.dom_unit()),
        dom_grouping: match grouping {
            DomGrouping::Bps {
                numerator,
                denominator,
            } => GroupingView::Bps {
                numerator,
                denominator,
                label,
            },
            DomGrouping::Ticks { per_bucket } => GroupingView::Ticks { per_bucket, label },
        },
        dom_levels: model.dom_levels(),
        risk_series: series_word(model.risk_series()),
    }
}

pub(super) fn ladder(model: &UiModel, feed: FeedStatus, fit_rows: usize) -> Ladder {
    let scales = model.instrument_scales();
    let source = model.dom_source(feed);
    let view = build_dom_view(DomViewInput {
        snapshot: model.book(model.selected()),
        overlay: source.overlay,
        tick: scales.tick,
        grouping: model.dom_grouping(),
        rows_per_side: model.dom_levels().min(fit_rows),
        feed: source.feed,
    });
    let mut scratch = String::new();
    format::write_opt_venue_mid(&mut scratch, view.mid_half_ticks, scales.tick);
    Ladder {
        status: status_word(view.status),
        pill: status_pill(view.status, source.stale_age)
            .map(|(text, tone)| Styled::new(text, tone)),
        is_muted: view.status != DomStatus::Live,
        mid: Styled::new(scratch, mid_tone(view.mid_half_ticks)),
        mid_half_ticks: view.mid_half_ticks,
        ticks_per_bucket: view.ticks_per_bucket,
        rows: view.ask_rows().len(),
        asks: view
            .ask_rows()
            .iter()
            .map(|row| ladder_row(row, &view, scales))
            .collect(),
        bids: view
            .bid_rows()
            .iter()
            .map(|row| ladder_row(row, &view, scales))
            .collect(),
        chevrons: chevrons(&view),
    }
}

fn ladder_row(row: &DomRow, view: &DomView, scales: InstrumentScales) -> LadderRow {
    let qty = |qty| {
        format::render(|out| format::write_qty(out, qty, scales.qty_scale, scales.qty_decimals))
    };
    let decimals = format::price_decimals(scales.tick);
    let price = format::render(|out| row_price(out, scales.tick, decimals, row.tick_index));
    LadderRow {
        tick: row.tick_index,
        price,
        public: row.public_qty.map(qty),
        order: match row.strategy_cell() {
            Some(StrategyCell::Order { qty: held, status }) => Some(OrderCell {
                qty: qty(held),
                word: status.word(),
                tone: order_tone(status).as_str(),
            }),
            Some(StrategyCell::Desired { .. }) | None => None,
        },
        want: row.strategy_qty.map(qty),
        ring: ring(row, view.status).map(ring_word),
    }
}

/// The ask side first and the bid side after, as the window paints them, so where both pick the
/// same edge the bid wins.
fn chevrons(view: &DomView) -> Chevrons {
    let ask = offscreen_of(view.ask_order_placement, view.ask_placement);
    let bid = offscreen_of(view.bid_order_placement, view.bid_placement);
    let mut chevrons = Chevrons::default();
    for placement in [ask, bid] {
        match placement {
            QuotePlacement::OffScreenAbove { delta_half_ticks } => {
                chevrons.up = Some(chevron(delta_half_ticks));
            }
            QuotePlacement::OffScreenBelow { delta_half_ticks } => {
                chevrons.down = Some(chevron(delta_half_ticks));
            }
            QuotePlacement::Visible | QuotePlacement::None => {}
        }
    }
    chevrons
}

fn chevron(delta_half_ticks: i64) -> Chevron {
    let text = format::render(|out| format::write_mid(out, delta_half_ticks));
    Chevron { text }
}

pub(super) fn summary(model: &UiModel, tick: Price) -> Summary {
    let unit = model.dom_unit();
    let summary = quote_summary(model, model.selected(), tick);
    let delta = |value, live| {
        let mut text = String::new();
        let wrote = write_delta(&mut text, value, summary.mid_half_ticks, unit);
        Styled::new(text, cell_tone(wrote, live))
    };
    let mut mid = String::new();
    let wrote = format::write_opt_venue_mid(&mut mid, summary.mid_half_ticks, tick);
    Summary {
        captions: SummaryCaptions {
            bid: summary_caption(Lane::Delta(Side::Buy), unit),
            mid: summary_caption(Lane::Mid, unit),
            ask: summary_caption(Lane::Delta(Side::Sell), unit),
        },
        bid: delta(summary.bid_delta_half_ticks, Tone::Bid),
        mid: Styled::new(mid, cell_tone(wrote, Tone::Text)),
        ask: delta(summary.ask_delta_half_ticks, Tone::Ask),
        hover: delta_hover(unit),
    }
}
