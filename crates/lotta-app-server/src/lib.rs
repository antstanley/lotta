//! HTTP, WebSocket, and OpenAI-compatible transport adapters.
//!
//! This crate decodes requests, invokes application operations, and encodes responses. It must
//! not own business logic or process lifecycle.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Authentication policy and bearer verification.
pub mod auth;
/// Listener command-line and startup configuration.
pub mod config;
/// Stable app-server errors.
pub mod error;
/// Axum listener lifecycle.
pub mod listener;
