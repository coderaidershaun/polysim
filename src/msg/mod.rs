//! The POD vocabulary that crosses a domain boundary, one file per boundary: `inbound` is
//! everything entering the hot thread, `exec` the order lifecycle out to a venue and back,
//! `persist` the records leaving for disk, `ui` the feed a workstation renders.

pub mod exec;
pub mod inbound;
pub mod persist;
pub mod ui;
