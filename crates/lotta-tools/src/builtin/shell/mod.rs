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
pub const LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT: u64 = 180_000;
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
