use crate::permissions::matcher::{canonical_tool_name, canonicalize_invocation_path, path_within};
use lotta_runtime::{WorkspaceSandbox, ports::ValidatedToolInput};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Fixed sandbox gate failure that retains no input or path detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SandboxError;

/// Exhaustive sandbox gate result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxDecision {
    /// Continue execution.
    Allow,
    /// Produce a durable sandbox-denied tool outcome.
    Deny,
}

/// Borrowed resolved invocation checked at the sandbox pipeline stage.
#[derive(Clone, Copy)]
pub struct SandboxInvocation<'a> {
    /// Stable internal tool name.
    pub internal_name: &'a str,
    /// Validated tool input.
    pub input: &'a ValidatedToolInput,
}

/// Synchronous effect-confinement gate.
pub trait SandboxGate: Send + Sync {
    /// Checks one invocation.
    ///
    /// # Errors
    /// Returns a fixed error when confinement infrastructure cannot inspect valid input.
    fn check(&self, invocation: SandboxInvocation<'_>) -> Result<SandboxDecision, SandboxError>;
}

/// Explicit permissive gate for callers without an attached workspace.
pub struct AllowAllSandbox;
impl SandboxGate for AllowAllSandbox {
    fn check(&self, _: SandboxInvocation<'_>) -> Result<SandboxDecision, SandboxError> {
        Ok(SandboxDecision::Allow)
    }
}

/// Canonical validated workspace and isolation roots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePolicy {
    root: PathBuf,
    isolation_root: PathBuf,
}

impl WorkspacePolicy {
    /// Validates and retains authoritative runtime workspace roots.
    ///
    /// # Errors
    /// Returns a fixed sandbox error unless both supplied roots are real, non-symlink directories
    /// and the workspace is a strict component-aware descendant of the isolation root.
    pub fn new(sandbox: &WorkspaceSandbox) -> Result<Self, SandboxError> {
        validate_supplied_directory(sandbox.root())?;
        validate_supplied_directory(sandbox.isolation_root())?;
        let root = sandbox.root().canonicalize().map_err(|_| SandboxError)?;
        let isolation_root = sandbox
            .isolation_root()
            .canonicalize()
            .map_err(|_| SandboxError)?;
        if root == isolation_root || !path_within(&root, &isolation_root) {
            return Err(SandboxError);
        }
        Ok(Self {
            root,
            isolation_root,
        })
    }

    /// Returns the authoritative canonical workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the authoritative canonical isolation root.
    #[must_use]
    pub fn isolation_root(&self) -> &Path {
        &self.isolation_root
    }
}

fn validate_supplied_directory(path: &Path) -> Result<(), SandboxError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SandboxError)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SandboxError);
    }
    Ok(())
}

/// Workspace-aware direct file-tool gate.
pub struct WorkspaceSandboxGate<'a> {
    policy: &'a WorkspacePolicy,
    cwd: &'a Path,
}

impl<'a> WorkspaceSandboxGate<'a> {
    /// Creates a direct file-tool gate for one validated workspace and cwd.
    #[must_use]
    pub const fn new(policy: &'a WorkspacePolicy, cwd: &'a Path) -> Self {
        Self { policy, cwd }
    }
}

impl SandboxGate for WorkspaceSandboxGate<'_> {
    fn check(&self, invocation: SandboxInvocation<'_>) -> Result<SandboxDecision, SandboxError> {
        if matches!(
            invocation.internal_name,
            "read_artifact_file" | "write_artifact_file"
        ) {
            return Ok(SandboxDecision::Allow);
        }
        let family = canonical_tool_name(invocation.internal_name);
        if family == "Bash" || !is_file_family(family) {
            return Ok(SandboxDecision::Allow);
        }
        let values = if invocation.internal_name == "apply_patch" {
            let patch = invocation
                .input
                .as_value()
                .get("input")
                .and_then(Value::as_str)
                .ok_or(SandboxError)?;
            crate::builtin::file::patch::scan_paths(patch).map_err(|()| SandboxError)?
        } else if invocation.internal_name == "read_many_files" {
            validate_read_many(invocation.input.as_value())?;
            vec!["."]
        } else {
            path_values(invocation.input.as_value(), family)?
        };
        for value in values {
            let path = canonicalize_invocation_path(self.cwd, value).map_err(|_| SandboxError)?;
            if !path_within(&path, self.policy.root()) {
                return Ok(SandboxDecision::Deny);
            }
        }
        Ok(SandboxDecision::Allow)
    }
}

fn path_values<'a>(value: &'a Value, family: &str) -> Result<Vec<&'a str>, SandboxError> {
    let object = value.as_object().ok_or(SandboxError)?;
    let mut output = Vec::new();
    for key in ["file_path", "path", "notebook_path", "dir_path"] {
        if let Some(value) = object.get(key) {
            output.push(value.as_str().ok_or(SandboxError)?);
        }
    }
    if family == "Glob"
        && !object.contains_key("path")
        && let Some(value) = object.get("pattern").and_then(Value::as_str)
        && pattern_is_path_like(value)
    {
        output.push(value);
    }
    if output.is_empty() && matches!(family, "Glob" | "Grep") {
        output.push(".");
    }
    if output.is_empty() {
        Err(SandboxError)
    } else {
        Ok(output)
    }
}

fn validate_read_many(value: &Value) -> Result<(), SandboxError> {
    let object = value.as_object().ok_or(SandboxError)?;
    for key in ["include", "exclude"] {
        let Some(values) = object.get(key) else {
            continue;
        };
        let values = values.as_array().ok_or(SandboxError)?;
        if values.len() > 32 || (key == "include" && values.is_empty()) {
            return Err(SandboxError);
        }
        for value in values {
            let pattern = value.as_str().ok_or(SandboxError)?;
            if pattern.is_empty()
                || pattern.len() > 4_096
                || pattern.starts_with('/')
                || pattern.contains('\\')
                || pattern.split('/').any(|part| part == "..")
            {
                return Err(SandboxError);
            }
        }
    }
    Ok(())
}

fn pattern_is_path_like(value: &str) -> bool {
    value.starts_with(['/', '.']) || value.contains('/') || value.contains('\\')
}

fn is_file_family(value: &str) -> bool {
    matches!(
        value,
        "Read" | "Write" | "Edit" | "Glob" | "Grep" | "ListDir"
    )
}
