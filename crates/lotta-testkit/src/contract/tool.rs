/// Observable tool behaviors that an adapter factory must arrange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolContractScenario {
    /// Successful execution.
    Success,
    /// User denial.
    UserDenied,
    /// Tool interruption.
    Interrupted,
    /// Tool timeout.
    Timeout,
    /// Input validation failure.
    ValidationFailure,
    /// Sandbox denial.
    SandboxDenied,
    /// Child spawn failure.
    SpawnFailure,
    /// A tool-defined terminal error.
    ToolDefinedError,
    /// An adapter-level terminal error.
    AdapterError,
    /// Records one ASCII request while returning an adapter error.
    ObserveAscii,
    /// Records one Unicode request while returning an adapter error.
    ObserveUnicode,
    /// Execution becomes pending and is then cancelled without consuming its scripted response.
    Cancelled,
    /// Two calls enter concurrently and must be admitted in deterministic sequential order.
    Sequential,
    /// The adapter's scenario setup and public boundary validation are observed.
    SetupBounds,
}

use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{
    ParallelSafety, ToolApprovalPolicy, ToolExecutionOwner, ToolExecutionRequest, ToolOutcome,
    ToolPort,
};
use serde_json::json;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// One adapter instance and instrumentation-neutral observation handle for a scenario.
pub struct ToolContractCase<Adapter> {
    /// Adapter under contract.
    pub port: Adapter,
    /// Testkit-owned observation handle fed by the adapter's test wrapper.
    pub probe: ToolContractProbe,
}

/// Shared, cloneable observation sink for tool contract wrappers.
#[derive(Clone, Debug, Default)]
pub struct ToolContractProbe {
    state: Arc<Mutex<ToolContractSnapshot>>,
}

/// Redaction-safe tool observation shared between contract and adapter wrappers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolContractObservation {
    /// Stable internal tool name.
    pub internal_name: String,
    /// Resolved model-facing name.
    pub model_name: String,
    /// Exact executor owner.
    pub execution_owner: ToolExecutionOwner,
    /// Conservative parallel-safety classification.
    pub parallel_safety: ParallelSafety,
    /// Definition approval policy.
    pub approval_policy: ToolApprovalPolicy,
    /// Permission action label.
    pub permission_action: String,
    /// Definition execution timeout.
    pub timeout: Duration,
    /// Definition output byte ceiling.
    pub output_bytes_max: usize,
    /// Definition output model-character ceiling.
    pub output_model_chars_max: usize,
    /// Secret-field count, without retaining paths or values.
    pub secret_field_count: usize,
    /// Deadline requested by the caller.
    pub deadline: Duration,
    /// Whether cancellation was terminal when observed.
    pub cancelled: bool,
    /// Exact compact JSON input byte count.
    pub input_bytes: u32,
}

/// Snapshot exposed by a tool contract probe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolContractSnapshot {
    /// Redaction-safe request observations.
    pub observations: Vec<ToolContractObservation>,
    /// Greatest number of simultaneously admitted calls.
    pub max_in_flight: u32,
    /// Number of terminal calls.
    pub completed_calls: u32,
    /// Number of calls currently waiting at the scenario gate.
    pub pending_calls: u32,
    /// Whether scenario setup and public boundary validation succeeded.
    pub setup_bounds_verified: bool,
}

impl ToolContractProbe {
    /// Returns an atomic observation snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ToolContractSnapshot {
        self.lock().clone()
    }

    /// Replaces the snapshot atomically; intended for adapter test wrappers.
    pub fn publish(&self, snapshot: ToolContractSnapshot) {
        *self.lock() = snapshot;
    }

    fn lock(&self) -> MutexGuard<'_, ToolContractSnapshot> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Drives all normalized tool, boundary, cancellation, and sequential scenarios.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn tool_contract<F, Fut, Adapter>(factory: F, requests: Vec<ToolExecutionRequest>)
