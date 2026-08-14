//! Deterministic test clocks, IDs, in-memory ports, and fixtures.
//!
//! This crate supports tests across workspace boundaries. Production crates must not depend on
//! it outside test and development dependency scopes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
