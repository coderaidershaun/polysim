//! Monitor driver: Feed pushes frames through pipeline.

use polysim::desktop::model::UiModel;
use polysim::desktop::monitor::MonitorUiState;
use polysim::desktop::monitor_model::SystemNote;
use polysim::hot::exec::{CloseReason, OrderState, RejectOrigin};
use polysim::ids::{AssetId, ClientOrderId};
use polysim::ids::{FIXED_SCALE, InstrumentId, Price, Qty, Side};
use polysim::msg::exec::Liquidity;
use polysim::msg::persist::FeatureId;
use polysim::msg::ui::{DomQuote, UiBookState, UiEvent};
use polysim::time::DurationUs;
use polysim::time::TsUs;

use crate::fixture::{BASE_TS, book, levels, px, qm};

pub const SPIN: DurationUs = DurationUs::from_micros(100_000);
pub const QUOTE_QTY_MILLI: i64 = 9_000;

pub struct MonitorScene {
    pub name: &'static str,
    pub model: UiModel,
    pub feature_names: Vec<Box<str>>,
    pub instrument_names: Vec<Box<str>>,
    pub instrument: InstrumentId,
    pub tick: Price,
    pub qty_scale: i64,
    pub qty_decimals: usize,
    pub state: MonitorUiState,
}

/// Commission at 1e-8: realistic, not round.
const FILL_COMMISSION: i64 = 944_000;

/// Drives real UiModel, faithful ordered stream.
pub struct Feed {
    model: UiModel,
    seq: u64,
    book_seq: u64,
}

impl Feed {
    pub fn new(instruments: usize, features: usize) -> Self {
        Self {
            model: UiModel::with_monitor_capacity(instruments, features, SPIN),
            seq: 0,
            book_seq: 0,
        }
    }

    pub fn lifecycle(&mut self, note: SystemNote) {
        self.model.note_lifecycle(note);
    }

    pub fn book(&mut self, state: UiBookState, ts: i64, bids: &[(i64, i64)], asks: &[(i64, i64)]) {
        self.book_seq += 1;
        self.model.apply_book(book(
            self.book_seq,
            BASE_TS + ts,
            state,
            &levels(bids),
            &levels(asks),
        ));
    }

    pub fn quote(&mut self, ts: i64, bid: Option<i64>, ask: Option<i64>) {
        let quote = DomQuote::top(
            bid.map(|tick| (px(tick), qm(QUOTE_QTY_MILLI))),
            ask.map(|tick| (px(tick), qm(QUOTE_QTY_MILLI))),
        );
        self.apply(UiEvent::Quote {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            quote,
        });
    }

    pub fn trade(&mut self, ts: i64, aggressor: Side, tick: i64, milli: i64) {
        self.apply(UiEvent::Trade {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            aggressor,
            price: px(tick),
            qty: qm(milli),
        });
    }

    pub fn feature(&mut self, feature: u16, ts: i64, value: f64) {
        self.apply(UiEvent::Feature {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            feature: FeatureId(feature),
            value,
        });
    }

    pub fn order(&mut self, ts: i64, id: u64, side: Side, tick: i64, state: OrderState) {
        let filled = if matches!(state, OrderState::Closed(CloseReason::Filled)) {
            qm(QUOTE_QTY_MILLI)
        } else {
            Qty(0)
        };
        self.apply(UiEvent::OrderUpdate {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            client_id: ClientOrderId(id),
            side,
            state,
            price: px(tick),
            qty: qm(QUOTE_QTY_MILLI),
            filled,
            quote_level: None,
        });
    }

    pub fn refusal(&mut self, ts: i64, side: Side, origin: RejectOrigin) {
        self.apply(UiEvent::Reject {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            side,
            origin,
        });
    }

    pub fn fill(&mut self, ts: i64, side: Side, tick: i64, milli: i64) {
        self.apply(UiEvent::Fill {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
            side,
            price: px(tick),
            qty: qm(milli),
            commission: FILL_COMMISSION,
            commission_asset: AssetId(1),
            liquidity: Some(Liquidity::Maker),
            quote_level: None,
        });
    }

    pub fn rotation(&mut self, ts: i64) {
        self.apply(UiEvent::Rotation {
            instrument: InstrumentId(0),
            seq: 0,
            event_ts_us: ts_at(ts),
        });
    }

    pub fn skip(&mut self, count: u64) {
        self.seq += count;
    }

    pub fn system_appended(&self) -> u64 {
        self.model.monitor().system_appended()
    }

    fn apply(&mut self, mut event: UiEvent) {
        event.set_seq(self.seq);
        self.seq += 1;
        self.model.apply_event(event);
    }

    pub fn finish(
        self,
        name: &'static str,
        features: &[&str],
        state: MonitorUiState,
    ) -> MonitorScene {
        MonitorScene {
            name,
            model: self.model,
            feature_names: features
                .iter()
                .map(|name| Box::<str>::from(*name))
                .collect(),
            instrument_names: vec![Box::<str>::from("BTCUSDT")],
            instrument: InstrumentId(0),
            tick: TICK,
            qty_scale: FIXED_SCALE,
            qty_decimals: 3,
            state,
        }
    }
}

const TICK: Price = Price(FIXED_SCALE);

fn ts_at(relative: i64) -> TsUs {
    TsUs::from_micros(BASE_TS + relative)
}
