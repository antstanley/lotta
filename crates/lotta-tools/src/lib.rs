//! Tool registry, permission checks, and built-in executors.
//!
//! This crate implements tool ports and workspace-specific OS sandbox adapters without unsafe
//! exceptions. Seatbelt and Bubblewrap execution uses only safe Tokio process APIs.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod allowlist;
pub mod builtin;
pub mod clamp;
pub mod limits;
pub mod names;
pub mod permissions;
pub mod pipeline;
pub mod registry;
pub mod sandbox;
pub mod scrub;
pub mod toolset;

pub use allowlist::ToolAllowlist;
pub use names::{ToolNameRow, internal_name, model_name, rows};
pub use permissions::{
    AllowAllPermissions, PermissionDecision, PermissionGate, PermissionInvocation,
    PermissionPolicy, PolicyGate,
};
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
pub use sandbox::{
    AllowAllSandbox, OsSandbox, SandboxBackend, SandboxDecision, SandboxError, SandboxGate,
    SandboxInvocation, WorkspacePolicy, WorkspaceSandboxGate,
};
pub use toolset::{ParseToolsetIdError, ToolsetId, ToolsetPreference};
