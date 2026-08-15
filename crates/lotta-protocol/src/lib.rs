//! Public wire commands, messages, and compatibility projections.
//!
//! This crate owns transport-neutral protocol types. It must not perform I/O or depend on
//! runtime and adapter implementations.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod command;
mod decode;
mod message;

pub use command::{ALL_COMMAND_DISCRIMINANTS, WsProtocolCommand};
pub use decode::{
    DecodeEffects, DecodeOutcome, RecoverableLoopErrorNotice, decode_text, decode_value,
};
pub use message::{ALL_MESSAGE_DISCRIMINANTS, WsProtocolMessage};

/// Baseline application-server protocol version from pinned `src/types/app-server-info.ts`.
pub const APP_SERVER_PROTOCOL_VERSION: u32 = 1;
