//! Core IDs, entities, state machines, and errors.
//!
//! This crate owns pure domain concepts. It has no ports and must not depend on protocol,
//! runtime, adapters, or perform I/O.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Named domain resource bounds and their observable actions.
pub mod bounds;
pub mod entities;
mod errors;
mod ids;
pub mod runtime;
mod scalars;
mod scope;
mod secret;

pub use entities::*;
pub use errors::DomainError;
pub use ids::{AgentId, ConversationId, IdKind, MessageId, ResponseId, RunId};
pub use runtime::*;
pub use scalars::{Clock, NonEmptyString, Timestamp};
pub use scope::RuntimeScope;
pub use secret::Secret;
