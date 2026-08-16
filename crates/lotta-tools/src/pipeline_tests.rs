use super::*;
use crate::{
    ToolsetId,
    registry::{ToolRegistration, ToolRegistry},
};
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::*,
};
use std::{
    fmt::{self, Write},
    fs, future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const ALPHA: &str = "secret-alpha-sentinel";
const BETA: &str = "secret-beta-sentinel";
const COMMAND: &str = "$API_TOKEN$SECOND,$API_TOKEN";
const PATH_NOTICE: &str = "/fake/full-output.txt";

#[derive(Default)]
struct State {
    trace: Vec<TraceEvent>,
    effects: Vec<&'static str>,
    posts: Vec<PostHookStatus>,
    pre_calls: usize,
    executed: usize,
    persisted: Vec<ToolOutcome>,
    emitted: Vec<ToolOutcome>,
    executor_input: Option<Value>,
    delivery: Option<ObservedDelivery>,
    resolved: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct ObservedDelivery {
    kind: SecretDeliveryKind,
    api_token: Option<String>,
    second: Option<String>,
    key_only: Option<String>,
    debug: String,
}

struct Trace(Arc<Mutex<State>>);
impl TraceSink for Trace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().trace.push(event);
    }
}
struct Sink(Arc<Mutex<State>>, &'static str);
impl OutcomeSink for Sink {
    fn record(&self, _: &str, outcome: &ToolOutcome) -> Result<(), PipelineError> {
        let mut state = self.0.lock().unwrap();
        state.effects.push(self.1);
        if self.1 == "persist" {
            state.persisted.push(outcome.clone());
        } else {
            state.emitted.push(outcome.clone());
        }
        Ok(())
    }
}
struct Secrets(Arc<Mutex<State>>);
impl SecretResolver for Secrets {
    fn resolve(&self, name: &str) -> Result<Option<String>, PipelineError> {
        self.0.lock().unwrap().resolved.push(name.to_owned());
        Ok(match name {
            "API_TOKEN" => Some(ALPHA.into()),
            "SECOND" => Some(BETA.into()),
            _ => None,
        })
    }
}
struct FixedPermissions(PermissionDecision);
impl crate::permissions::PermissionGate for FixedPermissions {
    fn check(
        &self,
        _: crate::permissions::PermissionInvocation<'_>,
    ) -> Result<PermissionDecision, crate::permissions::matcher::PermissionError> {
        Ok(self.0)
    }
}

struct Overflow(Arc<Mutex<Option<String>>>);
impl crate::clamp::OverflowWriter for Overflow {
    fn write(&self, _: &str, content: &str) -> Result<String, crate::clamp::ClampError> {
        *self.0.lock().unwrap() = Some(content.into());
        Ok(PATH_NOTICE.into())
    }
}

enum Action {
    Success(String),
    Failure(ToolOutcome),
    Error,
}
struct Executor(Arc<Mutex<State>>, Action);
impl ToolExecutor for Executor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let mut state = self.0.lock().unwrap();
        state.executed += 1;
        assert_eq!(request.model_name.as_str(), "Read");
        assert_eq!(request.definition.internal_name.as_str(), "Read");
        let delivery = request.secret_delivery();
        state.executor_input = Some(request.input.as_value().clone());
        state.delivery = Some(ObservedDelivery {
            kind: delivery.kind(),
            api_token: delivery.get("API_TOKEN").map(str::to_owned),
            second: delivery.get("SECOND").map(str::to_owned),
            key_only: delivery.get("KEY_ONLY").map(str::to_owned),
            debug: format!("{delivery:?}"),
        });
        let result = match &self.1 {
            Action::Success(value) => Ok(RawToolOutcome::Success(value.clone())),
            Action::Failure(value) => Ok(RawToolOutcome::Failure(value.clone())),
            Action::Error => Err(ExecutorError),
        };
        Box::pin(future::ready(result))
    }
}
struct Hooks(
    Arc<Mutex<State>>,
    Option<Result<PreHookResult, OwnerFailure>>,
    Option<OwnerFailure>,
);
impl PipelineHooks for Hooks {
    fn pre(&self, _: &ValidatedToolInput) -> Result<PreHookResult, OwnerFailure> {
        self.0.lock().unwrap().pre_calls += 1;
        match &self.1 {
            None | Some(Ok(PreHookResult::Allow)) => Ok(PreHookResult::Allow),
            Some(Ok(PreHookResult::Replace(value))) => Ok(PreHookResult::Replace(value.clone())),
            Some(Err(error)) => Err(error.clone()),
        }
    }
    fn post(&self, status: PostHookStatus) -> Result<(), OwnerFailure> {
        self.0.lock().unwrap().posts.push(status);
        self.2.clone().map_or(Ok(()), Err)
    }
}

