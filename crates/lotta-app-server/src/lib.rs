//! HTTP, WebSocket, and OpenAI-compatible transport adapters.
//!
//! This crate decodes requests, invokes application operations, and encodes responses. It must
//! not own business logic or process lifecycle.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
