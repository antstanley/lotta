//! Model-provider ports and provider adapters.
//!
//! This crate owns provider boundaries. It must not expose vendor errors or depend on transport
//! adapters and unrelated concrete adapters.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Stable model identity, settings, resolution, catalog, and update services.
pub mod model;

/// Local endpoint adapters and native model discovery.
pub mod local;

/// Native HTTP provider adapters.
pub mod native;
