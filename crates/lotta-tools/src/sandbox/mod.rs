//! Workspace-specific policy, OS sandbox adapters, and bounded process execution.

mod bwrap;
#[path = "workspace.rs"]
mod policy;
#[path = "process.rs"]
mod runner;
mod seatbelt;
mod unsupported;

pub use bwrap::bubblewrap_arguments;
pub use policy::{
    AllowAllSandbox, SandboxDecision, SandboxError, SandboxGate, SandboxInvocation,
    WorkspacePolicy, WorkspaceSandboxGate,
};
pub use runner::{OsSandbox, SandboxBackend};
pub use seatbelt::{SEATBELT_PROFILE, SEATBELT_PROGRAM, seatbelt_arguments};
pub use unsupported::unsupported_workspace_sandbox;

#[cfg(test)]
include!("tests.rs");
