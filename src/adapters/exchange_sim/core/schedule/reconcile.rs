//! Simulated reconciliation answers.

use crate::ids::ClientOrderId;
use crate::msg::exec::VenueOrderStatus;

use super::super::resting::{OrderPhase, OrderSnapshot};
use super::voice::refusal;
use super::{
    AnswerKind, AnswerSubject, DeliverySchedule, Landing, Rejection, VenueAnswer, VenueVoice,
};

impl DeliverySchedule {
    pub(super) fn status(&mut self, snapshot: OrderSnapshot, at: Landing) {
        self.push(
            None,
            AnswerKind::Observation,
            at,
            VenueVoice::Response(VenueAnswer::Status {
                order: snapshot.order,
                status: status_of(snapshot),
            }),
        );
    }

    pub(super) fn no_such_order(&mut self, client_id: ClientOrderId, at: Landing) {
        self.push(
            None,
            AnswerKind::Observation,
            at,
            refusal(client_id, Rejection::NoSuchOrder, AnswerSubject::Query),
        );
    }

    pub(super) fn snapshot(&mut self, rows: &[OrderSnapshot], at: Landing) {
        let listed = rows.iter().map(|row| row.order).collect();
        self.push(
            None,
            AnswerKind::Observation,
            at,
            VenueVoice::Response(VenueAnswer::OpenOrders(listed)),
        );
    }
}

fn status_of(snapshot: OrderSnapshot) -> VenueOrderStatus {
    match snapshot.phase {
        OrderPhase::Closed(reason) => reason.venue_status(),
        _ if snapshot.order.filled.0 > 0 => VenueOrderStatus::PartiallyFilled,
        _ => VenueOrderStatus::New,
    }
}
