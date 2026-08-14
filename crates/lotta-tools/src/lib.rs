//! Tool registry, permission checks, and built-in executors.
//!
//! This crate implements tool ports and must not depend on transport adapters or unrelated
//! concrete adapters. A future isolated OS-sandbox adapter may require an approved unsafe-code
//! exception with SAFETY proofs, invariant tests, and explicit review ownership; no exception is
//! enabled in this bootstrap.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
