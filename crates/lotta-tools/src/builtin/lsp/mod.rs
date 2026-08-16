//! Configured language-server diagnostics bridge.

use crate::{
    builtin::common,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
    registry::ToolRegistration,
};
use lotta_runtime::ports::{ToolApprovalPolicy, ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

/// Maximum returned diagnostics.
pub const LSP_DIAGNOSTICS_ITEMS_MAX: usize = 10;
/// Maximum bytes in one diagnostic message.
pub const LSP_DIAGNOSTIC_MESSAGE_BYTES_MAX: usize = 16 * 1024;
/// Maximum accepted source line and character index.
pub const LSP_POSITION_VALUE_MAX: u32 = 10_000_000;

/// LSP diagnostic severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum DiagnosticSeverity {
    /// Error.
    Error,
    /// Warning.
    Warning,
    /// Information.
    Information,
    /// Hint.
    Hint,
}

/// One bounded diagnostics-only result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Zero-based line.
    pub line: u32,
    /// Zero-based character.
    pub character: u32,
    /// Human-readable message.
    pub message: String,
}

/// Fixed language server port errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguageServerError {
    /// Extension has no configured server.
    Unconfigured,
    /// Request cancelled.
    Cancelled,
    /// Request timed out.
    Timeout,
    /// Server crashed or channel closed.
    Crashed,
    /// Result violated a bound.
    Limit,
    /// Path or input invalid.
    Invalid,
}

/// Future returned by diagnostics-only servers.
pub type DiagnosticsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Diagnostic>, LanguageServerError>> + Send + 'a>>;

/// Explicit diagnostics-only language server boundary.
pub trait LanguageServerPort: Send + Sync {
    /// Reads current diagnostics for one canonical workspace-confined file.
    fn diagnostics(
        &self,
        file: &'_ Path,
        cancellation: tokio_util::sync::CancellationToken,
        timeout: std::time::Duration,
    ) -> DiagnosticsFuture<'_>;
}

/// Explicit immutable extension-to-server registry.
pub struct LanguageServerRegistry {
    workspace_root: PathBuf,
    servers: BTreeMap<String, Arc<dyn LanguageServerPort>>,
}

impl LanguageServerRegistry {
    /// Creates a registry from a canonical workspace and normalized extensions.
    ///
    /// # Errors
    /// Rejects noncanonical roots and malformed or duplicate extensions.
    pub fn new(
        workspace_root: PathBuf,
        servers: impl IntoIterator<Item = (String, Arc<dyn LanguageServerPort>)>,
    ) -> Result<Self, LanguageServerError> {
        if !workspace_root.is_absolute()
            || workspace_root
                .canonicalize()
                .map_err(|_| LanguageServerError::Invalid)?
                != workspace_root
        {
            return Err(LanguageServerError::Invalid);
        }
        let mut indexed = BTreeMap::new();
        for (extension, server) in servers {
            if extension.is_empty()
                || extension.len() > 64
                || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                || indexed
                    .insert(extension.to_ascii_lowercase(), server)
                    .is_some()
            {
                return Err(LanguageServerError::Invalid);
            }
        }
        Ok(Self {
            workspace_root,
            servers: indexed,
        })
    }

    fn resolve(
        &self,
        input: &str,
    ) -> Result<(PathBuf, Arc<dyn LanguageServerPort>), LanguageServerError> {
        if input.is_empty() || input.contains('\0') {
            return Err(LanguageServerError::Invalid);
        }
        let joined = if Path::new(input).is_absolute() {
            PathBuf::from(input)
        } else {
            self.workspace_root.join(input)
        };
        let canonical = joined
            .canonicalize()
            .map_err(|_| LanguageServerError::Invalid)?;
        if !canonical.starts_with(&self.workspace_root) || !canonical.is_file() {
            return Err(LanguageServerError::Invalid);
        }
        let extension = canonical
            .extension()
            .and_then(|value| value.to_str())
            .ok_or(LanguageServerError::Unconfigured)?
            .to_ascii_lowercase();
        let server = self
            .servers
            .get(&extension)
            .cloned()
            .ok_or(LanguageServerError::Unconfigured)?;
        Ok((canonical, server))
    }
}

