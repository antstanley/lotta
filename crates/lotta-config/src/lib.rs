//! Validated configuration and secret-reference types.
//!
//! This crate owns configuration boundaries. It must not resolve secrets into logs or depend on
//! transport and runtime implementations.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