where
    F: Fn(ToolContractScenario) -> Fut,
    Fut: Future<Output = ToolContractCase<Adapter>>,
    Adapter: ToolPort + 'static,
{
    assert_eq!(requests.len(), 10);
    assert_tool_requests(&requests);
    let scenarios = [
        ToolContractScenario::Success,
        ToolContractScenario::UserDenied,
        ToolContractScenario::Interrupted,
        ToolContractScenario::Timeout,
        ToolContractScenario::ValidationFailure,
        ToolContractScenario::SandboxDenied,
        ToolContractScenario::SpawnFailure,
        ToolContractScenario::ToolDefinedError,
    ];
    for (scenario, request) in scenarios.into_iter().zip(requests.iter().cloned()) {
        let case = factory(scenario).await;
        assert_outcome(scenario, case.port.execute(request).await);
    }
    let adapter_error = factory(ToolContractScenario::AdapterError).await;
    assert!(matches!(
        adapter_error.port.execute(requests[8].clone()).await,
        Err(RuntimeError::AdapterFailure {
            code: "configured",
            ..
        })
    ));
    let mut cancelled = factory(ToolContractScenario::Cancelled).await;
    assert_pending_cancellation(&mut cancelled, requests[9].clone()).await;
    assert_sequential(
        factory(ToolContractScenario::Sequential).await,
        requests[0].clone(),
        requests[1].clone(),
    )
    .await;
    assert_tool_observation(
        factory(ToolContractScenario::ObserveAscii).await,
        &requests[0],
    )
    .await;
    assert_unicode_input_observation(
        factory(ToolContractScenario::ObserveUnicode).await,
        &requests[0],
    )
    .await;
    assert!(
        factory(ToolContractScenario::SetupBounds)
            .await
            .probe
            .snapshot()
            .setup_bounds_verified
    );
}

fn assert_tool_requests(requests: &[ToolExecutionRequest]) {
    assert!(requests.iter().all(|request| matches!(
        request.definition.parallel_safety,
        ParallelSafety::Sequential
    )));
    let definition = &requests[0].definition;
    assert_eq!(definition.internal_name.as_str(), "contract");
    assert_eq!(definition.model_name.as_str(), "Contract");
    assert_eq!(definition.execution_owner, ToolExecutionOwner::Rust);
    assert_eq!(definition.approval_policy, ToolApprovalPolicy::Never);
    assert_eq!(definition.permission_action.as_str(), "contract");
    assert_eq!(definition.timeout.get(), Duration::from_secs(1));
    assert_eq!(definition.output_limit.bytes_max(), 1_024);
    assert_eq!(definition.output_limit.model_chars_max(), 1_024);
    assert_eq!(requests[0].deadline.get(), Duration::from_millis(1));
    let debug = format!("{:?}", requests[0]);
    assert!(debug.contains("REDACTED"));
    assert!(!debug.contains("recognizable-secret"));
    assert!(!requests[0].cancellation.is_cancelled());
}

fn assert_outcome(scenario: ToolContractScenario, outcome: Result<ToolOutcome, RuntimeError>) {
    let wire = serde_json::to_value(outcome.as_ref().expect("tool outcome")).expect("wire");
    let expected = match scenario {
        ToolContractScenario::Success => "success",
        ToolContractScenario::UserDenied => "user_denied",
        ToolContractScenario::Interrupted => "interrupted",
        ToolContractScenario::Timeout => "timeout",
        ToolContractScenario::ValidationFailure => "validation_failure",
        ToolContractScenario::SandboxDenied => "sandbox_denied",
        ToolContractScenario::SpawnFailure => "spawn_failure",
        ToolContractScenario::ToolDefinedError => "tool_defined_error",
        _ => panic!("non-outcome tool scenario"),
    };
    assert_eq!(wire["type"], expected);
    match outcome.expect("tool outcome") {
        ToolOutcome::Success { content } => assert_eq!(content.as_str(), "success"),
        ToolOutcome::ToolDefinedError { code, message } => {
            assert_eq!(code.as_str(), "code");
            assert_eq!(message.as_str(), "message");
        }
        ToolOutcome::UserDenied { message }
        | ToolOutcome::Interrupted { message }
        | ToolOutcome::Timeout { message }
        | ToolOutcome::ValidationFailure { message }
        | ToolOutcome::SandboxDenied { message }
        | ToolOutcome::SpawnFailure { message } => assert_eq!(message.as_str(), "message"),
    }
}

