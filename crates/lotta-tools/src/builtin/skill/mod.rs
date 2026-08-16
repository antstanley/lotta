//! Registered skill-loader tool bridge.

use crate::{
    builtin::common,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
    registry::ToolRegistration,
};
use lotta_runtime::ports::{ToolApprovalPolicy, ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use std::{future::Future, pin::Pin, sync::Arc};

/// Maximum loaded skill model-output bytes before pipeline clamp.
pub const SKILL_OUTPUT_BYTES_MAX: usize = 1024 * 1024;

/// Loaded registered skill content independent of filesystem representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredSkillContent {
    /// Exact complete `SKILL.md` instructions.
    pub instructions: String,
    /// Deterministically ordered companion files.
    pub companions: Vec<RegisteredSkillCompanion>,
}

/// One exact registered skill companion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredSkillCompanion {
    /// Slash-normalized registered relative path.
    pub relative_path: String,
    /// Exact file bytes.
    pub bytes: Vec<u8>,
}

/// Fixed registered skill-loader errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillPortError {
    /// ID is absent from the selected registry.
    Unknown,
    /// Registered content failed confinement or changed.
    Invalid,
    /// Named output bound reached.
    Limit,
}

/// Future returned by the registered skill loader.
pub type SkillLoadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RegisteredSkillContent, SkillPortError>> + Send + 'a>>;

/// Explicit Task 36 registry/loader port; unknown IDs must fail before filesystem work.
pub trait RegisteredSkillPort: Send + Sync {
    /// Loads one already-registered ID with complete bounded content.
    fn load(&self, id: &str) -> SkillLoadFuture<'_>;
}

struct SkillExecutor {
    port: Arc<dyn RegisteredSkillPort>,
}

impl ToolExecutor for SkillExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let port = Arc::clone(&self.port);
        let value = request.input.as_value().clone();
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return failure("skill_interrupted", "Skill load interrupted.");
            }
            let id = value
                .get("skill")
                .and_then(serde_json::Value::as_str)
                .filter(|id| valid_id(id))
                .ok_or(ExecutorError)?;
            match port.load(id).await {
                Ok(content) => render(id, content).map(RawToolOutcome::Success),
                Err(SkillPortError::Unknown) => {
                    failure("unknown_skill", "Skill is not registered.")
                }
                Err(SkillPortError::Invalid) => {
                    failure("invalid_skill", "Registered skill could not be loaded.")
                }
                Err(SkillPortError::Limit) => {
                    failure("skill_limit", "Registered skill exceeds a resource limit.")
                }
            }
        })
    }
}

fn render(id: &str, content: RegisteredSkillContent) -> Result<String, ExecutorError> {
    let mut output = format!("<skill name=\"{}\">\n{}", escape(id), content.instructions);
    for companion in content.companions {
        if !valid_relative(&companion.relative_path) {
            return Err(ExecutorError);
        }
        output.push_str("\n<companion path=\"");
        output.push_str(&escape(&companion.relative_path));
        output.push_str("\">\n");
        output.push_str(&String::from_utf8_lossy(&companion.bytes));
        output.push_str("\n</companion>");
        if output.len() > SKILL_OUTPUT_BYTES_MAX {
            return Err(ExecutorError);
        }
    }
    output.push_str("\n</skill>");
    if output.len() > SKILL_OUTPUT_BYTES_MAX {
        return Err(ExecutorError);
    }
    Ok(output)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.split('/').any(|part| matches!(part, "" | "." | ".."))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn failure(code: &str, message: &str) -> Result<RawToolOutcome, ExecutorError> {
    Ok(RawToolOutcome::Failure(ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(code.to_owned()).map_err(|_| ExecutorError)?,
        message: ToolOutcomeMessage::new(message.to_owned()).map_err(|_| ExecutorError)?,
    }))
}

/// Builds the exact `Skill` registration.
///
/// # Errors
/// Rejects malformed pinned assets.
pub fn registration(
    port: Arc<dyn RegisteredSkillPort>,
) -> Result<ToolRegistration, SkillPortError> {
    let executor: Arc<dyn ToolExecutor> = Arc::new(SkillExecutor { port });
    common::registration(
        "Skill",
        include_str!("assets/schemas/Skill.json"),
        include_str!("assets/descriptions/Skill.md"),
        ToolApprovalPolicy::Never,
        "read",
        executor,
    )
    .map_err(|()| SkillPortError::Invalid)
}

#[cfg(test)]
mod tests;
