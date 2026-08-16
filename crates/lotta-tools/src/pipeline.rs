//! Ordered tool execution pipeline.
//!
//! Permission and sandbox are explicit allow stubs; Tasks 34–35 replace their bodies.

use crate::{
    clamp::{ClampError, OverflowWriter, clamp_text},
    limits,
    registry::RegistrySnapshot,
    scrub::{ScrubError, scrub_text},
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    bounds::TOOL_NAME_BYTES_MAX,
    ports::{
        ModelFacingToolName, ToolDefinition, ToolExecutionOwner, ToolOutcome, ToolOutcomeCode,
        ToolOutcomeMessage, ToolResultText, ToolTimeout, ValidatedToolInput,
    },
};
use serde_json::Value;
use std::{collections::BTreeSet, fmt, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

const SECRET_NAMES_MAX: usize = 256;
const SECRET_NAME_BYTES_MAX: usize = 4_096;
const SECRET_VALUE_BYTES_MAX: usize = 256 * 1024;

/// Exact ordered execution stages after the two-event preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineStage {
    /// Pre-tool hook.
    PreHook,
    /// Permission gate.
    Permission,
    /// Sandbox gate.
    Sandbox,
    /// Private secret delivery construction.
    SecretSubstitution,
    /// Raw executor.
    Executor,
    /// Post-tool hook.
    PostHook,
    /// Secret scrub.
    Scrub,
    /// Model-facing clamp.
    Clamp,
    /// Persistence sink.
    Persist,
    /// Emit sink.
    Emit,
}

/// Separate preflight trace events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightEvent {
    /// Registry name resolution.
    NameResolution,
    /// Declared JSON Schema validation.
    SchemaValidation,
}

/// Trace event carrying no input, output, or secret text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceEvent {
    /// Preflight event.
    Preflight(PreflightEvent),
    /// One exact execution stage.
    Stage(PipelineStage),
}

/// Minimal enum-only pipeline trace sink.
pub trait TraceSink: Send + Sync {
    /// Records one non-sensitive event.
    fn record(&self, event: TraceEvent);
}

/// Final-outcome sink used by persistence and emission.
pub trait OutcomeSink: Send + Sync {
    /// Receives only model name and final scrubbed, clamped outcome.
    ///
    /// # Errors
    /// Returns a fixed pipeline error when the sink cannot accept the outcome.
    fn record(&self, model_name: &str, outcome: &ToolOutcome) -> Result<(), PipelineError>;
}

/// Lookup seam for only referenced secret names.
pub trait SecretResolver: Send + Sync {
    /// Resolves one validated environment-style name; unknown names return `None`.
    ///
    /// # Errors
    /// Returns a fixed pipeline error when resolution infrastructure fails.
    fn resolve(&self, name: &str) -> Result<Option<String>, PipelineError>;
}

/// Where private secrets may be delivered, derived from execution owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretDeliveryKind {
    /// Native child environment.
    ChildEnvironment,
    /// External provider/controller request.
    ProviderRequest,
}

/// Private, non-serializable executor-only secret delivery.
#[derive(Clone)]
pub struct SecretDelivery {
    kind: SecretDeliveryKind,
    values: Arc<Vec<(String, String)>>,
}

impl SecretDelivery {
    /// Returns delivery destination without exposing values.
    #[must_use]
    pub const fn kind(&self) -> SecretDeliveryKind {
        self.kind
    }
    /// Executor-only value lookup.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }
}

impl fmt::Debug for SecretDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretDelivery([REDACTED])")
    }
}

/// Raw executor request, distinct from the already-clamped runtime port contract.
pub struct RawToolExecutionRequest {
    /// Validated bounded input, retaining placeholders.
    pub input: ValidatedToolInput,
    /// Explicit cancellation token.
    pub cancellation: CancellationToken,
    /// Definition deadline.
    pub deadline: ToolTimeout,
    /// Shared resolved definition.
    pub definition: Arc<ToolDefinition>,
    /// Resolved model-facing name.
    pub model_name: ModelFacingToolName,
    secrets: SecretDelivery,
}

impl RawToolExecutionRequest {
    /// Executor-only private delivery accessor.
    #[must_use]
    pub fn secret_delivery(&self) -> &SecretDelivery {
        &self.secrets
    }
}

/// Raw executor result where success owns unclamped output.
pub enum RawToolOutcome {
    /// Raw successful UTF-8 output.
    Success(String),
    /// Already-bounded non-success outcome.
    Failure(ToolOutcome),
}

/// Future returned by raw executors.
pub type ExecutorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RawToolOutcome, ExecutorError>> + Send + 'a>>;

