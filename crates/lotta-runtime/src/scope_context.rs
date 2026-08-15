//! Explicit, task-local per-turn ambient context.

use crate::RuntimeError;
use lotta_domain::{BoundedJsonValue, NonEmptyString, PermissionMode, RuntimeScope};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Typed workspace confinement roots carried by a turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSandbox {
    root: PathBuf,
    isolation_root: PathBuf,
}

impl WorkspaceSandbox {
    /// Creates explicit workspace and isolation roots.
    #[must_use]
    pub fn new(root: PathBuf, isolation_root: PathBuf) -> Self {
        Self {
            root,
            isolation_root,
        }
    }

    /// Returns the workspace sandbox root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the isolation root.
    #[must_use]
    pub fn isolation_root(&self) -> &Path {
        &self.isolation_root
    }
}

/// Validated input used to construct a complete ambient snapshot.
pub struct ScopeContextInput {
    /// Stable connection identifier.
    pub connection_id: NonEmptyString,
    /// Stable device identifier.
    pub device_id: NonEmptyString,
    /// Exact runtime scope.
    pub runtime_scope: RuntimeScope,
    /// Resolved current working directory.
    pub cwd: PathBuf,
    /// Workspace confinement roots.
    pub workspace_sandbox: Option<WorkspaceSandbox>,
    /// Current permission policy.
    pub permission_mode: PermissionMode,
    /// Immutable prevalidated selected skills.
    pub selected_skills: Arc<[NonEmptyString]>,
    /// Bounded validated tool context.
    pub tool_context: BoundedJsonValue,
    /// Shared cooperative cancellation token.
    pub cancellation_token: CancellationToken,
    /// Nonzero lifecycle lease generation.
    pub lease_generation: u64,
}

/// Cloneable immutable snapshot of all authoritative per-turn ambient data.
#[derive(Clone)]
pub struct ScopeContextSnapshot {
    connection_id: NonEmptyString,
    device_id: NonEmptyString,
    runtime_scope: RuntimeScope,
    cwd: PathBuf,
    workspace_sandbox: Option<WorkspaceSandbox>,
    permission_mode: PermissionMode,
    selected_skills: Arc<[NonEmptyString]>,
    tool_context: BoundedJsonValue,
    cancellation_token: CancellationToken,
    lease_generation: u64,
}

impl ScopeContextSnapshot {
    /// Validates and creates a complete snapshot.
    ///
    /// # Errors
    /// Returns [`RuntimeError::InvalidData`] when the lease generation is zero.
    pub fn new(input: ScopeContextInput) -> Result<Self, RuntimeError> {
        if input.lease_generation == 0 {
            return Err(RuntimeError::InvalidData {
                context: "lease_generation".into(),
            });
        }
        Ok(Self {
            connection_id: input.connection_id,
            device_id: input.device_id,
            runtime_scope: input.runtime_scope,
            cwd: input.cwd,
            workspace_sandbox: input.workspace_sandbox,
            permission_mode: input.permission_mode,
            selected_skills: input.selected_skills,
            tool_context: input.tool_context,
            cancellation_token: input.cancellation_token,
            lease_generation: input.lease_generation,
        })
    }

    /// Returns the connection identifier.
    #[must_use]
    pub const fn connection_id(&self) -> &NonEmptyString {
        &self.connection_id
    }
    /// Returns the device identifier.
    #[must_use]
    pub const fn device_id(&self) -> &NonEmptyString {
        &self.device_id
    }
    /// Returns the exact runtime scope.
    #[must_use]
    pub const fn runtime_scope(&self) -> &RuntimeScope {
        &self.runtime_scope
    }
    /// Returns the resolved current working directory.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Returns workspace confinement data.
    #[must_use]
    pub const fn workspace_sandbox(&self) -> Option<&WorkspaceSandbox> {
        self.workspace_sandbox.as_ref()
    }
    /// Returns the permission policy.
    #[must_use]
    pub const fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }
    /// Returns immutable selected skills.
    #[must_use]
    pub fn selected_skills(&self) -> &[NonEmptyString] {
        &self.selected_skills
    }
    /// Returns bounded tool context.
    #[must_use]
    pub const fn tool_context(&self) -> &BoundedJsonValue {
        &self.tool_context
    }
    /// Returns the shared cancellation token.
    #[must_use]
    pub const fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }
    /// Returns the nonzero lifecycle lease generation.
    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.lease_generation
    }
}

impl fmt::Debug for ScopeContextSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopeContextSnapshot")
            .field("connection_id", &self.connection_id)
            .field("device_id", &self.device_id)
            .field("runtime_scope", &self.runtime_scope)
            .field(
                "workspace_sandbox_present",
                &self.workspace_sandbox.is_some(),
            )
            .field("permission_mode", &self.permission_mode)
            .field("selected_skill_count", &self.selected_skills.len())
            .field("tool_context_present", &true)
            .field("cancelled", &self.cancellation_token.is_cancelled())
            .field("lease_generation", &self.lease_generation)
            .finish_non_exhaustive()
    }
}

tokio::task_local! {
    static SCOPE_CONTEXT: ScopeContextSnapshot;
}

/// Runs an async operation under one explicit task-local snapshot.
pub async fn scope_operation<F, T>(snapshot: ScopeContextSnapshot, operation: F) -> T
where
    F: Future<Output = T>,
{
    SCOPE_CONTEXT.scope(snapshot, operation).await
}

/// Returns the current snapshot only while inside an explicitly installed scope.
#[must_use]
pub fn try_current() -> Option<ScopeContextSnapshot> {
    SCOPE_CONTEXT.try_with(Clone::clone).ok()
}

/// Spawns an owned task, explicitly installs and passes the supplied cloned snapshot.
pub fn spawn_scoped<F, Fut, T>(snapshot: ScopeContextSnapshot, operation: F) -> JoinHandle<T>
where
    F: FnOnce(ScopeContextSnapshot) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        let operation_snapshot = snapshot.clone();
        scope_operation(snapshot, operation(operation_snapshot)).await
    })
}
