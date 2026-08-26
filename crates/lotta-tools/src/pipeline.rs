//! Ordered tool execution pipeline.
//!
//! Permission and sandbox gates run before secret resolution and execution.

use crate::{
    clamp::{ClampError, OverflowWriter, clamp_text},
    limits,
    permissions::{PermissionDecision, PermissionGate, PermissionInvocation},
    registry::RegistrySnapshot,
    sandbox::{SandboxDecision, SandboxGate, SandboxInvocation},
    scrub::{ScrubError, scrub_text},
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    hooks::{HookEvent, HookFailure, HookLifecycle, HookOutcome, HookPayload, HookRuntime},
    ports::{
        ModelFacingToolName, ToolApprovalGrant, ToolCallId, ToolDefinition, ToolExecutionOwner,
        ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage, ToolResultText, ToolTimeout,
        ValidatedToolInput,
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
    /// Permission-request hook at the policy gate.
    PermissionHook,
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

#[doc(hidden)]
#[must_use]
pub fn test_empty_secret_delivery() -> SecretDelivery {
    SecretDelivery {
        kind: SecretDeliveryKind::ChildEnvironment,
        values: Arc::new(Vec::new()),
    }
}

impl fmt::Debug for SecretDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretDelivery([REDACTED])")
    }
}

/// Raw executor request, distinct from the already-clamped runtime port contract.
pub struct RawToolExecutionRequest {
    /// Genuine caller-supplied provider invocation identity.
    pub tool_call_id: ToolCallId,
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
    pub(crate) secrets: SecretDelivery,
}

impl RawToolExecutionRequest {
    /// Builds a raw request without secret delivery for integration tests.
    #[doc(hidden)]
    #[must_use]
    pub fn without_secrets(
        tool_call_id: ToolCallId,
        input: ValidatedToolInput,
        cancellation: CancellationToken,
        deadline: ToolTimeout,
        definition: Arc<ToolDefinition>,
        model_name: ModelFacingToolName,
    ) -> Self {
        Self {
            tool_call_id,
            input,
            cancellation,
            deadline,
            definition,
            model_name,
            secrets: test_empty_secret_delivery(),
        }
    }

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

/// Typed fixed pipeline failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineError {
    /// Model-facing name was not registered.
    NameResolution,
    /// Input failed compact size admission.
    InputLimit,
    /// Input failed the declared JSON Schema.
    SchemaValidation,
    /// Hook failed with stable owner and hook attribution.
    Hook(HookFailure),
    /// Permission policy denied the invocation or rejected unsafe input.
    PermissionDenied,
    /// Permission policy requires interactive approval.
    ApprovalRequired,
    /// Sandbox gate infrastructure failed.
    Sandbox,
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
    /// A typed hook blocked the action.
    HookBlocked,
}

/// Owning pipeline request and its explicit effect seams.
pub struct PipelineRequest<'a> {
    /// Genuine provider invocation identity generated by the caller.
    pub tool_call_id: ToolCallId,
    /// Internal exact-call interactive approval proof.
    pub approval_grant: ToolApprovalGrant,
    /// Immutable registry snapshot captured by caller.
    pub registry: Arc<RegistrySnapshot>,
    /// Exposed tool name.
    pub model_name: &'a str,
    /// Bounded JSON input.
    pub input: BoundedJsonValue,
    /// Cancellation token.
    pub cancellation: CancellationToken,
    /// Typed asynchronous hook capability.
    pub hook_runtime: &'a dyn HookRuntime,
    /// Permission gate.
    pub permissions: &'a dyn PermissionGate,
    /// Sandbox gate.
    pub sandbox: &'a dyn SandboxGate,
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
        tool_call_id,
        approval_grant,
        registry,
        model_name,
        input,
        cancellation,
        hook_runtime,
        permissions,
        sandbox,
        secrets,
        trace,
        overflow,
        persistence,
        emit,
    } = request;
    let services = PipelineServices {
        tool_call_id,
        approval_grant,
        registry,
        model_name,
        cancellation,
        hook_runtime,
        permissions,
        sandbox,
        secrets,
        trace,
        overflow,
        persistence,
        emit,
    };
    let (tool, input) = prepare(&services, input)?;
    let input = fire_pre_tool(&services, &tool, input).await?;
    let (raw, delivery) = execute_stages(&services, &tool, input).await?;
    finalize(&services, &tool.definition, raw, &delivery)
}

