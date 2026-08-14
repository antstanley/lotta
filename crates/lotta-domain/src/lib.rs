//! Core IDs, entities, state machines, and errors.
//!
//! This crate owns pure domain concepts. It has no ports and must not depend on protocol,
//! runtime, adapters, or perform I/O.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod entities;
mod ids;
pub mod runtime;
mod scalars;
mod scope;

pub use entities::*;
pub use ids::{AgentId, ConversationId, IdError, IdKind, MessageId, ResponseId, RunId};
pub use runtime::*;
pub use scalars::{Clock, NonEmptyString, ScalarError, Timestamp};
pub use scope::RuntimeScope;
