//! Git-backed, agent-scoped memory editing tools.

mod definitions;
mod executor;
mod operations;
mod patch;
mod patch_parse;

use crate::registry::ToolRegistration;
use lotta_domain::AgentId;
use lotta_runtime::ports::MemFsPort;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Fixed memory bundle construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryBundleError;

/// Explicit identity used for memory commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryAuthor {
    name: String,
    email: String,
}

impl MemoryAuthor {
    /// Validates bounded, single-line, non-control author fields.
    ///
    /// # Errors
    /// Returns a fixed error for empty, overbound, control-bearing, or malformed email values.
    pub fn new(name: String, email: String) -> Result<Self, MemoryBundleError> {
        if !valid_identity(&name) || !valid_identity(&email) || !email.contains('@') {
            return Err(MemoryBundleError);
        }
        Ok(Self { name, email })
    }

    /// Returns the author name without exposing memory contents.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the author email.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }
}

/// Memory tools sharing one explicit agent repository capability.
pub struct MemoryToolBundle {
    root: PathBuf,
    registrations: Vec<ToolRegistration>,
}

pub(crate) struct MemoryState {
    root: PathBuf,
    agent: AgentId,
    port: Arc<dyn MemFsPort>,
    author: MemoryAuthor,
    mutations: Mutex<()>,
}

impl MemoryToolBundle {
    /// Constructs tools for exactly `memfs/<agent-id>/memory/`.
    ///
    /// # Errors
    /// Returns a fixed error unless the explicit root is canonical and matches the agent.
    pub fn new(
        root: &Path,
        agent: AgentId,
        port: Arc<dyn MemFsPort>,
        author: MemoryAuthor,
    ) -> Result<Self, MemoryBundleError> {
        validate_root(root, &agent)?;
        let state = Arc::new(MemoryState {
            root: root.to_owned(),
            agent,
            port,
            author,
            mutations: Mutex::new(()),
        });
        Ok(Self {
            root: root.to_owned(),
            registrations: definitions::registrations(&state)?,
        })
    }

    /// Returns the two canonical registrations.
    #[must_use]
    pub fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }

    /// Returns the exact canonical agent memory root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn validate_root(root: &Path, agent: &AgentId) -> Result<(), MemoryBundleError> {
    let metadata = std::fs::symlink_metadata(root).map_err(|_| MemoryBundleError)?;
    let suffix = PathBuf::from("memfs").join(agent.as_str()).join("memory");
    if !root.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(MemoryBundleError);
    }
    if root.canonicalize().map_err(|_| MemoryBundleError)? != root || !root.ends_with(suffix) {
        return Err(MemoryBundleError);
    }
    Ok(())
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod commits;
#[cfg(test)]
mod confinement;
#[cfg(test)]
mod is_sequential;