/// Executor boundary capable of carrying raw success output.
pub trait ToolExecutor: Send + Sync {
    /// Executes one validated owning request.
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_>;
}

/// Fixed executor infrastructure error with no retained detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorError;

/// Extension owner category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionOwnerKind {
    /// Hook owner.
    Hook,
    /// Mod owner.
    Mod,
}

/// Validated bounded owner identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerId(String);
impl OwnerId {
    /// Validates a nonempty, NUL-free identifier.
    ///
    /// # Errors
    /// Returns [`PipelineError::OwnerIdentity`] for invalid input.
    pub fn new(value: String) -> Result<Self, PipelineError> {
        if value.is_empty() || value.len() > TOOL_NAME_BYTES_MAX.value || value.contains('\0') {
            return Err(PipelineError::OwnerIdentity);
        }
        Ok(Self(value))
    }
    /// Borrows the owner identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded hook/mod owner identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionOwner {
    kind: ExtensionOwnerKind,
    id: OwnerId,
}
impl ExtensionOwner {
    /// Constructs a hook owner.
    ///
    /// # Errors
    /// Returns [`PipelineError::OwnerIdentity`] for invalid identifiers.
    pub fn hook(id: String) -> Result<Self, PipelineError> {
        Ok(Self {
            kind: ExtensionOwnerKind::Hook,
            id: OwnerId::new(id)?,
        })
    }
    /// Constructs a mod owner.
    ///
    /// # Errors
    /// Returns [`PipelineError::OwnerIdentity`] for invalid identifiers.
    pub fn extension_mod(id: String) -> Result<Self, PipelineError> {
        Ok(Self {
            kind: ExtensionOwnerKind::Mod,
            id: OwnerId::new(id)?,
        })
    }
    /// Returns the owner category.
    #[must_use]
    pub const fn kind(&self) -> ExtensionOwnerKind {
        self.kind
    }
    /// Returns the bounded identifier.
    #[must_use]
    pub const fn id(&self) -> &OwnerId {
        &self.id
    }
}

/// Pre-hook decision.
pub enum PreHookResult {
    /// Continue with current input.
    Allow,
    /// Replace input, requiring both admission and schema revalidation.
    Replace(BoundedJsonValue),
}

/// Minimal synchronous hook seam.
pub trait PipelineHooks: Send + Sync {
    /// Runs before policy.
    ///
    /// # Errors
    /// Returns bounded owner attribution on hook failure.
    fn pre(&self, input: &ValidatedToolInput) -> Result<PreHookResult, OwnerFailure>;
    /// Observes non-sensitive executor status without output or secrets.
    ///
    /// # Errors
    /// Returns bounded owner attribution on hook failure.
    fn post(&self, status: PostHookStatus) -> Result<(), OwnerFailure>;
}

/// Non-sensitive post-hook status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostHookStatus {
    /// Executor produced an admitted result.
    Completed,
    /// Executor or raw-result admission failed.
    Failed,
}

/// Fixed owner-attributed hook failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerFailure {
    /// Failing bounded owner.
    pub owner: ExtensionOwner,
    /// Fixed machine code.
    pub code: &'static str,
}

/// No-op hook implementation.
pub struct NoopHooks;
impl PipelineHooks for NoopHooks {
    fn pre(&self, _: &ValidatedToolInput) -> Result<PreHookResult, OwnerFailure> {
        Ok(PreHookResult::Allow)
    }
    fn post(&self, _: PostHookStatus) -> Result<(), OwnerFailure> {
        Ok(())
    }
}

/// Typed fixed pipeline failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineError {
    /// Model-facing name was not registered.
    NameResolution,
    /// Input failed compact size admission.
    InputLimit,
    /// Input failed the declared JSON Schema.
    SchemaValidation,
    /// Hook or mod failed with bounded attribution.
    Owner(OwnerFailure),
    /// Secret bounds or resolution failed.
    SecretDelivery,
    /// Raw result exceeded definition/global byte limits.
    ResultLimit,
    /// Scrubbing failed.
    Scrub,
    /// Clamp or overflow write failed.
    Clamp,
    /// Persistence failed.
    Persist,
    /// Emit failed.
    Emit,
    /// Executor infrastructure failed.
    Executor,
    /// Extension owner identity was invalid.
    OwnerIdentity,
}

