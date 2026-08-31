//! OpenAI-compatible model projection over the canonical local agent repository.
//!
//! The catalog is loaded as one bounded complete visible set before the response
//! cap is applied. This keeps advertised names collision-free even when the
//! colliding agent sorts beyond the returned page.

/// Stable OpenAI error envelopes.
pub mod errors;
/// OpenAI model listing wire types and handler operation.
pub mod models;
/// Shared advertised-model resolver for OpenAI routes.
pub mod resolve;

#[cfg(test)]
#[path = "tests/auth_is_shared.rs"]
mod auth_is_shared;
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