fn definition(owner: ToolExecutionOwner) -> Arc<ToolDefinition> {
    let schema = serde_json::from_str::<ToolInputSchema>(
        r#"{"type":"object","required":["command"],"properties":{"command":{"type":"string"}}}"#,
    )
    .unwrap();
    let secrets =
        serde_json::from_str::<SecretRedactionSpec>(r#"{"fields":[],"policy":"redact"}"#).unwrap();
    Arc::new(ToolDefinition::new(
        InternalToolName::new("Read".into()).unwrap(),
        ModelFacingToolName::new("Read".into()).unwrap(),
        schema,
        ToolDescriptionAsset::new(String::new()).unwrap(),
        owner,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).unwrap(),
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .unwrap(),
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .unwrap(),
        secrets,
    ))
}
fn registry(state: Arc<Mutex<State>>, action: Action) -> ToolRegistry {
    let registry = ToolRegistry::new([]).unwrap();
    let registration = ToolRegistration {
        definition: definition(ToolExecutionOwner::Rust),
        executor: Arc::new(Executor(state, action)),
    };
    registry
        .update(ToolsetId::None, &[registration], None)
        .unwrap();
    registry
}
fn input(value: Value) -> BoundedJsonValue {
    BoundedJsonValue::new(value).unwrap()
}

async fn run_with_snapshot(
    state: Arc<Mutex<State>>,
    hooks: &Hooks,
    snapshot: Arc<RegistrySnapshot>,
    value: Value,
    overflow: &dyn crate::clamp::OverflowWriter,
) -> Result<ToolOutcome, PipelineError> {
    let trace = Trace(Arc::clone(&state));
    let persistence = Sink(Arc::clone(&state), "persist");
    let emit = Sink(Arc::clone(&state), "emit");
    let secrets = Secrets(Arc::clone(&state));
    execute(PipelineRequest {
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: snapshot,
        model_name: "Read",
        input: input(value),
        cancellation: CancellationToken::new(),
        hooks,
        permissions: &crate::permissions::AllowAllPermissions,
        sandbox: &crate::sandbox::AllowAllSandbox,
        secrets: &secrets,
        trace: &trace,
        overflow,
        persistence: &persistence,
        emit: &emit,
    })
    .await
}

async fn run(
    state: Arc<Mutex<State>>,
    hooks: &Hooks,
    action: Action,
    value: Value,
    overflow: &dyn crate::clamp::OverflowWriter,
) -> Result<ToolOutcome, PipelineError> {
    let registry = registry(Arc::clone(&state), action);
    run_with_snapshot(state, hooks, registry.snapshot().unwrap(), value, overflow).await
}

fn assert_no_sentinel(value: &impl fmt::Debug) {
    let debug = format!("{value:?}");
    assert!(!debug.contains(ALPHA) && !debug.contains(BETA), "{debug}");
}

pub(super) async fn stage_order_case() {
    let state = Arc::new(Mutex::new(State::default()));
    let hooks = Hooks(
        Arc::clone(&state),
        Some(Ok(PreHookResult::Replace(input(
            serde_json::json!({"command":"replacement-valid"}),
        )))),
        None,
    );
    let overflow = Overflow(Arc::new(Mutex::new(None)));
    run(
        Arc::clone(&state),
        &hooks,
        Action::Success("ok".into()),
        serde_json::json!({"command":"read"}),
        &overflow,
    )
    .await
    .unwrap();
    let state = state.lock().unwrap();
    let expected = [
        PipelineStage::PreHook,
        PipelineStage::Permission,
        PipelineStage::Sandbox,
        PipelineStage::SecretSubstitution,
        PipelineStage::Executor,
        PipelineStage::PostHook,
        PipelineStage::Scrub,
        PipelineStage::Clamp,
        PipelineStage::Persist,
        PipelineStage::Emit,
    ];
    assert_eq!(
        &state.trace[..2],
        &[
            TraceEvent::Preflight(PreflightEvent::NameResolution),
            TraceEvent::Preflight(PreflightEvent::SchemaValidation)
        ]
    );
    assert_eq!(&state.trace[2..], expected.map(TraceEvent::Stage));
    assert_eq!(state.effects, ["persist", "emit"]);
    assert_eq!(state.pre_calls, 1);
    assert_eq!(state.executed, 1);
    assert_eq!(
        state.executor_input,
        Some(serde_json::json!({"command":"replacement-valid"}))
    );
    assert_eq!(state.posts, [PostHookStatus::Completed]);
}

