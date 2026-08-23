//! Contract-neutral state machines shared by Aex Environment implementations.
//!
//! Brain owns public request and response types. This crate deliberately uses opaque string and
//! byte identities so an executor can enforce the invariants without forking Brain's schema.

#![forbid(unsafe_code)]

pub mod connector;
pub mod files;
pub mod materialization;
pub mod operation;
pub mod page;
pub mod resources;
