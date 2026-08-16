use lotta_runtime::RuntimeError;

/// Returns the fixed error used when workspace sandboxing is unavailable.
#[must_use]
pub fn unsupported_workspace_sandbox() -> RuntimeError {
    RuntimeError::Unsupported {
        context: "workspace sandbox".into(),
    }
}
