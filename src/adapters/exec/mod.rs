//! Venue-neutral execution machinery: the core/driver split every exec adapter shares — phase
//! machine, order mirror, effects, exit lifecycle, resync policy. Proven generic by the simulator,
//! which drives it with no venue socket behind it.

mod capabilities;
mod core;
mod effect;
mod event;
mod exit;
mod identity;
mod inflight;
mod lifecycle;
mod mirror;
mod reject;
mod resync;

pub(crate) use capabilities::VenueCapabilities;
pub use core::{ExecCore, ObserveOrderError, Phase, REQUEST_TIMEOUT};
pub(crate) use effect::Outgoing;
pub use effect::{
    ExecEffect, ExecRequest, PlaceNotSentReason, RequestId, SkipReason, TimeoutFallout,
};
pub use event::open_orders_snapshot_end;
pub(crate) use event::{
    amend_not_sent, place_not_sent, request_timed_out, stream_ready, stream_reset,
};
pub(crate) use exit::{EdgeHandle, ExecStop, ExitPlan, SessionOutcome};
pub use identity::{EngineIdentity, LeaseNamespace, OrderOwnership, TeTag};
pub(crate) use inflight::{InFlightRequest, InFlightTable};
pub use lifecycle::LifecycleFold;
pub(crate) use lifecycle::mirrored_order;
pub use mirror::MirroredOrder;
pub use reject::{RejectVerdict, VenueAvailability};
pub use resync::{MAX_RESYNC_ATTEMPTS, ResyncPass, ResyncStep};