static PIPELINE_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn pipeline_overflow_root() -> PathBuf {
    let sequence = PIPELINE_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let base =
        std::env::temp_dir().join(format!("lotta-pipeline-{}-{sequence}", std::process::id()));
    let _ignored = fs::remove_dir_all(&base);
    fs::create_dir(&base).unwrap();
    base.canonicalize().unwrap()
}

fn footer_path(content: &str) -> &Path {
    let prefix = "[Full output written to: ";
    let line = content.lines().last().unwrap();
    assert!(line.starts_with(prefix) && line.ends_with(']'));
    Path::new(&line[prefix.len()..line.len() - 1])
}

pub(super) async fn secret_substitution_case() {
    let state = Arc::new(Mutex::new(State::default()));
    let hooks = Hooks(Arc::clone(&state), None, None);
    let root = pipeline_overflow_root();
    let overflow = crate::clamp::FileOverflowWriter::new(root.clone()).unwrap();
    let raw = format!("API_TOKEN={ALPHA} SECOND={BETA} {}", "é".repeat(31_000));
    let result = run(
        Arc::clone(&state),
        &hooks,
        Action::Success(raw.clone()),
        serde_json::json!({"command":COMMAND,"$KEY_ONLY":"ignored"}),
        &overflow,
    )
    .await
    .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(
        state
            .executor_input
            .as_ref()
            .unwrap()
            .get("command")
            .unwrap(),
        COMMAND
    );
    assert_eq!(
        state.delivery.as_ref().unwrap(),
        &ObservedDelivery {
            kind: SecretDeliveryKind::ChildEnvironment,
            api_token: Some(ALPHA.into()),
            second: Some(BETA.into()),
            key_only: None,
            debug: "SecretDelivery([REDACTED])".into(),
        }
    );
    assert_eq!(state.resolved, ["API_TOKEN", "SECOND"]);
    assert_eq!(state.posts, [PostHookStatus::Completed]);
    assert_eq!(state.persisted, state.emitted);
    assert_eq!(state.persisted.as_slice(), std::slice::from_ref(&result));
    assert_no_sentinel(&state.trace);
    assert_no_sentinel(&result);
    assert_no_sentinel(&state.persisted);
    assert_no_sentinel(&state.emitted);
    let scrubbed_raw = raw
        .replace(ALPHA, "API_TOKEN=<REDACTED>")
        .replace(BETA, "SECOND=<REDACTED>");
    let ToolOutcome::Success { content } = &result else {
        panic!("expected success")
    };
    let content = content.as_str();
    let path = footer_path(content);
    assert!(path.is_absolute() && path.starts_with(&root) && path.is_file());
    assert_eq!(fs::read_to_string(path).unwrap(), scrubbed_raw);
    assert!(content.chars().count() <= 32_000);
    assert!(
        content.len()
            <= definition(ToolExecutionOwner::Rust)
                .output_limit
                .bytes_max()
    );
    assert!(content.contains("[Output truncated: showing 30,000 of "));
    let retained = scrubbed_raw.chars().take(15_000).collect::<String>()
        + &scrubbed_raw
            .chars()
            .rev()
            .take(15_000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
    let middle = content.split("\n... [").next().unwrap().to_owned()
        + content
            .rsplit("] ...\n")
            .next()
            .unwrap()
            .split("[Output truncated:")
            .next()
            .unwrap();
    assert_eq!(middle, retained);
    assert_eq!(
        content.lines().last().unwrap(),
        format!("[Full output written to: {}]", path.display())
    );
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

struct GeneratedResolver {
    value_len: usize,
    extra_one: bool,
    requested: Mutex<Vec<String>>,
}
impl SecretResolver for GeneratedResolver {
    fn resolve(&self, name: &str) -> Result<Option<String>, PipelineError> {
        self.requested.lock().unwrap().push(name.to_owned());
        if name == "UNKNOWN" {
            Ok(None)
        } else if self.extra_one && name == "N16" {
            Ok(Some("x".into()))
        } else {
            Ok(Some("x".repeat(self.value_len)))
        }
    }
}

fn placeholders(count: usize) -> Value {
    let text = (0..count).fold(String::new(), |mut text, index| {
        write!(text, "$N{index} ").unwrap();
        text
    });
    Value::String(text)
}

#[test]
fn exact_secret_name_boundaries() {
    let at_max = placeholders(SECRET_NAMES_MAX);
    assert_eq!(placeholder_names(&at_max).unwrap().len(), SECRET_NAMES_MAX);
    assert_eq!(
        placeholder_names(&placeholders(SECRET_NAMES_MAX + 1)),
        Err(PipelineError::SecretDelivery)
    );
    let duplicate = Value::String(format!("{}$N0", at_max.as_str().unwrap()));
    assert_eq!(
        placeholder_names(&duplicate).unwrap().len(),
        SECRET_NAMES_MAX
    );
    let max_name = format!("A{}", "A".repeat(SECRET_NAME_BYTES_MAX - 1));
    assert_eq!(
        placeholder_names(&Value::String(format!("${max_name}")))
            .unwrap()
            .len(),
        1
    );
    let over_name = format!("A{}", "A".repeat(SECRET_NAME_BYTES_MAX));
    assert_eq!(
        placeholder_names(&Value::String(format!("${over_name}"))),
        Err(PipelineError::SecretDelivery)
    );
}

#[test]
fn exact_secret_delivery_boundaries() {
    let resolver = GeneratedResolver {
        value_len: SECRET_VALUE_BYTES_MAX,
        extra_one: false,
        requested: Mutex::new(Vec::new()),
    };
    let one = resolve_secrets(
        &Value::String("$N0".into()),
        ToolExecutionOwner::Rust,
        &resolver,
    )
    .unwrap();
    assert_eq!(one.get("N0").unwrap().len(), SECRET_VALUE_BYTES_MAX);
    let too_large = GeneratedResolver {
        value_len: SECRET_VALUE_BYTES_MAX + 1,
        extra_one: false,
        requested: Mutex::new(Vec::new()),
    };
    assert!(matches!(
        resolve_secrets(
            &Value::String("$N0".into()),
            ToolExecutionOwner::Rust,
            &too_large
        ),
        Err(PipelineError::SecretDelivery)
    ));
    let delivery = resolve_secrets(&placeholders(16), ToolExecutionOwner::Rust, &resolver).unwrap();
    for index in 0..16 {
        assert_eq!(
            delivery.get(&format!("N{index}")).unwrap().len(),
            SECRET_VALUE_BYTES_MAX
        );
    }
    let over_aggregate = GeneratedResolver {
        value_len: SECRET_VALUE_BYTES_MAX,
        extra_one: true,
        requested: Mutex::new(Vec::new()),
    };
    assert!(matches!(
        resolve_secrets(&placeholders(17), ToolExecutionOwner::Rust, &over_aggregate),
        Err(PipelineError::SecretDelivery)
    ));
    let unknown = GeneratedResolver {
        value_len: 1,
        extra_one: false,
        requested: Mutex::new(Vec::new()),
    };
    let delivery = resolve_secrets(
        &serde_json::json!({"$KEY_ONLY":"$N0$UNKNOWN!"}),
        ToolExecutionOwner::Rust,
        &unknown,
    )
    .unwrap();
    assert_eq!(delivery.get("N0"), Some("x"));
    assert_eq!(delivery.get("UNKNOWN"), None);
    assert_eq!(*unknown.requested.lock().unwrap(), ["N0", "UNKNOWN"]);
}

#[test]
fn provider_request_secret_delivery() {
    let state = Arc::new(Mutex::new(State::default()));
    let resolver = Secrets(Arc::clone(&state));
    let value = serde_json::json!({"command":"$API_TOKEN"});
    let delivery = resolve_secrets(&value, ToolExecutionOwner::Controller, &resolver).unwrap();
    assert_eq!(delivery.kind(), SecretDeliveryKind::ProviderRequest);
    assert_eq!(delivery.get("API_TOKEN"), Some(ALPHA));
    assert_eq!(state.lock().unwrap().resolved, ["API_TOKEN"]);
}

pub(super) async fn owner_attribution_case() {
    for owner in [
        ExtensionOwner::hook("hook-id".into()).unwrap(),
        ExtensionOwner::extension_mod("mod-id".into()).unwrap(),
    ] {
        assert_owner_failure(owner.clone(), false).await;
        assert_owner_failure(owner, true).await;
    }
}

async fn assert_owner_failure(owner: ExtensionOwner, post: bool) {
    let state = Arc::new(Mutex::new(State::default()));
    let failure = OwnerFailure {
        owner: owner.clone(),
        code: "blocked",
    };
    let hooks = if post {
        Hooks(Arc::clone(&state), None, Some(failure.clone()))
    } else {
        Hooks(Arc::clone(&state), Some(Err(failure.clone())), None)
    };
    let overflow = Overflow(Arc::new(Mutex::new(None)));
    let registry = registry(Arc::clone(&state), Action::Success("ok".into()));
    let before = registry.snapshot().unwrap();
    let error = run_with_snapshot(
        Arc::clone(&state),
        &hooks,
        Arc::clone(&before),
        serde_json::json!({"command":"read"}),
        &overflow,
    )
    .await
    .unwrap_err();
    let after = registry.snapshot().unwrap();
    assert_eq!(error, PipelineError::Owner(failure.clone()));
    assert_eq!(failure.owner.kind(), owner.kind());
    assert_eq!(failure.owner.id(), owner.id());
    assert_eq!(failure.code, "blocked");
    assert!(Arc::ptr_eq(&before, &after));
    assert_eq!(
        before.by_model("Read").unwrap().model_name,
        after.by_model("Read").unwrap().model_name
    );
    let state = state.lock().unwrap();
    let expected_last = if post {
        PipelineStage::PostHook
    } else {
        PipelineStage::PreHook
    };
    assert_eq!(state.trace.last(), Some(&TraceEvent::Stage(expected_last)));
    assert_eq!(state.pre_calls, 1);
    assert_eq!(state.executed, usize::from(post));
    assert!(state.effects.is_empty());
    assert_eq!(state.posts.len(), usize::from(post));
}

#[test]
fn owner_identity_rejects_bounded_invalid_values_without_disclosure() {
    let overbound = "owner-payload-sentinel".repeat(20);
    for value in [
        overbound.clone(),
        String::new(),
        "nul-payload-sentinel\0".into(),
    ] {
        let error = OwnerId::new(value.clone()).unwrap_err();
        assert_eq!(error, PipelineError::OwnerIdentity);
        let debug = format!("{error:?}");
        assert!(value.is_empty() || !debug.contains(&value));
        assert!(!debug.contains("sentinel") && !debug.contains("payload"));
    }
}

fn assert_executor_failure(state: &State, error: &PipelineError) {
    assert_eq!(state.executed, 1);
    assert_eq!(state.posts, [PostHookStatus::Failed]);
    assert!(state.effects.is_empty());
    assert_eq!(
        state.trace.last(),
        Some(&TraceEvent::Stage(PipelineStage::PostHook))
    );
    assert!(
        !state
            .trace
            .contains(&TraceEvent::Stage(PipelineStage::Scrub))
    );
    assert!(matches!(
        error,
        PipelineError::Executor | PipelineError::ResultLimit
    ));
}

#[tokio::test]
async fn invalid_inputs_and_executor_failures_stop_effects() {
    for value in [serde_json::json!({}), serde_json::json!({"command": 1})] {
        let state = Arc::new(Mutex::new(State::default()));
        let hooks = Hooks(Arc::clone(&state), None, None);
        let overflow = Overflow(Arc::new(Mutex::new(None)));
        let error = run(
            Arc::clone(&state),
            &hooks,
            Action::Success("x".into()),
            value,
            &overflow,
        )
        .await
        .unwrap_err();
        assert_eq!(error, PipelineError::SchemaValidation);
        let state = state.lock().unwrap();
        assert_eq!(
            state.trace,
            [
                TraceEvent::Preflight(PreflightEvent::NameResolution),
                TraceEvent::Preflight(PreflightEvent::SchemaValidation),
            ]
        );
        assert_eq!(state.pre_calls, 0);
        assert_eq!(state.executed, 0);
        assert!(state.posts.is_empty() && state.effects.is_empty());
    }

    let state = Arc::new(Mutex::new(State::default()));
    let hooks = Hooks(
        Arc::clone(&state),
        Some(Ok(PreHookResult::Replace(input(
            serde_json::json!({"command":1}),
        )))),
        None,
    );
    let overflow = Overflow(Arc::new(Mutex::new(None)));
    let error = run(
        Arc::clone(&state),
        &hooks,
        Action::Success("x".into()),
        serde_json::json!({"command":"valid"}),
        &overflow,
    )
    .await
    .unwrap_err();
    assert_eq!(error, PipelineError::SchemaValidation);
    {
        let state_guard = state.lock().unwrap();
        assert_eq!(
            state_guard.trace,
            [
                TraceEvent::Preflight(PreflightEvent::NameResolution),
                TraceEvent::Preflight(PreflightEvent::SchemaValidation),
                TraceEvent::Stage(PipelineStage::PreHook),
            ]
        );
        assert!(
            !state_guard
                .trace
                .contains(&TraceEvent::Stage(PipelineStage::Permission))
        );
        assert!(state_guard.effects.is_empty() && state_guard.posts.is_empty());
        assert_eq!(state_guard.pre_calls, 1);
        assert_eq!(state_guard.executed, 0);
    }

    for (action, expected) in [
        (Action::Error, PipelineError::Executor),
        (
            Action::Success("x".repeat(TOOL_RESULT_BYTES_MAX.value + 1)),
            PipelineError::ResultLimit,
        ),
    ] {
        let state = Arc::new(Mutex::new(State::default()));
        let hooks = Hooks(Arc::clone(&state), None, None);
        let error = run(
            Arc::clone(&state),
            &hooks,
            action,
            serde_json::json!({"command":"x"}),
            &overflow,
        )
        .await
        .unwrap_err();
        assert_eq!(error, expected);
        assert_executor_failure(&state.lock().unwrap(), &error);
    }
}

#[tokio::test]
async fn sandbox_stage_follows_permission_for_effect_recheck_ownership() {
    let state = Arc::new(Mutex::new(State::default()));
    let hooks = Hooks(Arc::clone(&state), None, None);
    let overflow = Overflow(Arc::new(Mutex::new(None)));
    let trace = Trace(Arc::clone(&state));
    let persistence = Sink(Arc::clone(&state), "persist");
    let emit = Sink(Arc::clone(&state), "emit");
    let secrets = Secrets(Arc::clone(&state));
    let registry = registry(Arc::clone(&state), Action::Success("x".into()));
    execute(PipelineRequest {
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: registry.snapshot().unwrap(),
        model_name: "Read",
        input: input(serde_json::json!({"command":"valid"})),
        cancellation: CancellationToken::new(),
        hooks: &hooks,
        permissions: &FixedPermissions(PermissionDecision::Allow),
        sandbox: &crate::sandbox::AllowAllSandbox,
        secrets: &secrets,
        trace: &trace,
        overflow: &overflow,
        persistence: &persistence,
        emit: &emit,
    })
    .await
    .unwrap();
    let trace = &state.lock().unwrap().trace;
    let permission = trace
        .iter()
        .position(|event| event == &TraceEvent::Stage(PipelineStage::Permission))
        .unwrap();
    let sandbox = trace
        .iter()
        .position(|event| event == &TraceEvent::Stage(PipelineStage::Sandbox))
        .unwrap();
    assert_eq!(sandbox, permission + 1);
}

#[test]
fn task56_owns_approval_request_consumption_boundary() {
    let source = include_str!("builtin/interaction/mod.rs");
    let pipeline = include_str!("pipeline.rs");
    let bundle = include_str!("builtin/task40.rs");
    assert!(bundle.contains("Task 56 owns runtime permission-request consumption"));
    assert!(source.contains("pub async fn request_approval"));
    assert!(!pipeline.contains("InteractionPort"));
    assert!(!pipeline.contains("request_approval"));
}

#[tokio::test]
async fn permission_deny_and_ask_stop_later_effects() {
    for (decision, expected) in [
        (PermissionDecision::Deny, PipelineError::PermissionDenied),
        (PermissionDecision::Ask, PipelineError::ApprovalRequired),
    ] {
        let state = Arc::new(Mutex::new(State::default()));
        let hooks = Hooks(Arc::clone(&state), None, None);
        let overflow = Overflow(Arc::new(Mutex::new(None)));
        let trace = Trace(Arc::clone(&state));
        let persistence = Sink(Arc::clone(&state), "persist");
        let emit = Sink(Arc::clone(&state), "emit");
        let secrets = Secrets(Arc::clone(&state));
        let registry = registry(Arc::clone(&state), Action::Success("x".into()));
        let gate = FixedPermissions(decision);
        let error = execute(PipelineRequest {
            tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
                lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
            ),
            registry: registry.snapshot().unwrap(),
            model_name: "Read",
            input: input(serde_json::json!({"command":"valid"})),
            cancellation: CancellationToken::new(),
            hooks: &hooks,
            permissions: &gate,
            sandbox: &crate::sandbox::AllowAllSandbox,
            secrets: &secrets,
            trace: &trace,
            overflow: &overflow,
            persistence: &persistence,
            emit: &emit,
        })
        .await
        .unwrap_err();
        assert_eq!(error, expected);
        let state = state.lock().unwrap();
        assert_eq!(
            state.trace.last(),
            Some(&TraceEvent::Stage(PipelineStage::Permission))
        );
        assert_eq!(state.executed, 0);
        assert!(state.effects.is_empty() && state.posts.is_empty() && state.resolved.is_empty());
    }
}

#[tokio::test]
async fn failure_code_and_message_are_scrubbed() {
    let message = || ToolOutcomeMessage::new(format!("failure {ALPHA}")).unwrap();
    let state = Arc::new(Mutex::new(State::default()));
    let hooks = Hooks(Arc::clone(&state), None, None);
    let overflow = Overflow(Arc::new(Mutex::new(None)));
    let failure = ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(format!("code-{ALPHA}-{BETA}")).unwrap(),
        message: ToolOutcomeMessage::new(format!("message-{ALPHA}-{BETA}")).unwrap(),
    };
    let result = run(
        Arc::clone(&state),
        &hooks,
        Action::Failure(failure),
        serde_json::json!({"command":"$API_TOKEN$SECOND"}),
        &overflow,
    )
    .await
    .unwrap();
    let pipeline_state = state.lock().unwrap();
    assert_eq!(pipeline_state.posts, [PostHookStatus::Failed]);
    assert_eq!(pipeline_state.persisted, pipeline_state.emitted);
    assert_eq!(
        pipeline_state.persisted.as_slice(),
        std::slice::from_ref(&result)
    );
    assert_no_sentinel(&result);
    assert_no_sentinel(&pipeline_state.persisted);
    assert_no_sentinel(&pipeline_state.emitted);
    let debug = format!("{result:?}");
    assert!(debug.contains("API_TOKEN=<REDACTED>") && debug.contains("SECOND=<REDACTED>"));
    assert!(overflow.0.lock().unwrap().is_none());
    drop(pipeline_state);
    let failures = [
        ToolOutcome::UserDenied { message: message() },
        ToolOutcome::Interrupted { message: message() },
        ToolOutcome::Timeout { message: message() },
        ToolOutcome::ValidationFailure { message: message() },
        ToolOutcome::SandboxDenied { message: message() },
        ToolOutcome::SpawnFailure { message: message() },
        ToolOutcome::ToolDefinedError {
            code: ToolOutcomeCode::new(format!("code-{ALPHA}")).unwrap(),
            message: message(),
        },
    ];
    for failure in failures {
        let scrubbed = scrub_failure(failure, &[("API_TOKEN", ALPHA)]).unwrap();
        let debug = format!("{scrubbed:?}");
        assert!(!debug.contains(ALPHA));
        assert!(debug.contains("API_TOKEN=<REDACTED>"));
    }
    let success = ToolOutcome::Success {
        content: ToolResultText::new(
            "ok".into(),
            definition(ToolExecutionOwner::Rust).output_limit,
        )
        .unwrap(),
    };
    assert_eq!(
        scrub_failure(success, &[("API_TOKEN", ALPHA)]),
        Err(PipelineError::Executor)
    );
}
