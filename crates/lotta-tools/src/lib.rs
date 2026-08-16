//! Tool registry, permission checks, and built-in executors.
//!
//! This crate implements tool ports and must not depend on transport adapters or unrelated
//! concrete adapters. A future isolated OS-sandbox adapter may require an approved unsafe-code
//! exception with SAFETY proofs, invariant tests, and explicit review ownership; no exception is
//! enabled in this bootstrap.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod allowlist;
pub mod clamp;
pub mod limits;
pub mod names;
pub mod pipeline;
pub mod registry;
pub mod scrub;
pub mod toolset;

pub use allowlist::ToolAllowlist;
pub use names::{ToolNameRow, internal_name, model_name, rows};
pub use pipeline::{
    ExecutorError, ExtensionOwner, ExtensionOwnerKind, NoopHooks, OutcomeSink, OwnerFailure,
    OwnerId, PipelineError, PipelineHooks, PipelineRequest, PipelineStage, PostHookStatus,
    PreHookResult, PreflightEvent, RawToolExecutionRequest, RawToolOutcome, SecretDelivery,
    SecretDeliveryKind, SecretResolver, ToolExecutor, TraceEvent, TraceSink, execute,
};
pub use registry::{
    RegisteredTool, RegistryError, RegistrySnapshot, TOOLS_LOADED_MAX, ToolRegistration,
    ToolRegistry,
};
pub use toolset::{ParseToolsetIdError, ToolsetId, ToolsetPreference};
