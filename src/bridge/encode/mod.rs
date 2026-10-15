//! Turns the attached model into protocol lines. It remembers what the page already holds, so a tick
//! carries only the sections that changed and only the chart buckets the page has not seen.

mod channels;
mod chart;
mod ladder;
mod link;
mod monitor;

use crate::desktop::attachment::Screen;
use crate::desktop::chart_model::ChartBucket;
use crate::desktop::chart_view;
use crate::desktop::dom_view::{FeedStatus, MAX_ROWS_PER_SIDE};
use crate::desktop::link_model::LinkStatus;
use crate::desktop::model::UiModel;
use crate::desktop::monitor_view::Channel;
use crate::desktop::position_chart_model::PositionBucket;
use crate::ids::InstrumentId;

use super::command::PageFit;
use super::frame::{
    Account, ByeLine, Chart, FeatureLine, Gaps, Ladder, Latency, LifecycleLine, Line, LinkLine,
    Risk, Summary, TickLine, View,
};

pub use link::hello_line;

/// `axis_ticks` returns as many ticks as asked when a fine step fits, and a risk span is at least
/// 1e8 mantissas, so a count taken straight from stdin could build an enormous axis.
const MAX_AXIS_TICKS: usize = 100;

/// A page that has not measured its plots yet still gets a usable axis.
const DEFAULT_MID_TICKS: usize = 9;
const DEFAULT_RISK_TICKS: usize = 5;

pub struct TickInput<'a> {
    pub model: &'a UiModel,
    pub session: u64,
    /// What the link says about the feed, before the ladder checks how fresh the book is.
    pub feed: FeedStatus,
}

/// What the page already holds of a series of buckets: how many were ever pushed, and the newest as
/// last sent, which may still be growing.
struct Cursor<B> {
    appended: u64,
    newest: Option<B>,
}

/// The last version of each section the page was sent. Forgetting one makes the next tick resend it.
#[derive(Default)]
struct Sent {
    view: Option<View>,
    ladder: Option<Ladder>,
    summary: Option<Summary>,
    account: Option<Account>,
    features: Option<Vec<FeatureLine>>,
    latency: Option<Latency>,
    gaps: Option<Gaps>,
    chart: Option<Chart>,
    risk: Option<Risk>,
    channels: [Option<u64>; Channel::ALL.len()],
    mid: Option<Cursor<ChartBucket>>,
    fills: Option<u64>,
    positions: Option<Cursor<PositionBucket>>,
}

pub struct Encoder {
    generation: u64,
    seq: u64,
    fit_rows: usize,
    mid_ticks: usize,
    risk_ticks: usize,
    catalog_session: Option<u64>,
    last_selected: Option<InstrumentId>,
    last_link: Option<LinkLine>,
    last_lifecycle: Option<LifecycleLine>,
    sent: Sent,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    pub fn new() -> Self {
        Self {
            generation: 1,
            seq: 0,
            fit_rows: MAX_ROWS_PER_SIDE,
            mid_ticks: DEFAULT_MID_TICKS,
            risk_ticks: DEFAULT_RISK_TICKS,
            catalog_session: None,
            last_selected: None,
            last_link: None,
            last_lifecycle: None,
            sent: Sent::default(),
        }
    }

    /// The catalog line when it is due, then the tick line when any section changed. Empty
    /// otherwise.
    pub fn lines(&mut self, input: &TickInput<'_>) -> Vec<String> {
        let model = input.model;
        let Some(catalog) = model.catalog() else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if self.catalog_session != Some(input.session) {
            self.catalog_session = Some(input.session);
            lines.push(link::catalog_line(input.session, catalog));
        }
        if self.has_diverged(model) {
            self.sent = Sent::default();
            self.generation += 1;
        }
        self.last_selected = Some(model.selected());
        let mut tick = self.sections(input);
        if !tick.is_empty() {
            self.seq += 1;
            tick.seq = self.seq;
            lines.push(Line::Tick(&tick).to_json());
        }
        lines
    }

    /// What the page can show. Clamped: rows to what the ladder can hold, each tick count to the
    /// axis ceiling.
    pub fn fit(&mut self, fit: PageFit) {
        match fit {
            PageFit::DomRows(rows) => self.fit_rows = rows.min(MAX_ROWS_PER_SIDE),
            PageFit::AxisTicks { mid, risk } => {
                self.mid_ticks = mid.min(MAX_AXIS_TICKS);
                self.risk_ticks = risk.min(MAX_AXIS_TICKS);
            }
        }
    }

    /// Forget everything sent and bump `generation`, so the next lines tell the page everything.
    pub fn resync(&mut self) {
        self.sent = Sent::default();
        self.generation += 1;
        self.catalog_session = None;
        self.last_link = None;
        self.last_lifecycle = None;
    }

    pub(crate) fn link_line(&mut self, status: &LinkStatus) -> Option<String> {
        let line = link::link_dto(status);
        if self.last_link.as_ref() == Some(&line) {
            return None;
        }
        let json = Line::Link(&line).to_json();
        self.last_link = Some(line);
        Some(json)
    }

    pub(crate) fn lifecycle_line(&mut self, session: u64, screen: &Screen) -> Option<String> {
        let line = link::lifecycle_dto(session, screen);
        if self.last_lifecycle.as_ref() == Some(&line) {
            return None;
        }
        let json = Line::Lifecycle(&line).to_json();
        self.last_lifecycle = Some(line);
        Some(json)
    }

    pub(crate) fn bye_line(reason: &'static str) -> String {
        Line::Bye(&ByeLine { reason }).to_json()
    }

    /// True when the page's picture can no longer be patched: another instrument is selected, or a
    /// ring was rebuilt under a cursor, which leaves its counter below what was already sent.
    fn has_diverged(&self, model: &UiModel) -> bool {
        let selected = model.selected();
        let chart = model.chart();
        self.last_selected.is_some_and(|last| last != selected)
            || self
                .sent
                .mid
                .as_ref()
                .is_some_and(|cursor| chart.buckets_appended(selected) < cursor.appended)
            || self
                .sent
                .fills
                .is_some_and(|appended| chart.fills_appended(selected) < appended)
            || self.sent.positions.as_ref().is_some_and(|cursor| {
                model.positions().buckets_appended(selected) < cursor.appended
            })
    }

    fn sections(&mut self, input: &TickInput<'_>) -> TickLine {
        let model = input.model;
        let scales = model.instrument_scales();
        let domain = chart_view::domain(model.chart(), model.selected());
        TickLine {
            session: input.session,
            view: changed(&mut self.sent.view, ladder::view(model)),
            ladder: changed(
                &mut self.sent.ladder,
                ladder::ladder(model, input.feed, self.fit_rows),
            ),
            summary: changed(&mut self.sent.summary, ladder::summary(model, scales.tick)),
            account: changed(
                &mut self.sent.account,
                monitor::account_section(model, scales.tick),
            ),
            features: changed(&mut self.sent.features, monitor::features(model)),
            channels: self.channels(model),
            latency: changed(&mut self.sent.latency, monitor::latency(model)),
            chart: self.chart(model, domain),
            risk: self.risk(model, domain),
            gaps: changed(
                &mut self.sent.gaps,
                Gaps {
                    books: model.book_gaps(),
                    events: model.event_gaps(),
                },
            ),
            ..TickLine::default()
        }
    }
}

/// `next` when it differs from what was last sent, remembering it as sent.
fn changed<T: PartialEq + Clone>(last: &mut Option<T>, next: T) -> Option<T> {
    if last.as_ref() == Some(&next) {
        return None;
    }
    *last = Some(next.clone());
    Some(next)
}