async fn assert_pending_cancellation<Adapter: ToolPort>(
    case: &mut ToolContractCase<Adapter>,
    mut request: ToolExecutionRequest,
) {
    let cancellation = CancellationToken::new();
    request.cancellation = cancellation.clone();
    assert_eq!(case.probe.snapshot().pending_calls, 1);
    cancellation.cancel();
    assert!(matches!(
        case.port.execute(request.clone()).await,
        Ok(ToolOutcome::Interrupted { .. })
    ));
    assert_eq!(case.probe.snapshot().completed_calls, 1);
    request.cancellation = CancellationToken::new();
    assert!(matches!(
        case.port.execute(request).await,
        Ok(ToolOutcome::Success { .. })
    ));
    assert_eq!(case.probe.snapshot().completed_calls, 2);
}

async fn assert_tool_observation<Adapter: ToolPort>(
    case: ToolContractCase<Adapter>,
    request: &ToolExecutionRequest,
) {
    let result = case.port.execute(request.clone()).await;
    assert!(matches!(result, Err(RuntimeError::AdapterFailure { .. })));
    let snapshot = case.probe.snapshot();
    let observation = &snapshot.observations[0];
    assert_eq!(observation.internal_name, "contract");
    assert_eq!(observation.model_name, "Contract");
    assert_eq!(observation.execution_owner, ToolExecutionOwner::Rust);
    assert_eq!(observation.parallel_safety, ParallelSafety::Sequential);
    assert_eq!(observation.approval_policy, ToolApprovalPolicy::Never);
    assert_eq!(observation.permission_action, "contract");
    assert_eq!(observation.timeout, Duration::from_secs(1));
    assert_eq!(observation.output_bytes_max, 1_024);
    assert_eq!(observation.output_model_chars_max, 1_024);
    assert_eq!(observation.secret_field_count, 1);
    assert_eq!(observation.deadline, Duration::from_millis(1));
    assert!(!observation.cancelled);
    let expected = serde_json::to_string(request.input.as_value())
        .expect("input writer")
        .len();
    assert_eq!(observation.input_bytes as usize, expected);
    let debug = format!("{observation:?}");
    assert!(!debug.contains("recognizable-secret"));
    assert!(!debug.contains("description"));
    assert!(!debug.contains("type\":\"object"));
}

async fn assert_unicode_input_observation<Adapter: ToolPort>(
    case: ToolContractCase<Adapter>,
    request: &ToolExecutionRequest,
) {
    let mut request = request.clone();
    request.input = lotta_runtime::ports::ValidatedToolInput::new(
        lotta_domain::BoundedJsonValue::new(json!({
            "ascii":"line\nquote\"",
            "unicode":"é😀"
        }))
        .expect("unicode input"),
    )
    .expect("validated unicode input");
    let _ = case.port.execute(request.clone()).await;
    let observation = case.probe.snapshot().observations.remove(0);
    let expected = serde_json::to_string(request.input.as_value())
        .expect("unicode writer")
        .len();
    assert_eq!(observation.input_bytes as usize, expected);
    assert!(!format!("{observation:?}").contains("é😀"));
}

async fn assert_sequential<P>(
    case: ToolContractCase<P>,
    first: ToolExecutionRequest,
    second: ToolExecutionRequest,
) where
    P: ToolPort + 'static,
{
    let port = Arc::new(case.port);
    let first_port = Arc::clone(&port);
    let second_port = Arc::clone(&port);
    let first_call = tokio::spawn(async move { first_port.execute(first).await });
    let second_call = tokio::spawn(async move { second_port.execute(second).await });
    let (first_result, second_result) = tokio::join!(first_call, second_call);
    let first = first_result
        .expect("first sequential task")
        .expect("first sequential result");
    let second = second_result
        .expect("second sequential task")
        .expect("second sequential result");
    assert!(matches!(first, ToolOutcome::Success { ref content } if content.as_str() == "first"));
    assert!(matches!(second, ToolOutcome::Success { ref content } if content.as_str() == "second"));
    let snapshot = case.probe.snapshot();
    assert_eq!(snapshot.max_in_flight, 1);
    assert_eq!(snapshot.completed_calls, 2);
}
