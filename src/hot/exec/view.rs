//! What a strategy is shown of execution state: `Copy` projections and the three events it can be
//! called back on.
//!
//! Projections rather than borrows, so a strategy cannot hold a view of the order table across the
//! mutation its own callback causes. Everything derived is a METHOD rather than a field —
//! `remaining()` computed at the call site cannot go stale, whereas a `remaining` field written at
//! one moment and read at another can.

use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side, TradeId, VenueOrderId};
use crate::msg::exec::{Liquidity, RejectClass};
use crate::time::TsUs;

use super::level::QuoteLevel;
use super::order::OrderState;
use super::reconcile::RejectReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderView {
    pub client_id: ClientOrderId,
    pub venue_order_id: Option<VenueOrderId>,
    pub instrument: InstrumentId,
    pub side: Side,
    pub level: QuoteLevel,
    pub state: OrderState,
    pub price: Price,
    pub qty: Qty,
    pub filled: Qty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkingOrderView {
    pub client_id: ClientOrderId,
    pub instrument: InstrumentId,
    pub side: Side,
    pub level: Option<QuoteLevel>,
    pub state: OrderState,
    pub price: Price,
    pub qty: Qty,
    pub filled: Qty,
}

impl OrderView {
    #[inline]
    pub fn is_in_flight(self) -> bool {
        self.state.is_in_flight()
    }

    #[inline]
    pub fn is_working(self) -> bool {
        self.state.is_working()
    }

    #[inline]
    pub fn remaining(self) -> Qty {
        Qty((self.qty.0 - self.filled.0).max(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fill {
    pub instrument: InstrumentId,
    pub client_id: ClientOrderId,
    pub side: Side,
    pub level: QuoteLevel,
    pub price: Price,
    pub qty: Qty,
    pub notional: i64,
    pub commission: i64,
    pub commission_asset: AssetId,
    pub liquidity: Option<Liquidity>,
    pub trade_id: Option<TradeId>,
    pub state: OrderState,
    pub event_ts_us: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderUpdate {
    pub instrument: InstrumentId,
    pub client_id: ClientOrderId,
    pub side: Side,
    pub level: QuoteLevel,
    pub state: OrderState,
    pub price: Price,
    pub qty: Qty,
    pub filled: Qty,
    pub event_ts_us: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectOrigin {
    Local(RejectReason),
    Venue { class: RejectClass, code: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderReject {
    pub instrument: InstrumentId,
    pub client_id: Option<ClientOrderId>,
    pub side: Side,
    pub level: Option<QuoteLevel>,
    pub origin: RejectOrigin,
    pub event_ts_us: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecCallback {
    None,
    Fill(Fill),
    Update(OrderUpdate),
    Reject(OrderReject),
}