struct PipelineServices<'a> {
    tool_call_id: ToolCallId,
    approval_grant: ToolApprovalGrant,
    registry: Arc<RegistrySnapshot>,
    model_name: &'a str,
    cancellation: CancellationToken,
    hook_runtime: &'a dyn HookRuntime,
    permissions: &'a dyn PermissionGate,
    sandbox: &'a dyn SandboxGate,
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
    let input = validate_input(value, &tool.definition)?;
    Ok((tool, input))
}

async fn fire_pre_tool(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: ValidatedToolInput,
) -> Result<ValidatedToolInput, PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::PreHook));
    let payload = tool_payload(
        HookEvent::PreToolUse,
        request,
        tool,
        Some(&input),
        None,
        None,
    )?;
    match HookLifecycle::new(request.hook_runtime)
        .pre_tool_use(payload, request.cancellation.child_token())
        .await
        .map_err(PipelineError::Hook)?
    {
        HookOutcome::Allow => Ok(input),
        HookOutcome::Block(_) => Err(PipelineError::HookBlocked),
        HookOutcome::Modify(payload) => {
            let value = payload
                .value()
                .get("tool_input")
                .cloned()
                .ok_or(PipelineError::SchemaValidation)?;
            validate_input(
                BoundedJsonValue::new(value).map_err(|_| PipelineError::InputLimit)?,
                &tool.definition,
            )
        }
    }
}

async fn execute_stages(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: ValidatedToolInput,
) -> Result<(RawToolOutcome, SecretDelivery), PipelineError> {
    enforce_execution_permission(request, tool, &input).await?;
    if let Some(outcome) = enforce_sandbox(request, tool, &input).await? {
        return Ok(outcome);
    }
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
    let hook_input = input.clone();
    let raw = tool
        .executor
        .execute(RawToolExecutionRequest {
            tool_call_id: request.tool_call_id.clone(),
            input,
            cancellation: request.cancellation.clone(),
            deadline: tool.definition.timeout,
            definition: Arc::clone(&tool.definition),
            model_name: tool.model_name.clone(),
            secrets: delivery.clone(),
        })
        .await
        .map_err(|_| PipelineError::Executor);
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::PostHook));
    finish_execution(request, tool, &hook_input, raw, delivery).await
}

async fn enforce_execution_permission(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: &ValidatedToolInput,
) -> Result<(), PipelineError> {
    match permission_decision(request, tool, input)? {
        PermissionDecision::Allow => Ok(()),
        PermissionDecision::Deny => Err(PipelineError::PermissionDenied),
        PermissionDecision::Ask
            if request
                .approval_grant
                .matches(&request.tool_call_id, &tool.definition) =>
        {
            Ok(())
        }
        PermissionDecision::Ask => {
            fire_permission_hook(request, tool, input).await?;
            Err(PipelineError::ApprovalRequired)
        }
    }
}

async fn enforce_sandbox(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: &ValidatedToolInput,
) -> Result<Option<(RawToolOutcome, SecretDelivery)>, PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Sandbox));
    let sandbox = request
        .sandbox
        .check(SandboxInvocation {
            internal_name: tool.definition.internal_name.as_str(),
            input,
        })
        .map_err(|_| PipelineError::Sandbox)?;
    if sandbox == SandboxDecision::Deny {
        let (raw, delivery) = sandbox_denied(tool)?;
        request
            .trace
            .record(TraceEvent::Stage(PipelineStage::PostHook));
        fire_post_failure(request, tool, input, "sandbox denied").await?;
        return Ok(Some((raw, delivery)));
    }
    Ok(None)
}

