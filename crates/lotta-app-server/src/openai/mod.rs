//! OpenAI-compatible model projection over the canonical local agent repository.
//!
//! The catalog is loaded as one bounded complete visible set before the response
//! cap is applied. This keeps advertised names collision-free even when the
//! colliding agent sorts beyond the returned page.

/// OpenAI Chat Completions route and runtime adapter.
pub mod chat;
/// Bounded header-keyed conversation cache.
pub mod chat_keys;
/// Unsigned bounded stored-Response cursors.
pub mod cursor;
/// Stable OpenAI error envelopes.
pub mod errors;
/// Bounded in-flight and settled idempotency outcomes.
pub mod idempotency;
/// OpenAI model listing wire types and handler operation.
pub mod models;
/// Shared advertised-model resolver for OpenAI routes.
pub mod resolve;
/// OpenAI Responses route and runtime projection.
pub mod responses;

#[cfg(test)]
#[path = "tests/auth_is_shared.rs"]
mod auth_is_shared;
#[cfg(test)]
#[path = "tests/chat_transport.rs"]
mod chat_transport;
#[cfg(test)]
#[path = "tests/listing_cap.rs"]
mod listing_cap;
#[cfg(test)]
#[path = "tests/model_resolution.rs"]
mod model_resolution;
#[cfg(test)]
#[path = "tests/route_registration.rs"]
mod route_registration;
#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;
