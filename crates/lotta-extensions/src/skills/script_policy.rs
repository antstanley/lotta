//! Skill process execution through the existing permission, sandbox, and runtime ports.

use lotta_runtime::ports::{
    ProcessEvent, ProcessOutcome, ProcessRequest, SandboxPort, ToolDefinition, ValidatedToolInput,
};
use lotta_tools::{
    PermissionDecision, PermissionGate, PermissionInvocation, SandboxDecision, SandboxGate,
    SandboxInvocation,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Fixed script execution failure categories without input or adapter detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillScriptError {
    /// Permission policy denied execution.
    PermissionDenied,
    /// Permission policy requires caller-owned approval.
    ApprovalRequired,
    /// Workspace sandbox policy denied execution.
    SandboxDenied,
    /// A permission or sandbox gate could not inspect the invocation.
    PolicyInfrastructure,
    /// The runtime sandbox adapter failed or is unavailable.
    RuntimeAdapter,
}

/// Executes caller-built bounded requests only after the shared gates.
pub struct SkillScriptRunner<'a> {
    permission: &'a dyn PermissionGate,
    sandbox: &'a dyn SandboxGate,
    process: &'a dyn SandboxPort,
}

impl<'a> SkillScriptRunner<'a> {
    /// Creates a runner using the exact shared policy seams and sandbox process port.
    #[must_use]
    pub const fn new(
        permission: &'a dyn PermissionGate,
        sandbox: &'a dyn SandboxGate,
        process: &'a dyn SandboxPort,
    ) -> Self {
        Self {
            permission,
            sandbox,
            process,
        }
    }

    /// Runs permission, then sandbox, then the sandbox process adapter exactly once.
    ///
    /// # Errors
    /// Returns a fixed typed policy or runtime failure.
    ///
    /// # Cancellation
    /// Cancellation is delegated unchanged to [`SandboxPort`].
    pub async fn run(
        &self,
        definition: &ToolDefinition,
        input: &ValidatedToolInput,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> Result<ProcessOutcome, SkillScriptError> {
        let permission = PermissionInvocation::from_definition(definition, input);
        match self
            .permission
            .check(permission)
            .map_err(|_| SkillScriptError::PolicyInfrastructure)?
        {
            PermissionDecision::Deny => return Err(SkillScriptError::PermissionDenied),
            PermissionDecision::Ask => return Err(SkillScriptError::ApprovalRequired),
            PermissionDecision::Allow => {}
        }
        let sandbox = SandboxInvocation {
            internal_name: definition.internal_name.as_str(),
            input,
        };
        match self
            .sandbox
            .check(sandbox)
            .map_err(|_| SkillScriptError::PolicyInfrastructure)?
        {
            SandboxDecision::Deny => return Err(SkillScriptError::SandboxDenied),
            SandboxDecision::Allow => {}
        }
        self.process
            .execute(request, events, cancellation)
            .await
            .map_err(|_| SkillScriptError::RuntimeAdapter)
    }
}
