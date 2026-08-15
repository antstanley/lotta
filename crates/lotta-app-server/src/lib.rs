//! HTTP, WebSocket, and OpenAI-compatible transport adapters.
//!
//! This crate decodes requests, invokes application operations, and encodes responses. It must
//! not own business logic or process lifecycle.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Authentication policy and bearer verification.
pub mod auth;
/// Canonical transport bounds.
pub mod bounds;
/// Listener command-line and startup configuration.
pub mod config;
/// Stable app-server errors.
pub mod errors;
/// Compatibility alias for the Task 14 error module path.
pub use errors as error;
/// Bounded WebSocket framing.
pub mod framing;
/// Injected-clock heartbeat state.
pub mod heartbeat;
/// Bounded HTTP JSON extraction.
pub mod http_body;
/// Axum listener lifecycle.
pub mod listener;