/// Owning pipeline request and its explicit effect seams.
pub struct PipelineRequest<'a> {
    /// Immutable registry snapshot captured by caller.
    pub registry: Arc<RegistrySnapshot>,
    /// Exposed tool name.
    pub model_name: &'a str,
    /// Bounded JSON input.
    pub input: BoundedJsonValue,
    /// Cancellation token.
    pub cancellation: CancellationToken,
    /// Hook seam.
    pub hooks: &'a dyn PipelineHooks,
    /// Secret resolver.
    pub secrets: &'a dyn SecretResolver,
    /// Enum-only trace sink.
    pub trace: &'a dyn TraceSink,
    /// Overflow writer.
    pub overflow: &'a dyn OverflowWriter,
    /// Persistence sink.
    pub persistence: &'a dyn OutcomeSink,
    /// Emit sink.
    pub emit: &'a dyn OutcomeSink,
}

/// Runs preflight and all ten exact stages.
///
/// # Errors
/// Returns a fixed [`PipelineError`] at the first failed admission, extension, execution, or sink.
pub async fn execute(request: PipelineRequest<'_>) -> Result<ToolOutcome, PipelineError> {
    let PipelineRequest {
        registry,
        model_name,
        input,
        cancellation,
        hooks,
        secrets,
        trace,
        overflow,
        persistence,
        emit,
    } = request;
    let services = PipelineServices {
        registry,
        model_name,
        cancellation,
        hooks,
        secrets,
        trace,
        overflow,
        persistence,
        emit,
    };
    let (tool, input) = prepare(&services, input)?;
    let (raw, delivery) = execute_stages(&services, &tool, input).await?;
    finalize(&services, &tool.definition, raw, &delivery)
}

struct PipelineServices<'a> {
    registry: Arc<RegistrySnapshot>,
    model_name: &'a str,
    cancellation: CancellationToken,
    hooks: &'a dyn PipelineHooks,
    secrets: &'a dyn SecretResolver,
    trace: &'a dyn TraceSink,
    overflow: &'a dyn OverflowWriter,
    persistence: &'a dyn OutcomeSink,
    emit: &'a dyn OutcomeSink,
}

fn prepare(
    request: &PipelineServices<'_>,
    value: BoundedJsonValue,
) -> Result<(Arc<crate::registry::RegisteredTool>, ValidatedToolInput), PipelineError> {
    request
        .trace
        .record(TraceEvent::Preflight(PreflightEvent::NameResolution));
    let tool = request
        .registry
        .by_model(request.model_name)
        .cloned()
        .ok_or(PipelineError::NameResolution)?;
    request
        .trace
        .record(TraceEvent::Preflight(PreflightEvent::SchemaValidation));
    let mut input = validate_input(value, &tool.definition)?;
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::PreHook));
    let decision = request.hooks.pre(&input).map_err(PipelineError::Owner)?;
    if let PreHookResult::Replace(value) = decision {
        input = validate_input(value, &tool.definition)?;
    }
    Ok((tool, input))
}

async fn execute_stages(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: ValidatedToolInput,
) -> Result<(RawToolOutcome, SecretDelivery), PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Permission));
    permission_allow();
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Sandbox));
    sandbox_allow();
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::SecretSubstitution));
    let delivery = resolve_secrets(
        input.as_value(),
        tool.definition.execution_owner,
        request.secrets,
    )?;
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Executor));
    let raw = tool
        .executor
        .execute(RawToolExecutionRequest {
            input,
            cancellation: request.cancellation.clone(),
            deadline: tool.definition.timeout,
            definition: Arc::clone(&tool.definition),
            model_name: tool.model_name.clone(),
            secrets: delivery.clone(),
        })
        .await
        .map_err(|_| PipelineError::Executor)
        .and_then(|value| admit_raw(value, &tool.definition));
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::PostHook));
    let status = post_hook_status(&raw);
    request.hooks.post(status).map_err(PipelineError::Owner)?;
    Ok((raw?, delivery))
}

fn post_hook_status(raw: &Result<RawToolOutcome, PipelineError>) -> PostHookStatus {
    match raw {
        Ok(RawToolOutcome::Success(_)) => PostHookStatus::Completed,
        Ok(RawToolOutcome::Failure(_)) | Err(_) => PostHookStatus::Failed,
    }
}

fn finalize(
    request: &PipelineServices<'_>,
    definition: &ToolDefinition,
    raw: RawToolOutcome,
    delivery: &SecretDelivery,
) -> Result<ToolOutcome, PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Scrub));
    let scrubbed = scrub_outcome(raw, delivery)?;
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Clamp));
    let final_outcome = clamp_outcome(scrubbed, definition, request.overflow)?;
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Persist));
    request
        .persistence
        .record(request.model_name, &final_outcome)
        .map_err(|_| PipelineError::Persist)?;
    request.trace.record(TraceEvent::Stage(PipelineStage::Emit));
    request
        .emit
        .record(request.model_name, &final_outcome)
        .map_err(|_| PipelineError::Emit)?;
    Ok(final_outcome)
}