async fn finish_execution(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    hook_input: &ValidatedToolInput,
    raw: Result<RawToolOutcome, PipelineError>,
    delivery: SecretDelivery,
) -> Result<(RawToolOutcome, SecretDelivery), PipelineError> {
    match raw {
        Ok(RawToolOutcome::Success(output)) => {
            if output.len() > lotta_runtime::hooks::HOOK_PAYLOAD_BYTES_MAX / 2 {
                fire_post_failure(request, tool, hook_input, "result limit").await?;
                return Err(PipelineError::ResultLimit);
            }
            let raw = fire_post_success(request, tool, hook_input, output).await?;
            Ok((raw, delivery))
        }
        Ok(RawToolOutcome::Failure(failure)) => {
            let failure = match admit_raw(RawToolOutcome::Failure(failure), &tool.definition) {
                Ok(RawToolOutcome::Failure(failure)) => failure,
                Ok(RawToolOutcome::Success(_)) => unreachable!("failure admission changed variant"),
                Err(error) => {
                    fire_post_failure(request, tool, hook_input, failure_code(&error)).await?;
                    return Err(error);
                }
            };
            fire_post_failure(request, tool, hook_input, "tool failure").await?;
            Ok((RawToolOutcome::Failure(failure), delivery))
        }
        Err(error) => {
            fire_post_failure(request, tool, hook_input, failure_code(&error)).await?;
            Err(error)
        }
    }
}

async fn fire_post_success(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    _input: &ValidatedToolInput,
    output: String,
) -> Result<RawToolOutcome, PipelineError> {
    let result = raw_json(&RawToolOutcome::Success(output.clone()));
    let value = serde_json::json!({
        "event_type": HookEvent::PostToolUse,
        "working_directory": "",
        "tool_name": tool.model_name.as_str(),
        "tool_call_id": request.tool_call_id.as_str(),
        "tool_result": result,
    });
    let payload = HookPayload::new(HookEvent::PostToolUse, value)
        .map_err(|_| hook_boundary_error("payload"))?;
    match HookLifecycle::new(request.hook_runtime)
        .post_tool_use(payload, request.cancellation.child_token())
        .await
        .map_err(PipelineError::Hook)?
    {
        HookOutcome::Allow => admit_raw(RawToolOutcome::Success(output), &tool.definition),
        HookOutcome::Block(_) => Err(PipelineError::HookBlocked),
        HookOutcome::Modify(payload) => {
            let result = payload
                .value()
                .get("tool_result")
                .ok_or(PipelineError::ResultLimit)?;
            if result.get("status").and_then(Value::as_str) != Some("success") {
                return Err(PipelineError::ResultLimit);
            }
            let output = result
                .get("output")
                .and_then(Value::as_str)
                .ok_or(PipelineError::ResultLimit)?
                .to_owned();
            admit_raw(RawToolOutcome::Success(output), &tool.definition)
        }
    }
}

fn failure_code(error: &PipelineError) -> &'static str {
    match error {
        PipelineError::ResultLimit => "result limit",
        _ => "executor failure",
    }
}

async fn fire_post_failure(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    _input: &ValidatedToolInput,
    error: &'static str,
) -> Result<(), PipelineError> {
    let value = serde_json::json!({
        "event_type": HookEvent::PostToolUseFailure,
        "working_directory": "",
        "tool_name": tool.model_name.as_str(),
        "tool_call_id": request.tool_call_id.as_str(),
        "error_message": error,
    });
    let payload = HookPayload::new(HookEvent::PostToolUseFailure, value)
        .map_err(|_| hook_boundary_error("payload"))?;
    match HookLifecycle::new(request.hook_runtime)
        .post_tool_use_failure(payload, request.cancellation.child_token())
        .await
        .map_err(PipelineError::Hook)?
    {
        HookOutcome::Allow => Ok(()),
        HookOutcome::Block(_) => Err(PipelineError::HookBlocked),
        HookOutcome::Modify(_) => Err(hook_boundary_error("illegal_modification")),
    }
}

