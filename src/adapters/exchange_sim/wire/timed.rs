//! Exchange timestamps applied to simulated payloads.

use super::super::core::orders::SimOrder;
use super::{SimFill, VENUE_MINTED_CANCEL_ID, VenueWire, templates};
use crate::msg::exec::VenueOrderStatus;
use crate::time::TsUs;

#[derive(Debug, Clone, Copy)]
pub struct TimedVenueWire<'a> {
    wire: &'a VenueWire,
    event_ts_us: TsUs,
}

impl VenueWire {
    pub fn at(&self, event_ts_us: TsUs) -> TimedVenueWire<'_> {
        TimedVenueWire {
            wire: self,
            event_ts_us,
        }
    }
}

impl TimedVenueWire<'_> {
    pub fn place_ack(&self, order: &SimOrder) -> String {
        self.wire.order_ack(
            templates::PLACE_ACK,
            order,
            VenueOrderStatus::New,
            self.event_ts_us,
            |_| {},
        )
    }

    pub fn cancel_ack(&self, order: &SimOrder) -> String {
        let subject = self.wire.client_order_id(order);
        self.wire.order_ack(
            templates::CANCEL_ACK,
            order,
            VenueOrderStatus::Canceled,
            self.event_ts_us,
            |result| {
                result["clientOrderId"] = VENUE_MINTED_CANCEL_ID.into();
                result["origClientOrderId"] = subject.into();
            },
        )
    }

    pub fn amend_ack(&self, order: &SimOrder) -> String {
        self.wire.amend_ack(order, self.event_ts_us)
    }

    pub fn order_status_as(&self, order: &SimOrder, status: VenueOrderStatus) -> String {
        self.wire.status_ack(order, status, self.event_ts_us)
    }

    pub fn open_orders(&self, resting: &[SimOrder]) -> String {
        self.wire.open_orders(resting, self.event_ts_us)
    }

    pub fn new_report(&self, order: &SimOrder) -> String {
        self.wire
            .report(templates::REPORT_NEW, order, self.event_ts_us, |_| {})
    }

    /// # Panics
    /// If the settlement's cumulative totals disagree with the order it names.
    pub fn trade_report(&self, order: &SimOrder, fill: SimFill<'_>) -> String {
        self.wire.trade_report(order, fill, self.event_ts_us)
    }

    pub fn cancel_report(&self, order: &SimOrder) -> String {
        let subject = self.wire.client_order_id(order);
        self.wire.report(
            templates::REPORT_CANCELED,
            order,
            self.event_ts_us,
            |event| {
                event["c"] = VENUE_MINTED_CANCEL_ID.into();
                event["C"] = subject.into();
            },
        )
    }

    pub fn rejected_report(&self, order: &SimOrder) -> String {
        self.wire
            .rejection_report(templates::REPORT_REJECTED_CROSS, order, self.event_ts_us)
    }
}

pub(super) fn stamp_stream_event(event: &mut serde_json::Value, event_ts_us: TsUs) {
    let event_ts_ms = event_ts_us.micros().div_euclid(1_000);
    event["E"] = event_ts_ms.into();
    event["T"] = event_ts_ms.into();
}

pub(super) fn stamp_response(document: &mut serde_json::Value, event_ts_us: TsUs) {
    let Some(result) = document.get_mut("result") else {
        return;
    };
    stamp_order(result, event_ts_us);
}

pub(super) fn stamp_order(order: &mut serde_json::Value, event_ts_us: TsUs) {
    let event_ts_ms = event_ts_us.micros().div_euclid(1_000);
    for field in ["transactTime", "updateTime"] {
        if order.get(field).is_some() {
            order[field] = event_ts_ms.into();
        }
    }
}
