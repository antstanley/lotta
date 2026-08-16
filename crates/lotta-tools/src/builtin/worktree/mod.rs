//! Managed linked-worktree ownership and provisioning tools.

mod definitions;
mod executor;
mod manager;
mod provisioning;

use crate::registry::ToolRegistration;
use lotta_runtime::ports::ChildProcessPort;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

/// Fixed worktree bundle construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorktreeBundleError;

/// Stable owner identity for one conversation and one process incarnation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorktreeOwner {
    /// Agent identifier.
    pub agent_id: String,
    /// Conversation identifier.
    pub conversation_id: String,
    /// Process identifier used for stale-owner detection.
    pub process_id: u32,
    /// Host identifier; remote hosts are never presumed stale.
    pub hostname: String,
    /// Unique process-start nonce, preventing PID reuse from matching.
    pub start_nonce: String,
}

impl WorktreeOwner {
    /// Validates bounded, single-line identity fields.
    ///
    /// # Errors
    ///
    /// Returns an error when an identity field is invalid or the process ID is zero.
    pub fn new(
        agent_id: String,
        conversation_id: String,
        process_id: u32,
        hostname: String,
        start_nonce: String,
    ) -> Result<Self, WorktreeBundleError> {
        if process_id == 0
            || !valid(&agent_id)
            || !valid(&conversation_id)
            || !valid(&hostname)
            || !valid(&start_nonce)
        {
            return Err(WorktreeBundleError);
        }
        Ok(Self {
            agent_id,
            conversation_id,
            process_id,
            hostname,
            start_nonce,
        })
    }
}

/// Trusted, conversation-local canonical working-directory state.
pub struct ConversationWorktreeContext {
    main_cwd: PathBuf,
    current_cwd: RwLock<PathBuf>,
}

impl ConversationWorktreeContext {
    /// Creates canonical typed context without consulting process-global cwd.
    ///
    /// # Errors
    ///
    /// Returns an error when either path is not a canonical, non-symlink directory.
    pub fn new(main_cwd: &Path, current_cwd: &Path) -> Result<Self, WorktreeBundleError> {
        let main_cwd = canonical_directory(main_cwd)?;
        let current_cwd = canonical_directory(current_cwd)?;
        Ok(Self {
            main_cwd,
            current_cwd: RwLock::new(current_cwd),
        })
    }

    /// Returns the canonical primary checkout cwd.
    pub fn main_cwd(&self) -> PathBuf {
        self.main_cwd.clone()
    }

    /// Returns the canonical current conversation cwd.
    ///
    /// # Errors
    ///
    /// Returns an error when the context lock is poisoned.
    pub fn current_cwd(&self) -> Result<PathBuf, WorktreeBundleError> {
        self.current_cwd
            .read()
            .map(|value| value.clone())
            .map_err(|_| WorktreeBundleError)
    }

    fn replace(&self, cwd: PathBuf) -> Result<PathBuf, ()> {
        let mut current = self.current_cwd.write().map_err(|_| ())?;
        Ok(std::mem::replace(&mut *current, cwd))
    }
}

/// Managed worktree tools sharing trusted identity and cwd context.
pub struct WorktreeToolBundle {
    registrations: Vec<ToolRegistration>,
    manager: Arc<WorktreeManager>,
}

/// Repository-scoped manager for linked worktrees and advisory ownership.
pub struct WorktreeManager {
    primary_root: PathBuf,
    managed_root: PathBuf,
    process: Arc<dyn ChildProcessPort>,
    liveness: Arc<dyn ProcessLiveness>,
    owner: WorktreeOwner,
    context: Arc<ConversationWorktreeContext>,
    owned: Mutex<BTreeSet<PathBuf>>,
}

/// Injectable process-liveness seam used for deterministic stale lock handling.
pub trait ProcessLiveness: Send + Sync {
    /// Returns whether this exact same-host process incarnation remains live.
    fn is_alive(&self, process_id: u32, start_nonce: &str) -> bool;
}

impl WorktreeToolBundle {
    /// Constructs exact `EnterWorktree` and `ExitWorktree` registrations.
    ///
    /// # Errors
    ///
    /// Returns an error when either canonical tool registration cannot be built.
    pub fn new(manager: Arc<WorktreeManager>) -> Result<Self, WorktreeBundleError> {
        Ok(Self {
            registrations: definitions::registrations(&manager)?,
            manager,
        })
    }

    /// Returns the exact two canonical registrations.
    #[must_use]
    pub fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }

    /// Returns the shared repository-scoped manager.
    #[must_use]
    pub fn manager(&self) -> &Arc<WorktreeManager> {
        &self.manager
    }
}

impl WorktreeManager {
    /// Validates explicit canonical roots, owner, and conversation context.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid roots, mismatched context, or managed-directory failure.
    pub fn new(
        primary_root: &Path,
        process: Arc<dyn ChildProcessPort>,
        liveness: Arc<dyn ProcessLiveness>,
        owner: WorktreeOwner,
        context: Arc<ConversationWorktreeContext>,
    ) -> Result<Self, WorktreeBundleError> {
        let primary_root = canonical_directory(primary_root)?;
        if context.main_cwd() != primary_root {
            return Err(WorktreeBundleError);
        }
        let managed_root = primary_root.join(".letta/worktrees");
        std::fs::create_dir_all(&managed_root).map_err(|_| WorktreeBundleError)?;
        if canonical_directory(&managed_root)? != managed_root {
            return Err(WorktreeBundleError);
        }
        Ok(Self {
            primary_root,
            managed_root,
            process,
            liveness,
            owner,
            context,
            owned: Mutex::new(BTreeSet::new()),
        })
    }

    /// Returns the canonical primary checkout.
    #[must_use]
    pub fn primary_root(&self) -> &Path {
        &self.primary_root
    }

    /// Returns the canonical managed worktree directory.
    #[must_use]
    pub fn managed_root(&self) -> &Path {
        &self.managed_root
    }

    /// Returns trusted conversation context.
    #[must_use]
    pub fn context(&self) -> &Arc<ConversationWorktreeContext> {
        &self.context
    }
}

impl Drop for WorktreeManager {
    fn drop(&mut self) {
        let paths = self
            .owned
            .lock()
            .map(|mut value| std::mem::take(&mut *value))
            .unwrap_or_default();
        for path in paths {
            let _ = self.release(&path);
        }
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, WorktreeBundleError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| WorktreeBundleError)?;
    if !path.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorktreeBundleError);
    }
    path.canonicalize().map_err(|_| WorktreeBundleError)
}

fn valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod execution_evidence;
#[cfg(test)]
mod ownership_lock;
#[cfg(test)]
mod provisioning_tests;
