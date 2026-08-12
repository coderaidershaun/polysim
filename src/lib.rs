//! Market-data + quant engine: one lib (substance), binaries/examples (composition only).
//!
//! Groups below read in dependency order. Vocabulary is the POD types that cross a domain
//! boundary — nothing else crosses one.
//!
//! Submodules that are implementation partitions stay private behind a `pub use` facade; submodules
//! that are concepts in their own right stay `pub mod` and callers name the file.
//!
//! Every ring producer the hot thread writes to is a `*Sink` in [`sink`] — one type per
//! full-ring policy, and the module's files are the taxonomy.

#![forbid(unsafe_code)]

// declaration forms (macros only, no runtime surface)
mod labelled_enum;

// vocabulary
pub mod ids;
pub mod msg;
pub mod time;

// ring producers
pub mod sink;

// hot path
pub mod hot;

// async edge
pub mod adapters;
pub mod exposure;
pub mod link;
pub mod log;
pub mod persist;

// assembly
pub mod config;
pub mod registry;
pub mod runtime;
pub mod secrets;
pub mod shutdown;

/// Feature-gated: headless engine never builds wgpu/winit. What `pub` means here is stated inside.
#[cfg(feature = "ui")]
pub mod desktop;
