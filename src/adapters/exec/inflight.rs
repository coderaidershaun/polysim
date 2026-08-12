//! Outstanding requests; past deadline -> reconcile, never re-send (order may exist, answer lost, re-send = double).
//! Venue-neutral: every edge that pipelines requests needs the same table.

use crate::time::{DurationUs, TsUs};
use crate::warn;

use super::{ExecRequest, RequestId};

const CROWDED: usize = 32;

#[derive(Debug, Clone, Copy)]
pub(crate) struct InFlightRequest {
    pub request_id: RequestId,
    pub request: ExecRequest, // A bare venue error (binance {code,msg}, polymarket {error}) names no order -> needs context.
    pub sent_at: TsUs,
    pub recon_seq: u64, // Hot seq -> re-attach at send.
}

pub(crate) struct InFlightTable {
    entries: Vec<InFlightRequest>,
    timeout: DurationUs,
    timed_out: u64,
}

impl InFlightTable {
    pub(crate) fn new(timeout: DurationUs) -> Self {
        Self {
            entries: Vec::with_capacity(CROWDED),
            timeout,
            timed_out: 0,
        }
    }

    pub(crate) fn record(
        &mut self,
        request_id: RequestId,
        request: ExecRequest,
        sent_at: TsUs,
        recon_seq: u64,
    ) {
        self.entries.push(InFlightRequest {
            request_id,
            request,
            sent_at,
            recon_seq,
        });
        // On the crossing only: the table stays over the mark for as long as the crowd lasts, and
        // a warning per request is a flood at exactly the moment one is least wanted.
        if self.entries.len() == CROWDED + 1 {
            self.report_crowding();
        }
    }

    pub(crate) fn take(&mut self, request_id: RequestId) -> Option<InFlightRequest> {
        let position = self
            .entries
            .iter()
            .position(|entry| entry.request_id == request_id)?;
        Some(self.entries.swap_remove(position))
    }

    pub(crate) fn take_expired(&mut self, now: TsUs) -> Vec<InFlightRequest> {
        let timeout = self.timeout;
        let mut expired = Vec::new();
        self.entries.retain(|entry| {
            let is_live = now.diff(entry.sent_at) < timeout;
            if !is_live {
                expired.push(*entry);
            }
            is_live
        });
        self.timed_out += expired.len() as u64;
        expired
    }

    /// Dead socket -> answers gone; treat as timed out.
    pub(crate) fn take_all(&mut self) -> Vec<InFlightRequest> {
        std::mem::take(&mut self.entries)
    }

    pub(crate) fn timed_out(&self) -> u64 {
        self.timed_out
    }

    #[cold]
    fn report_crowding(&self) {
        warn!(
            "an execution edge has {} requests outstanding — above {CROWDED}, which the send policy should make unreachable",
            self.entries.len()
        );
    }
}