fn validate_input(
    value: BoundedJsonValue,
    definition: &ToolDefinition,
) -> Result<ValidatedToolInput, PipelineError> {
    limits::check_tool_input_value(&value).map_err(|_| PipelineError::InputLimit)?;
    let input = ValidatedToolInput::new(value).map_err(|_| PipelineError::InputLimit)?;
    let validator = jsonschema::validator_for(definition.input_schema.as_value())
        .map_err(|_| PipelineError::SchemaValidation)?;
    if !validator.is_valid(input.as_value()) {
        return Err(PipelineError::SchemaValidation);
    }
    Ok(input)
}

fn admit_raw(
    raw: RawToolOutcome,
    definition: &ToolDefinition,
) -> Result<RawToolOutcome, PipelineError> {
    match &raw {
        RawToolOutcome::Success(text) => {
            if text.len() > definition.output_limit.bytes_max() {
                return Err(PipelineError::ResultLimit);
            }
            limits::check_tool_result(text.len()).map_err(|_| PipelineError::ResultLimit)?;
        }
        RawToolOutcome::Failure(ToolOutcome::Success { .. }) => {
            return Err(PipelineError::Executor);
        }
        RawToolOutcome::Failure(_) => {}
    }
    Ok(raw)
}

fn permission_allow() {}
fn sandbox_allow() {}

fn resolve_secrets(
    value: &Value,
    owner: ToolExecutionOwner,
    resolver: &dyn SecretResolver,
) -> Result<SecretDelivery, PipelineError> {
    let names = placeholder_names(value)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(names.len())
        .map_err(|_| PipelineError::SecretDelivery)?;
    for name in names {
        if let Some(value) = resolver.resolve(&name)? {
            if value.len() > SECRET_VALUE_BYTES_MAX {
                return Err(PipelineError::SecretDelivery);
            }
            let aggregate = values
                .iter()
                .try_fold(0usize, |total, (_, value): &(String, String)| {
                    total.checked_add(value.len())
                })
                .and_then(|total| total.checked_add(value.len()))
                .ok_or(PipelineError::SecretDelivery)?;
            if aggregate > limits::SECRET_DELIVERY_BYTES_MAX {
                return Err(PipelineError::SecretDelivery);
            }
            if !value.is_empty() {
                values.push((name, value));
            }
        }
    }
    let kind = if owner == ToolExecutionOwner::Rust {
        SecretDeliveryKind::ChildEnvironment
    } else {
        SecretDeliveryKind::ProviderRequest
    };
    Ok(SecretDelivery {
        kind,
        values: Arc::new(values),
    })
}

fn placeholder_names(root: &Value) -> Result<BTreeSet<String>, PipelineError> {
    let stack_max = limits::TOOL_INPUT_BYTES_MAX.value;
    let mut stack = Vec::new();
    stack
        .try_reserve(1)
        .map_err(|_| PipelineError::SecretDelivery)?;
    stack.push(root);
    let mut names = BTreeSet::new();
    let mut name_bytes = 0usize;
    while let Some(value) = stack.pop() {
        match value {
            Value::String(text) => scan_names(text.as_bytes(), &mut names, &mut name_bytes)?,
            Value::Array(items) => push_values(&mut stack, items.iter(), stack_max)?,
            Value::Object(items) => push_values(&mut stack, items.values(), stack_max)?,
            _ => {}
        }
    }
    Ok(names)
}

fn push_values<'a, I>(
    stack: &mut Vec<&'a Value>,
    values: I,
    stack_max: usize,
) -> Result<(), PipelineError>
where
    I: ExactSizeIterator<Item = &'a Value>,
{
    let next = stack
        .len()
        .checked_add(values.len())
        .ok_or(PipelineError::SecretDelivery)?;
    if next > stack_max {
        return Err(PipelineError::SecretDelivery);
    }
    stack
        .try_reserve(values.len())
        .map_err(|_| PipelineError::SecretDelivery)?;
    stack.extend(values);
    Ok(())
}

