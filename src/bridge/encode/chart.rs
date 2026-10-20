//! The mid chart and the risk chart under it. Both are append-only for the page: a tick carries the
//! buckets and fills it has not seen, plus the newest bucket again while it is still growing, and
//! the whole window after a generation bump. That can be hundreds of buckets, so their wire keys are
//! one or two letters while the field names here stay whole.

use serde::Serialize;

use crate::desktop::chart_model::{ChartBucket, ChartFill, bucket_open_ts};
use crate::desktop::chart_view::{self, ChartBounds, ChartDomain};
use crate::desktop::format;
use crate::desktop::model::UiModel;
use crate::desktop::position_chart_model::PositionBucket;
use crate::desktop::position_chart_view::{self, QuoteBounds, RiskSeries};
use crate::desktop::text::{
    AxisLabels, NO_EVENT_TIME, axis_labels, mid_note, risk_note, sign_tone,
};
use crate::ids::{Price, Side};
use crate::time::DurationUs;

use super::{Cursor, Encoder, Styled, series_word};

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Chart {
    generation: u64,
    capacity: usize,
    first: Option<u64>,
    last: Option<u64>,
    low: Option<i64>,
    high: Option<i64>,
    axis: Vec<AxisTick>,
    buckets: Vec<ChartBucketLine>,
    fills: Vec<ChartFillLine>,
    note: Option<&'static str>,
    #[serde(rename = "fillsCaption")]
    has_fills_caption: bool,
}

#[derive(Serialize, Clone, PartialEq)]
struct AxisTick {
    #[serde(rename = "v")]
    value: i64,
    label: String,
}

#[derive(Serialize, Clone, PartialEq)]
struct ChartBucketLine {
    #[serde(rename = "i")]
    index: u64,
    #[serde(rename = "o")]
    open_half_ticks: i64,
    #[serde(rename = "h")]
    high_half_ticks: i64,
    #[serde(rename = "l")]
    low_half_ticks: i64,
    #[serde(rename = "c")]
    close_half_ticks: i64,
    #[serde(rename = "cl")]
    close_label: String,
    #[serde(rename = "t")]
    open_time: String,
    #[serde(rename = "gap")]
    has_gap_before: bool,
}

#[derive(Serialize, Clone, PartialEq)]
struct ChartFillLine {
    #[serde(rename = "i")]
    index: u64,
    #[serde(rename = "ht")]
    half_ticks: i64,
    side: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
pub(super) struct Risk {
    generation: u64,
    series: &'static str,
    latest: Option<Styled>,
    bounds: RiskBounds,
    axis: RiskAxis,
    buckets: Vec<RiskBucketLine>,
    note: Option<&'static str>,
}

#[derive(Serialize, Clone, PartialEq)]
struct RiskBounds {
    exposure: Option<Range>,
    pnl: Option<Range>,
}

#[derive(Serialize, Clone, PartialEq)]
struct Range {
    low: i64,
    high: i64,
}

#[derive(Serialize, Clone, PartialEq)]
struct RiskAxis {
    exposure: Vec<AxisTick>,
    pnl: Vec<AxisTick>,
}

#[derive(Serialize, Clone, PartialEq)]
struct RiskBucketLine {
    #[serde(rename = "i")]
    index: u64,
    #[serde(rename = "e")]
    exposure_quote: i64,
    #[serde(rename = "p")]
    pnl_quote: i64,
    #[serde(rename = "el")]
    exposure_label: String,
    #[serde(rename = "pl")]
    pnl_label: String,
}

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

impl Encoder {
    pub(super) fn chart(&mut self, model: &UiModel, domain: Option<ChartDomain>) -> Option<Chart> {
        let selected = model.selected();
        let chart = model.chart();
        let tick = model.instrument_scales().tick;
        let bounds = domain.and_then(|domain| chart_view::bounds(chart, selected, domain));
        let visible: Vec<ChartBucket> = domain.map_or_else(Vec::new, |domain| {
            chart_view::segment_points(chart, selected, domain)
                .map(|point| *point.bucket)
                .collect()
        });
        let buckets: Vec<ChartBucketLine> = cursor_delta(
            &mut self.sent.mid,
            chart.buckets_appended(selected),
            &visible,
            |bucket| bucket.index,
        )
        .iter()
        .map(|bucket| bucket_line(bucket, tick, chart.spin_interval()))
        .collect();
        let fills = self.fill_delta(model, domain);

        let has_visible_fill = domain.is_some_and(|domain| {
            chart_view::visible_fills(chart, selected, domain)
                .next()
                .is_some()
        });
        let note = mid_note(tick, domain, bounds);
        let head = Chart {
            generation: self.generation,
            capacity: chart.capacity(),
            first: domain.map(|domain| domain.first),
            last: domain.map(|domain| domain.last),
            low: bounds.map(|bounds| bounds.low),
            high: bounds.map(|bounds| bounds.high),
            axis: axis(bounds, self.mid_ticks, AxisLabels::VenueMid(tick)),
            buckets: Vec::new(),
            fills: Vec::new(),
            note,
            has_fills_caption: note.is_none() && has_visible_fill,
        };
        let is_unchanged =
            self.sent.chart.as_ref() == Some(&head) && buckets.is_empty() && fills.is_empty();
        if is_unchanged {
            return None;
        }
        self.sent.chart = Some(head.clone());
        Some(Chart {
            buckets,
            fills,
            ..head
        })
    }