struct LspExecutor {
    registry: Arc<LanguageServerRegistry>,
}

#[derive(Deserialize)]
struct ReadInput {
    file_path: String,
}

impl ToolExecutor for LspExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let registry = Arc::clone(&self.registry);
        let value = request.input.as_value().clone();
        Box::pin(async move {
            let input: ReadInput = serde_json::from_value(value).map_err(|_| ExecutorError)?;
            let (file, server) = registry.resolve(&input.file_path).map_err(map_executor)?;
            match server
                .diagnostics(&file, request.cancellation, request.deadline.get())
                .await
                .and_then(validate_diagnostics)
            {
                Ok(diagnostics) => format_diagnostics(diagnostics).map(RawToolOutcome::Success),
                Err(error) => failure(error),
            }
        })
    }
}

fn validate_diagnostics(values: Vec<Diagnostic>) -> Result<Vec<Diagnostic>, LanguageServerError> {
    if values.len() > LSP_DIAGNOSTICS_ITEMS_MAX {
        return Err(LanguageServerError::Limit);
    }
    for value in &values {
        if value.line > LSP_POSITION_VALUE_MAX
            || value.character > LSP_POSITION_VALUE_MAX
            || value.message.is_empty()
            || value.message.len() > LSP_DIAGNOSTIC_MESSAGE_BYTES_MAX
            || value.message.contains('\0')
        {
            return Err(LanguageServerError::Limit);
        }
    }
    Ok(values)
}

fn format_diagnostics(values: Vec<Diagnostic>) -> Result<String, ExecutorError> {
    let mut output = String::new();
    for value in values
        .into_iter()
        .filter(|value| value.severity == DiagnosticSeverity::Error)
    {
        let line = value.line.checked_add(1).ok_or(ExecutorError)?;
        let character = value.character.checked_add(1).ok_or(ExecutorError)?;
        writeln!(output, "ERROR [{line}:{character}] {}", value.message)
            .map_err(|_| ExecutorError)?;
    }
    Ok(output)
}

fn failure(error: LanguageServerError) -> Result<RawToolOutcome, ExecutorError> {
    let (code, message) = match error {
        LanguageServerError::Cancelled => ("lsp_cancelled", "Language server request cancelled."),
        LanguageServerError::Timeout => ("lsp_timeout", "Language server request timed out."),
        LanguageServerError::Crashed => ("lsp_crashed", "Language server unavailable."),
        LanguageServerError::Unconfigured => ("lsp_unconfigured", "No language server configured."),
        LanguageServerError::Limit => ("lsp_limit", "Language server result exceeds a limit."),
        LanguageServerError::Invalid => ("lsp_invalid", "Language server input is invalid."),
    };
    Ok(RawToolOutcome::Failure(ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(code.to_owned()).map_err(|_| ExecutorError)?,
        message: ToolOutcomeMessage::new(message.to_owned()).map_err(|_| ExecutorError)?,
    }))
}

fn map_executor(_: LanguageServerError) -> ExecutorError {
    ExecutorError
}

/// Builds the exact diagnostics-only `ReadLSP` registration.
///
/// # Errors
/// Rejects malformed pinned assets.
pub fn registration(
    registry: Arc<LanguageServerRegistry>,
) -> Result<ToolRegistration, LanguageServerError> {
    let executor: Arc<dyn ToolExecutor> = Arc::new(LspExecutor { registry });
    common::registration(
        "ReadLSP",
        include_str!("assets/schemas/ReadLSP.json"),
        include_str!("assets/descriptions/ReadLSP.md"),
        ToolApprovalPolicy::Never,
        "read",
        executor,
    )
    .map_err(|()| LanguageServerError::Invalid)
}

#[cfg(test)]
mod tests;