async fn fire_permission_hook(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: &ValidatedToolInput,
) -> Result<(), PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::PermissionHook));
    let payload = tool_payload(
        HookEvent::PermissionRequest,
        request,
        tool,
        Some(input),
        None,
        None,
    )?;
    let outcome = HookLifecycle::new(request.hook_runtime)
        .permission_request(payload, request.cancellation.child_token())
        .await
        .map_err(PipelineError::Hook)?;
    if matches!(outcome, HookOutcome::Block(_)) {
        Err(PipelineError::HookBlocked)
    } else {
        Ok(())
    }
}

fn permission_decision(
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: &ValidatedToolInput,
) -> Result<PermissionDecision, PipelineError> {
    request
        .trace
        .record(TraceEvent::Stage(PipelineStage::Permission));
    request
        .permissions
        .check(PermissionInvocation::from_definition(
            &tool.definition,
            input,
        ))
        .map_err(|_| PipelineError::PermissionDenied)
}

fn tool_payload(
    event: HookEvent,
    request: &PipelineServices<'_>,
    tool: &crate::registry::RegisteredTool,
    input: Option<&ValidatedToolInput>,
    result: Option<serde_json::Value>,
    error: Option<&str>,
) -> Result<HookPayload, PipelineError> {
    let mut value = serde_json::json!({
        "event_type": serde_json::to_value(event).map_err(|_| PipelineError::Executor)?,
        "working_directory": "",
        "tool_name": tool.model_name.as_str(),
        "tool_call_id": request.tool_call_id.as_str(),
    });
    if let Some(input) = input {
        value["tool_input"] = input.as_value().clone();
    }
    if let Some(result) = result {
        value["tool_result"] = result;
    }
    if let Some(error) = error {
        value["error_message"] = Value::String(error.into());
    }
    HookPayload::new(event, value).map_err(|_| hook_boundary_error("payload"))
}

fn hook_boundary_error(code: &'static str) -> PipelineError {
    PipelineError::Hook(HookFailure {
        owner: lotta_runtime::hooks::HookOwner::new("runtime".into()).expect("constant owner"),
        hook_id: lotta_runtime::hooks::HookId::new("tool-pipeline".into())
            .expect("constant hook id"),
        code,
    })
}

fn raw_json(raw: &RawToolOutcome) -> serde_json::Value {
    match raw {
        RawToolOutcome::Success(output) => serde_json::json!({"status":"success","output":output}),
        RawToolOutcome::Failure(_) => serde_json::json!({"status":"error"}),
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

/// Validates bounded edited input against the exact registered tool schema.
///
/// # Errors
/// Returns the same typed pipeline validation failures used by normal tool admission.
pub fn validate_input(
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

fn sandbox_denied(
    tool: &crate::registry::RegisteredTool,
) -> Result<(RawToolOutcome, SecretDelivery), PipelineError> {
    let raw = RawToolOutcome::Failure(ToolOutcome::SandboxDenied {
        message: ToolOutcomeMessage::new("workspace sandbox denied".into())
            .map_err(|_| PipelineError::Sandbox)?,
    });
    let delivery = SecretDelivery {
        kind: if tool.definition.execution_owner == ToolExecutionOwner::Rust {
            SecretDeliveryKind::ChildEnvironment
        } else {
            SecretDeliveryKind::ProviderRequest
        },
        values: Arc::new(Vec::new()),
    };
    Ok((raw, delivery))
}

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
        ToolOutcome::Interruption { message: value } => ToolOutcome::Interruption {
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
