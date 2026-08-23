//! Shared Environment vocabulary owned by no state machine: the canonical identifier grammar, secret
//! newtypes, connector classes, guest environment policy, and cross-boundary object bounds.
//!
//! Every other Environments crate depends on this leaf; it depends only on Brain's protocol constants.

#![forbid(unsafe_code)]

pub mod connector;
pub mod guest_env;
pub mod identity;
pub mod secret;

/// One bound for every boundary that moves whole objects: the live-file export path, the guest
/// install routes, and the trusted adapter's staging transfers.
pub const MAX_OBJECT_BYTES: u64 = 512 * 1024 * 1024;