fn scan_names(
    bytes: &[u8],
    names: &mut BTreeSet<String>,
    name_bytes: &mut usize,
) -> Result<(), PipelineError> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() && is_secret_start(bytes[index + 1]) {
            let start = index + 1;
            let mut end = start + 1;
            while end < bytes.len() && is_secret_continue(bytes[end]) {
                end += 1;
            }
            add_name(&bytes[start..end], names, name_bytes)?;
            index = end;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn add_name(
    bytes: &[u8],
    names: &mut BTreeSet<String>,
    name_bytes: &mut usize,
) -> Result<(), PipelineError> {
    let name = std::str::from_utf8(bytes).map_err(|_| PipelineError::SecretDelivery)?;
    if names.contains(name) {
        return Ok(());
    }
    let next_bytes = name_bytes
        .checked_add(name.len())
        .ok_or(PipelineError::SecretDelivery)?;
    if names.len() >= SECRET_NAMES_MAX || next_bytes > SECRET_NAME_BYTES_MAX {
        return Err(PipelineError::SecretDelivery);
    }
    names.insert(name.to_owned());
    *name_bytes = next_bytes;
    Ok(())
}

fn is_secret_start(value: u8) -> bool {
    value == b'_' || value.is_ascii_uppercase()
}
fn is_secret_continue(value: u8) -> bool {
    is_secret_start(value) || value.is_ascii_digit()
}

fn scrub_outcome(
    raw: RawToolOutcome,
    delivery: &SecretDelivery,
) -> Result<RawToolOutcome, PipelineError> {
    let mut pairs = Vec::new();
    pairs
        .try_reserve_exact(delivery.values.len())
        .map_err(|_| PipelineError::Scrub)?;
    for (name, value) in delivery.values.iter() {
        pairs.push((name.as_str(), value.as_str()));
    }
    match raw {
        RawToolOutcome::Success(text) => Ok(RawToolOutcome::Success(
            scrub_text(&text, &pairs).map_err(map_scrub)?,
        )),
        RawToolOutcome::Failure(outcome) => {
            Ok(RawToolOutcome::Failure(scrub_failure(outcome, &pairs)?))
        }
    }
}

fn scrub_failure(
    outcome: ToolOutcome,
    pairs: &[(&str, &str)],
) -> Result<ToolOutcome, PipelineError> {
    let message = |value: ToolOutcomeMessage| {
        ToolOutcomeMessage::new(scrub_text(value.as_str(), pairs).map_err(map_scrub)?)
            .map_err(|_| PipelineError::Scrub)
    };
    Ok(match outcome {
        ToolOutcome::Success { .. } => return Err(PipelineError::Executor),
        ToolOutcome::UserDenied { message: value } => ToolOutcome::UserDenied {
            message: message(value)?,
        },
        ToolOutcome::Interrupted { message: value } => ToolOutcome::Interrupted {
            message: message(value)?,
        },
        ToolOutcome::Timeout { message: value } => ToolOutcome::Timeout {
            message: message(value)?,
        },
        ToolOutcome::ValidationFailure { message: value } => ToolOutcome::ValidationFailure {
            message: message(value)?,
        },
        ToolOutcome::SandboxDenied { message: value } => ToolOutcome::SandboxDenied {
            message: message(value)?,
        },
        ToolOutcome::SpawnFailure { message: value } => ToolOutcome::SpawnFailure {
            message: message(value)?,
        },
        ToolOutcome::ToolDefinedError {
            code,
            message: value,
        } => ToolOutcome::ToolDefinedError {
            code: ToolOutcomeCode::new(scrub_text(code.as_str(), pairs).map_err(map_scrub)?)
                .map_err(|_| PipelineError::Scrub)?,
            message: message(value)?,
        },
    })
}

fn map_scrub(_: ScrubError) -> PipelineError {
    PipelineError::Scrub
}

fn clamp_outcome(
    raw: RawToolOutcome,
    definition: &ToolDefinition,
    writer: &dyn OverflowWriter,
) -> Result<ToolOutcome, PipelineError> {
    match raw {
        RawToolOutcome::Failure(outcome) => Ok(outcome),
        RawToolOutcome::Success(text) => {
            let content = clamp_text(
                definition.internal_name.as_str(),
                &text,
                definition.output_limit,
                writer,
            )
            .map_err(map_clamp)?;
            let content = ToolResultText::new(content, definition.output_limit)
                .map_err(|_| PipelineError::Clamp)?;
            Ok(ToolOutcome::Success { content })
        }
    }
}
fn map_clamp(_: ClampError) -> PipelineError {
    PipelineError::Clamp
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
#[tokio::test]
async fn stage_order() {
    tests::stage_order_case().await;
}

#[cfg(test)]
#[tokio::test]
async fn secret_substitution() {
    tests::secret_substitution_case().await;
}

#[cfg(test)]
#[tokio::test]
async fn owner_attribution() {
    tests::owner_attribution_case().await;
}
