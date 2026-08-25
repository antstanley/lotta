//! Bounded shell and process tools sharing one explicit sandbox capability.

mod definitions;
mod executor;
mod manager;
#[cfg(test)]
mod output_bounds;
#[cfg(test)]
mod two_stage_kill;

use crate::registry::ToolRegistration;
use lotta_domain::RuntimeScope;
use std::{path::Path, sync::Arc};

pub use manager::{CHILD_PROCESS_OUTPUT_BYTES_MAX, SHELL_CHILD_KILL_GRACE_MS, ShellSandbox};

/// Default local shell-tool deadline in milliseconds.
pub use lotta_runtime::bounds::LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT;
/// Largest pinned shell-family deadline in milliseconds.
pub const LOCAL_TOOL_EXECUTION_TIMEOUT_MS_MAX: u64 = 3_600_000;

/// Fixed shell bundle construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellBundleError;

/// Seven exact shell registrations sharing one bounded process manager and sandbox port.
pub struct ShellToolBundle {
    registrations: Vec<ToolRegistration>,
    manager: Arc<manager::ProcessManager>,
}

impl ShellToolBundle {
    /// Constructs shell registrations for one explicit runtime scope and canonical workspace.
    ///
    /// # Errors
    /// Rejects non-canonical workspace roots and malformed pinned definition assets.
    pub fn new(
        workspace_root: &Path,
        scope: RuntimeScope,
        sandbox: Arc<dyn ShellSandbox>,
    ) -> Result<Self, ShellBundleError> {
        validate_root(workspace_root)?;
        let manager = Arc::new(manager::ProcessManager::new(
            workspace_root.to_owned(),
            scope,
            sandbox,
        ));
        let registrations = definitions::registrations(&manager)?;
        Ok(Self {
            registrations,
            manager,
        })
    }

    /// Returns a cancellation owner for this bundle's exact process manager.
    ///
    /// # Errors
    /// Returns a fixed bundle error if the shared owner registry is unavailable.
    pub fn cancellation_owner(
        &self,
        scope: RuntimeScope,
        lease_generation: u64,
    ) -> Result<ShellCancellationOwner, ShellBundleError> {
        let owner = manager::TurnOwner {
            scope,
            lease_generation,
        };
        self.manager
            .set_launch_owner(owner.clone())
            .map_err(|_| ShellBundleError)?;
        Ok(ShellCancellationOwner {
            manager: Arc::clone(&self.manager),
            owner,
        })
    }

    /// Returns whether the shared manager retains any operation records.
    #[must_use]
    pub fn has_operations(&self) -> bool {
        self.manager.has_operations()
    }

    /// Summaries of every background session this bundle's manager tracks.
    ///
    /// Backs the device-status `background_processes` section, so snapshots
    /// reflect actual sessions instead of a hardcoded empty list.
    #[must_use]
    pub fn background_snapshot(&self) -> Vec<manager::ShellSessionSummary> {
        self.manager.background_snapshot()
    }

    /// Returns a stable identity for the exact shared process manager.
    #[must_use]
    pub fn manager_id(&self) -> usize {
        Arc::as_ptr(&self.manager) as usize
    }

    /// Cancels and joins every owned background shell session.
    ///
    /// # Errors
    /// Returns a fixed bundle error when a task cannot be joined.
    pub async fn shutdown(&self) -> Result<(), ShellBundleError> {
        self.manager.shutdown().await.map_err(|_| ShellBundleError)
    }

    /// Returns the exact production registrations.
    #[must_use]
    pub fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }
}

/// Runtime cancellation adapter over the existing Task38 process manager.
#[derive(Clone)]
pub struct ShellCancellationOwner {
    manager: Arc<manager::ProcessManager>,
    owner: manager::TurnOwner,
}

impl ShellCancellationOwner {
    /// Returns a stable identity for the exact shared process manager.
    #[must_use]
    pub fn manager_id(&self) -> usize {
        Arc::as_ptr(&self.manager) as usize
    }
}

impl lotta_runtime::turn::TurnChildOwner for ShellCancellationOwner {
    fn has_operations(&self) -> bool {
        self.manager.has_owned_operations(&self.owner)
    }

    fn terminate_and_reap(
        &self,
        kill_grace: std::time::Duration,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            if kill_grace != std::time::Duration::from_millis(SHELL_CHILD_KILL_GRACE_MS) {
                return Err(lotta_runtime::RuntimeError::InvalidData {
                    context: "shell child kill grace".into(),
                });
            }
            self.manager.shutdown_owner(&self.owner).await.map_err(|_| {
                lotta_runtime::RuntimeError::AdapterFailure {
                    code: "shell_child_cleanup",
                    context: "turn-scoped shell process manager".into(),
                }
            })
        })
    }
}

fn validate_root(path: &Path) -> Result<(), ShellBundleError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ShellBundleError)?;
    if !path.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ShellBundleError);
    }
    if path.canonicalize().map_err(|_| ShellBundleError)? != path {
        return Err(ShellBundleError);
    }
    Ok(())
}

#[cfg(test)]
mod execution;
#[cfg(test)]
mod names_match_baseline;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod timeout;