    /// The visible fills the page does not hold, oldest first. Advances the fill cursor.
    fn fill_delta(&mut self, model: &UiModel, domain: Option<ChartDomain>) -> Vec<ChartFillLine> {
        let selected = model.selected();
        let chart = model.chart();
        let appended = chart.fills_appended(selected);
        let ring: Vec<ChartFill> = chart.fills(selected).copied().collect();
        let newer = self.sent.fills.map_or(ring.len(), |sent| {
            usize::try_from(appended.saturating_sub(sent)).map_or(ring.len(), |n| n.min(ring.len()))
        });
        let unsent = &ring[ring.len() - newer..];
        // A fill can arrive before the book that opens its bucket. It waits for that bucket, since
        // the cursor never comes back for a fill it has passed.
        let ready = unsent
            .iter()
            .position(|fill| domain.is_none_or(|domain| fill.index > domain.last))
            .unwrap_or(unsent.len());
        let waiting = (unsent.len() - ready) as u64;
        self.sent.fills = Some(appended.saturating_sub(waiting));
        unsent[..ready]
            .iter()
            .filter(|fill| domain.is_some_and(|domain| domain.contains(fill.index)))
            .map(|fill| ChartFillLine {
                index: fill.index,
                half_ticks: fill.half_ticks,
                side: side_word(fill.side),
            })
            .collect()
    }

    pub(super) fn risk(&mut self, model: &UiModel, domain: Option<ChartDomain>) -> Option<Risk> {
        let selected = model.selected();
        let positions = model.positions();
        let series = model.risk_series();
        let bounds_of = |series| {
            domain
                .and_then(|domain| position_chart_view::bounds(positions, selected, series, domain))
        };
        let exposure = bounds_of(RiskSeries::Exposure);
        let pnl = bounds_of(RiskSeries::Pnl);

        let visible: Vec<PositionBucket> = domain.map_or_else(Vec::new, |domain| {
            position_chart_view::visible_buckets(positions, selected, domain)
                .copied()
                .collect()
        });
        let buckets = cursor_delta(
            &mut self.sent.positions,
            positions.buckets_appended(selected),
            &visible,
            |bucket| bucket.index,
        );

        let head = Risk {
            generation: self.generation,
            series: series_word(series),
            latest: positions.latest(selected).map(|bucket| {
                let value = series.value(&bucket);
                Styled::new(readout(value), sign_tone(value))
            }),
            bounds: RiskBounds {
                exposure: exposure.map(range),
                pnl: pnl.map(range),
            },
            axis: RiskAxis {
                exposure: axis(
                    exposure.map(QuoteBounds::as_chart_bounds),
                    self.risk_ticks,
                    AxisLabels::QuoteAmount,
                ),
                pnl: axis(
                    pnl.map(QuoteBounds::as_chart_bounds),
                    self.risk_ticks,
                    AxisLabels::QuoteAmount,
                ),
            },
            buckets: Vec::new(),
            note: risk_note(domain, bounds_of(series)),
        };
        let buckets: Vec<RiskBucketLine> = buckets.iter().map(bucket_line_risk).collect();
        if self.sent.risk.as_ref() == Some(&head) && buckets.is_empty() {
            return None;
        }
        self.sent.risk = Some(head.clone());
        Some(Risk { buckets, ..head })
    }
}

/// Empty when there is no plot to label or the page asked for no ticks.
fn axis(bounds: Option<ChartBounds>, ticks: usize, labels: AxisLabels) -> Vec<AxisTick> {
    let Some(bounds) = bounds.filter(|_| ticks > 0) else {
        return Vec::new();
    };
    axis_labels(bounds, ticks, labels)
        .into_iter()
        .map(|(value, label)| AxisTick { value, label })
        .collect()
}

fn range(bounds: QuoteBounds) -> Range {
    let bounds = bounds.as_chart_bounds();
    Range {
        low: bounds.low,
        high: bounds.high,
    }
}

/// The visible buckets the page does not hold: all of them when it holds none, else the ones newer
/// than its newest and that one again if it has changed since. Advances the cursor.
fn cursor_delta<B: Copy + PartialEq>(
    cursor: &mut Option<Cursor<B>>,
    appended: u64,
    visible: &[B],
    index: impl Fn(&B) -> u64,
) -> Vec<B> {
    let held = cursor.as_ref().and_then(|cursor| cursor.newest);
    let delta = match held {
        None => visible.to_vec(),
        Some(held) => visible
            .iter()
            .filter(|bucket| {
                index(bucket) > index(&held) || (index(bucket) == index(&held) && **bucket != held)
            })
            .copied()
            .collect(),
    };
    *cursor = Some(Cursor {
        appended,
        newest: visible.last().copied(),
    });
    delta
}

fn bucket_line(bucket: &ChartBucket, tick: Price, spin: DurationUs) -> ChartBucketLine {
    let close = format::render(|out| format::write_venue_mid(out, bucket.close_half_ticks, tick));
    let time = format::render(|out| match bucket_open_ts(bucket.index, spin) {
        Some(at) => format::write_time_of_day(out, at),
        None => out.push_str(NO_EVENT_TIME),
    });
    ChartBucketLine {
        index: bucket.index,
        open_half_ticks: bucket.open_half_ticks,
        high_half_ticks: bucket.high_half_ticks,
        low_half_ticks: bucket.low_half_ticks,
        close_half_ticks: bucket.close_half_ticks,
        close_label: close,
        open_time: time,
        has_gap_before: bucket.has_gap_before,
    }
}

fn bucket_line_risk(bucket: &PositionBucket) -> RiskBucketLine {
    RiskBucketLine {
        index: bucket.index,
        exposure_quote: bucket.exposure_quote,
        pnl_quote: bucket.pnl_quote,
        exposure_label: readout(bucket.exposure_quote),
        pnl_label: readout(bucket.pnl_quote),
    }
}

fn readout(quote: i64) -> String {
    format::render(|out| format::write_quote_amount(out, quote, RiskSeries::READOUT_DECIMALS))
}
