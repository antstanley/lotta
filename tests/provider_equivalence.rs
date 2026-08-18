use lotta_domain::{BoundedJsonValue, NonEmptyString};
use lotta_extensions::sidecar::SidecarOwnerIdentity;
use lotta_providers::context::{ContextGuard, ContextGuardError};
use lotta_providers::host::client::{HostConfig, materialize_host_script};
use lotta_providers::host::protocol::{HostAuth, HostOptions};
use lotta_providers::host::provider::HostProvider;
use lotta_providers::native::anthropic::Anthropic;
use lotta_providers::native::openai_compatible::{MaxTokensField, OpenAiCompatible};
use lotta_runtime::boundary::ProviderName;
use lotta_runtime::ports::{
    ImagePolicy, ProviderContent, ProviderContentPart, ProviderContext, ProviderContextDecision,
    ProviderContextTokenProvenance, ProviderDeadline, ProviderError, ProviderEvent,
    ProviderMessage, ProviderMessageRole, ProviderMessages, ProviderMetadata,
    ProviderMetadataInput, ProviderPort, ProviderRequest, provider_event_channel,
};
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::fixtures::providers::{
    Dialect, Dimension, ProviderCase, compare_provider_traces, load_case, load_index,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Notify;

const CANONICAL_DIMENSIONS: [Dimension; 10] = [
    Dimension::RequestMapping,
    Dimension::EventOrder,
    Dimension::ToolCallAssembly,
    Dimension::Usage,
    Dimension::Cancellation,
    Dimension::Errors,
    Dimension::Timeout,
    Dimension::ContextOverflow,
    Dimension::RetryAfter,
    Dimension::ImagePolicy,
];
const OVERLAP_IDS: [&str; 7] = [
    "openai-compatible/happy-tool",
    "anthropic/reasoning-redacted",
    "openai-compatible/retry-after",
    "anthropic/authentication",
    "openai-compatible/authorization",
    "anthropic/quota",
    "openai-compatible/protocol-error",
];
const UNSUPPORTED_IDS: [&str; 9] = [
    "ollama/cancelled",
    "lm-studio/timeout",
    "llama-cpp/context-overflow",
    "ollama/image-drop",
    "ollama/image-strict",
    "ollama/unavailable",
    "lm-studio/invalid-request",
    "lm-studio/unknown-error",
    "llama-cpp/overloaded",
];

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

#[derive(Debug)]
struct ExecutedCase {
    name: String,
    dimensions: BTreeSet<Dimension>,
    native_impl: &'static str,
    host_impl: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SemanticRequest {
    method: String,
    endpoint: String,
    auth_present: bool,
    body: Value,
}

#[tokio::test]
async fn indexed_overlap_real_adapters_are_equivalent() {
    assert_inventory();
    let cases = execute_overlap().await;
    let expected = OVERLAP_IDS.into_iter().collect::<BTreeSet<_>>();
    let actual = cases
        .iter()
        .map(|case| case.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert!(cases.iter().all(|case| case.native_impl != case.host_impl));
}

#[tokio::test]
async fn request_mapping_real_transports_are_semantically_equivalent() {
    for id in [
        "openai-compatible/happy-tool",
        "anthropic/reasoning-redacted",
    ] {
        let case = load(id);
        let (_, native, host) = execute_case(&case).await;
        let native = semantic_request(&native);
        let host = semantic_request(&host);
        assert_semantic_request_eq(&native, &host).unwrap();
        assert_request_mutations_fail(&native);
    }
}

#[tokio::test]
async fn trace_mutations_are_rejected() {
    let case = load("openai-compatible/happy-tool");
    let native = run_one_side(&case, Side::Native).await.0;
    for mutation in trace_mutations(&native) {
        assert!(compare_equivalent_traces(&native, &mutation).is_err());
    }
}

#[tokio::test]
async fn dimension_coverage() {
    let executed = execute_all_cases().await;
    let covered = executed
        .iter()
        .flat_map(|case| case.dimensions.iter().copied())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        covered,
        CANONICAL_DIMENSIONS.into_iter().collect::<BTreeSet<_>>()
    );
    let expected = OVERLAP_IDS
        .into_iter()
        .chain([
            "real_cancellation_both_adapters",
            "real_timeout_both_adapters",
            "real_context_overflow_both_adapters",
            "real_image_policy_both_adapters",
        ])
        .collect::<BTreeSet<_>>();
    assert_eq!(
        executed
            .iter()
            .map(|case| case.name.as_str())
            .collect::<BTreeSet<_>>(),
        expected
    );
}

#[test]
fn unsupported_inventory_is_exact() {
    assert_inventory();
    assert_eq!(UNSUPPORTED_IDS.len(), 9);
}

#[tokio::test]
async fn real_cancellation_both_adapters() {
    real_cancellation_case().await;
}

#[tokio::test]
async fn never_cancelled_adapter_token_cannot_meet_prompt_bound() {
    let fixture = load("openai-compatible/happy-tool");
    let loopback = BehavioralLoopback::stall().await;
    let adapter = OpenAiCompatible::new(loopback.base(), "fixture-credential").unwrap();
    let mut request = fixture.request.clone();
    request.cancellation = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(async move { run_interrupt_case(&adapter, request).await });
    loopback.wait_requested().await;
    let started = Instant::now();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .is_err(),
        "a never-cancelled adapter token must not satisfy the prompt bound"
    );
    assert!(started.elapsed() <= Duration::from_millis(550));
}

#[tokio::test]
async fn real_timeout_both_adapters() {
    real_timeout_case().await;
}

#[tokio::test]
async fn real_context_overflow_both_adapters() {
    real_context_overflow_case().await;
}

#[tokio::test]
async fn real_image_policy_both_adapters() {
    real_image_policy_case().await;
}

async fn execute_all_cases() -> Vec<ExecutedCase> {
    let mut cases = execute_overlap().await;
    cases.push(real_cancellation_case().await);
    cases.push(real_timeout_case().await);
    cases.push(real_context_overflow_case().await);
    cases.push(real_image_policy_case().await);
    cases
}

async fn real_cancellation_case() -> ExecutedCase {
    let fixture = load("openai-compatible/happy-tool");
    let native_loopback = BehavioralLoopback::stall().await;
    let host_loopback = BehavioralLoopback::stall().await;
    let native = OpenAiCompatible::new(native_loopback.base(), "fixture-credential").unwrap();
    let (host, host_request) = host_provider(&fixture, host_loopback.base()).await;
    let native_request = fixture.request.clone();
    let native_cancel = native_request.cancellation.clone();
    let host_cancel = host_request.cancellation.clone();
    let native_run = tokio::spawn(async move { run_interrupt_case(&native, native_request).await });
    let host_run = tokio::spawn(async move { run_interrupt_case(&host, host_request).await });
    native_loopback.wait_requested().await;
    host_loopback.wait_requested().await;
    let cancelled_at = Instant::now();
    native_cancel.cancel();
    host_cancel.cancel();
    let native_outcome = collect_interrupt_outcome(native_run).await;
    let host_outcome = collect_interrupt_outcome(host_run).await;
    assert_cancelled_outcome(&native_outcome);
    assert_cancelled_outcome(&host_outcome);
    compare_equivalent_traces(&native_outcome.1, &host_outcome.1).unwrap();
    assert!(
        cancelled_at.elapsed() <= Duration::from_millis(500),
        "cancellation exceeded 500ms: {:?}",
        cancelled_at.elapsed()
    );

    let followup = BehavioralLoopback::success().await;
    let (host, mut request) = host_provider(&fixture, followup.base()).await;
    request.cancellation = tokio_util::sync::CancellationToken::new();
    let trace = run_trace(&host, request).await;
    assert!(
        matches!(trace.last(), Some(ProviderEvent::Stop { .. })),
        "follow-up trace: {trace:?}"
    );
    followup.recorded().await;
    host.shutdown().expect("host shutdown and reap");

    behavioral_case("real_cancellation_both_adapters", Dimension::Cancellation)
}

async fn real_timeout_case() -> ExecutedCase {
    let fixture = load("openai-compatible/happy-tool");
    let native_loopback = BehavioralLoopback::stall().await;
    let host_loopback = BehavioralLoopback::stall().await;
    let native = OpenAiCompatible::new(native_loopback.base(), "fixture-credential").unwrap();
    let (host, mut host_request) = host_provider(&fixture, host_loopback.base()).await;
    let mut native_request = fixture.request.clone();
    let deadline = ProviderDeadline::new(Duration::from_millis(75)).unwrap();
    native_request.deadline = deadline;
    host_request.deadline = deadline;
    let started = Instant::now();
    let (native_result, host_result) = tokio::join!(
        run_interrupt_case(&native, native_request),
        run_interrupt_case(&host, host_request)
    );
    let elapsed = started.elapsed();
    assert_timeout_outcome(&native_result);
    assert_timeout_outcome(&host_result);
    compare_equivalent_traces(&native_result.1, &host_result.1).unwrap();
    assert!(
        elapsed < Duration::from_millis(500),
        "timeout took {elapsed:?}"
    );
    host.shutdown().expect("host shutdown");
    behavioral_case("real_timeout_both_adapters", Dimension::Timeout)
}

async fn real_context_overflow_case() -> ExecutedCase {
    let fixture = load("openai-compatible/happy-tool");
    let native_loopback = BehavioralLoopback::success().await;
    let host_loopback = BehavioralLoopback::success().await;
    let native = OpenAiCompatible::new(native_loopback.base(), "fixture-credential").unwrap();
    let (host, mut host_request) = host_provider(&fixture, host_loopback.base()).await;
    let mut native_request = fixture.request.clone();
    native_request.model = host_request.model.clone();
    configure_overflow(&mut native_request, 3);
    configure_overflow(&mut host_request, 3);
    for completed in 0..3 {
        let mut request = native_request.clone();
        configure_overflow(&mut request, completed);
        let Err(ContextGuardError::Overflow(ProviderContextDecision::CompactionRequired(detail))) =
            ContextGuard.check(&request, None)
        else {
            panic!("compaction {completed} must be required")
        };
        assert_overflow_detail(&detail, completed);
    }
    let native_result = run_trace_result(&native, native_request).await;
    let host_result = run_trace_result(&host, host_request).await;
    let native_detail = context_overflow_detail(&native_result);
    let host_detail = context_overflow_detail(&host_result);
    assert_overflow_detail(native_detail, 3);
    assert_overflow_detail(host_detail, 3);
    assert_eq!(native_detail, host_detail);
    assert!(native_result.1.is_empty() && host_result.1.is_empty());
    assert_eq!(native_loopback.request_count(), 0);
    assert_eq!(host_loopback.request_count(), 0);
    host.shutdown().expect("host shutdown");
    behavioral_case(
        "real_context_overflow_both_adapters",
        Dimension::ContextOverflow,
    )
}

async fn real_image_policy_case() -> ExecutedCase {
    let fixture = load("openai-compatible/happy-tool");
    let strict_native_loopback = BehavioralLoopback::success().await;
    let strict_host_loopback = BehavioralLoopback::success().await;
    let native = OpenAiCompatible::with_capabilities(
        strict_native_loopback.base(),
        "fixture-credential",
        MaxTokensField::MaxTokens,
        false,
    )
    .unwrap();
    let (host, mut host_request) =
        host_provider_without_images(&fixture, strict_host_loopback.base()).await;
    let mut native_request = fixture.request.clone();
    configure_image(&mut native_request, ImagePolicy::Strict);
    configure_image(&mut host_request, ImagePolicy::Strict);
    let native_strict = run_trace_result(&native, native_request).await;
    let host_strict = run_trace_result(&host, host_request).await;
    assert_identical_rejection(&native_strict, &host_strict);
    assert_eq!(strict_native_loopback.request_count(), 0);
    assert_eq!(strict_host_loopback.request_count(), 0);
    host.shutdown().expect("host shutdown");

    let native_loopback = BehavioralLoopback::success().await;
    let host_loopback = BehavioralLoopback::success().await;
    let native = OpenAiCompatible::with_capabilities(
        native_loopback.base(),
        "fixture-credential",
        MaxTokensField::MaxTokens,
        false,
    )
    .unwrap();
    let (host, mut host_request) =
        host_provider_without_images(&fixture, host_loopback.base()).await;
    let mut native_request = fixture.request.clone();
    configure_image(&mut native_request, ImagePolicy::Drop);
    configure_image(&mut host_request, ImagePolicy::Drop);
    let (native_trace, host_trace) = tokio::join!(
        run_trace(&native, native_request),
        run_trace(&host, host_request)
    );
    compare_equivalent_traces(&native_trace, &host_trace).unwrap();
    let native_captured = native_loopback.recorded().await;
    let host_captured = host_loopback.recorded().await;
    assert_no_image_preserves_text(&native_captured.body);
    assert_no_image_preserves_text(&host_captured.body);
    assert_semantic_request_eq(
        &semantic_request(&native_captured),
        &semantic_request(&host_captured),
    )
    .unwrap();
    host.shutdown().expect("host shutdown");
    behavioral_case("real_image_policy_both_adapters", Dimension::ImagePolicy)
}

fn behavioral_case(name: &str, dimension: Dimension) -> ExecutedCase {
    ExecutedCase {
        name: name.to_owned(),
        dimensions: [dimension].into_iter().collect(),
        native_impl: "lotta_providers::native::openai_compatible::OpenAiCompatible",
        host_impl: "lotta_providers::host::provider::HostProvider",
    }
}

fn configure_overflow(request: &mut ProviderRequest, compactions_completed: u8) {
    request.system_prompt =
        Some(lotta_runtime::boundary::ProviderText::new("overflow ".repeat(256)).unwrap());
    request.context = Some(ProviderContext {
        server_max: Some(1_024),
        catalog_max: Some(768),
        agent_max: Some(512),
        conversation_max: Some(32),
        measured_input_tokens: Some(64),
        compactions_completed,
    });
}

fn assert_overflow_detail(
    detail: &lotta_runtime::ports::ProviderContextOverflowDetail,
    completed: u8,
) {
    assert_eq!(detail.limit, 32);
    assert_eq!(detail.compactions_completed, completed);
    assert_eq!(detail.attempt, completed + 1);
    assert_eq!(detail.measured.as_ref().unwrap().tokens, 64);
    assert_eq!(
        detail.measured.as_ref().unwrap().provenance,
        ProviderContextTokenProvenance::Measured
    );
    assert!(detail.estimated.tokens > detail.limit);
    assert_eq!(
        detail.estimated.provenance,
        ProviderContextTokenProvenance::FallbackBytesPerToken
    );
    let safe = serde_json::to_string(detail).unwrap();
    assert!(!safe.contains("overflow overflow"));
}

fn context_overflow_detail(
    result: &(Result<(), lotta_runtime::RuntimeError>, Vec<ProviderEvent>),
) -> &lotta_runtime::ports::ProviderContextOverflowDetail {
    let Err(lotta_runtime::RuntimeError::ContextOverflow { detail }) = &result.0 else {
        panic!("expected context overflow, got {result:?}")
    };
    detail
}

fn configure_image(request: &mut ProviderRequest, image_policy: ImagePolicy) {
    request.image_policy = image_policy;
    request.messages = ProviderMessages::new(vec![ProviderMessage {
        role: ProviderMessageRole::User,
        content: ProviderContent::new(vec![
            ProviderContentPart::Text(
                lotta_runtime::boundary::ProviderText::new("preserved text".into()).unwrap(),
            ),
            ProviderContentPart::Image {
                media_type: ProviderName::new("image/png".into()).unwrap(),
                bytes: lotta_runtime::boundary::ProviderImageBytes::new(vec![1, 2, 3]).unwrap(),
            },
        ])
        .unwrap(),
        tool_call_id: None,
    }])
    .unwrap();
}

fn assert_no_image_preserves_text(body: &Value) {
    let encoded = body.to_string();
    assert!(encoded.contains("preserved text"));
    assert!(!encoded.contains("image_url"));
    assert!(!encoded.contains("base64"));
    assert!(!encoded.contains("AQID"));
}

fn assert_identical_rejection(
    native: &(Result<(), lotta_runtime::RuntimeError>, Vec<ProviderEvent>),
    host: &(Result<(), lotta_runtime::RuntimeError>, Vec<ProviderEvent>),
) {
    assert!(native.1.is_empty() && host.1.is_empty());
    assert_eq!(format!("{:?}", native.0), format!("{:?}", host.0));
    assert!(matches!(
        native.0,
        Err(lotta_runtime::RuntimeError::InvalidData { .. })
    ));
}

type InterruptOutcome = (
    Result<(), lotta_runtime::RuntimeError>,
    Vec<ProviderEvent>,
    Duration,
);

fn assert_cancelled_outcome(outcome: &InterruptOutcome) {
    assert!(
        matches!(
            outcome.0,
            Err(lotta_runtime::RuntimeError::Cancelled { .. })
        ),
        "actual cancellation outcome: {:?}",
        outcome.0
    );
    assert!(
        outcome.1.is_empty(),
        "late cancellation events: {:?}",
        outcome.1
    );
    assert!(
        outcome.2 <= Duration::from_millis(500),
        "cancellation took {:?}",
        outcome.2
    );
}

fn assert_timeout_outcome(outcome: &InterruptOutcome) {
    assert!(
        matches!(outcome.0, Err(lotta_runtime::RuntimeError::Timeout { .. })),
        "actual timeout outcome: {:?}",
        outcome.0
    );
    assert!(outcome.1.is_empty(), "late timeout events: {:?}", outcome.1);
}

async fn collect_interrupt_outcome(
    task: tokio::task::JoinHandle<InterruptOutcome>,
) -> InterruptOutcome {
    tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("stream cancellation exceeded 500ms")
        .expect("stream task")
}

async fn run_interrupt_case(
    provider: &dyn ProviderPort,
    request: ProviderRequest,
) -> InterruptOutcome {
    let sink_cancellation = tokio_util::sync::CancellationToken::new();
    let started = Instant::now();
    let (sink, mut receiver) =
        provider_event_channel(256, &sink_cancellation).expect("event channel");
    let producer = async {
        match provider.stream(request, sink).await {
            Err(error @ lotta_runtime::RuntimeError::Cancelled { .. })
            | Err(error @ lotta_runtime::RuntimeError::Timeout { .. }) => Err(error),
            result => result,
        }
    };
    let consumer = async {
        let mut events = Vec::new();
        loop {
            match receiver.receive().await {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(lotta_runtime::RuntimeError::Cancelled { .. }) => break,
                Err(error) => panic!("event receive: {error:?}"),
            }
        }
        events
    };
    let (result, events) = tokio::join!(producer, consumer);
    let result = match (result, events.as_slice()) {
        (
            Ok(()),
            [
                ProviderEvent::Error {
                    error: ProviderError::Cancelled(_),
                },
            ],
        ) => Err(lotta_runtime::RuntimeError::Cancelled {
            context: "provider cancellation outcome".into(),
        }),
        (
            Ok(()),
            [
                ProviderEvent::Error {
                    error: ProviderError::Timeout(_),
                },
            ],
        ) => Err(lotta_runtime::RuntimeError::Timeout {
            context: "provider timeout outcome".into(),
        }),
        (result, _) => result,
    };
    let events = if matches!(
        result,
        Err(lotta_runtime::RuntimeError::Cancelled { .. }
            | lotta_runtime::RuntimeError::Timeout { .. })
    ) {
        Vec::new()
    } else {
        events
    };
    (result, events, started.elapsed())
}

async fn run_trace_result(
    provider: &dyn ProviderPort,
    request: ProviderRequest,
) -> (Result<(), lotta_runtime::RuntimeError>, Vec<ProviderEvent>) {
    let cancellation = request.cancellation.clone();
    let (sink, mut receiver) = provider_event_channel(256, &cancellation).expect("event channel");
    let producer = provider.stream(request, sink);
    let consumer = async {
        let mut events = Vec::new();
        loop {
            match receiver.receive().await {
                Ok(Some(event)) => events.push(event),
                Ok(None) | Err(lotta_runtime::RuntimeError::Cancelled { .. }) => break,
                Err(error) => panic!("event receive: {error:?}"),
            }
        }
        events
    };
    tokio::join!(producer, consumer)
}

async fn host_provider_without_images(
    case: &ProviderCase,
    base: &reqwest::Url,
) -> (HostProvider, ProviderRequest) {
    let (provider, request) = host_provider(case, base).await;
    provider
        .unregister_adapter("task53-openai", "task53")
        .unwrap();
    provider
        .register_adapter(descriptor_with_input(
            "task53-openai",
            "fixture-model",
            "openai-completions",
            &request,
            &["text"],
        ))
        .await
        .unwrap();
    (provider, request)
}

fn descriptor_with_input(
    provider: &str,
    model: &str,
    api: &str,
    request: &ProviderRequest,
    input: &[&str],
) -> Value {
    let mut value = descriptor(provider, model, api, request);
    value["models"][0]["input"] = json!(input);
    value
}

struct BehavioralLoopback {
    base: reqwest::Url,
    requested: Arc<Notify>,
    count: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<Option<CapturedRequest>>,
}

impl BehavioralLoopback {
    async fn stall() -> Self {
        Self::start(true).await
    }

    async fn success() -> Self {
        Self::start(false).await
    }

    async fn start(stall: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requested = Arc::new(Notify::new());
        let count = Arc::new(AtomicUsize::new(0));
        let task_requested = Arc::clone(&requested);
        let task_count = Arc::clone(&count);
        let task = tokio::spawn(async move {
            let accepted = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accepted else {
                return None;
            };
            let request = read_request(&mut stream).await;
            task_count.fetch_add(1, Ordering::SeqCst);
            task_requested.notify_waiters();
            if stall {
                let head = concat!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n",
                    "connection: close\r\n\r\n"
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                tokio::time::sleep(Duration::from_secs(4)).await;
            } else {
                let body = b"data: {\"id\":\"equivalence\",\"model\":\"fixture-model\",\
                    \"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: \
                    {\"id\":\"equivalence\",\"model\":\"fixture-model\",\"choices\":[],\
                    \"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n\
                    data: [DONE]\n\n";
                let head = format!(
                    concat!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n",
                        "content-length: {}\r\nconnection: close\r\n\r\n"
                    ),
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
            }
            Some(request)
        });
        Self {
            base: reqwest::Url::parse(&format!("http://{address}/")).unwrap(),
            requested,
            count,
            task,
        }
    }

    fn base(&self) -> &reqwest::Url {
        &self.base
    }

    fn request_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    async fn wait_requested(&self) {
        loop {
            let notified = self.requested.notified();
            if self.request_count() != 0 {
                return;
            }
            tokio::time::timeout(Duration::from_secs(5), notified)
                .await
                .expect("request latch timeout");
        }
    }

    async fn recorded(self) -> CapturedRequest {
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("loopback timeout")
            .expect("loopback task")
            .expect("captured request")
    }
}

async fn execute_overlap() -> Vec<ExecutedCase> {
    let mut executed = Vec::new();
    for id in OVERLAP_IDS {
        let case = load(id);
        let (_, native, host) = execute_case(&case).await;
        assert_eq!(native.method, "POST");
        assert_eq!(host.method, "POST");
        executed.push(executed_case(&case));
    }
    executed
}

async fn execute_case(
    case: &ProviderCase,
) -> (Vec<ProviderEvent>, CapturedRequest, CapturedRequest) {
    let (native_trace, native_request) = run_one_side(case, Side::Native).await;
    let (host_trace, host_request) = run_one_side(case, Side::Host).await;
    compare_equivalent_traces(&native_trace, &host_trace).unwrap_or_else(|error| {
        panic!(
            "{}: {error}; native={native_trace:?}; host={host_trace:?}",
            case.record.id
        )
    });
    (native_trace, native_request, host_request)
}

#[derive(Clone, Copy)]
enum Side {
    Native,
    Host,
}

async fn run_one_side(case: &ProviderCase, side: Side) -> (Vec<ProviderEvent>, CapturedRequest) {
    let loopback = Loopback::start(case).await;
    let (trace, shutdown) = match side {
        Side::Native => {
            let adapter = native_adapter(case.record.dialect, loopback.base());
            (
                run_trace(adapter.as_ref(), case.request.clone()).await,
                None,
            )
        }
        Side::Host => {
            let (adapter, request) = host_provider(case, loopback.base()).await;
            let trace = run_trace(&adapter, request).await;
            (trace, Some(adapter))
        }
    };
    let request = loopback.recorded().await;
    if let Some(host) = shutdown {
        host.shutdown().expect("host shutdown");
    }
    (trace, request)
}

fn executed_case(case: &ProviderCase) -> ExecutedCase {
    ExecutedCase {
        name: case.record.id.clone(),
        dimensions: case.record.dimensions.as_slice().iter().copied().collect(),
        native_impl: "lotta_providers::native",
        host_impl: "lotta_providers::host::provider::HostProvider",
    }
}

fn compare_equivalent_traces(
    left: &[ProviderEvent],
    right: &[ProviderEvent],
) -> Result<(), String> {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    normalize_continuation_metadata(&mut left)?;
    normalize_continuation_metadata(&mut right)?;
    normalize_optional_host_fingerprint(&mut left, &mut right)?;
    compare_provider_traces(&left, &right).map_err(|error| {
        let values = left
            .iter()
            .zip(&right)
            .filter_map(|(left, right)| {
                let (
                    ProviderEvent::ProviderMetadata { metadata: left },
                    ProviderEvent::ProviderMetadata { metadata: right },
                ) = (left, right)
                else {
                    return None;
                };
                Some((
                    left.iter()
                        .map(|(key, value)| (key.as_str(), value))
                        .collect::<Vec<_>>(),
                    right
                        .iter()
                        .map(|(key, value)| (key.as_str(), value))
                        .collect::<Vec<_>>(),
                ))
            })
            .collect::<Vec<_>>();
        format!("{error}; metadata={values:?}")
    })
}

fn normalize_continuation_metadata(trace: &mut [ProviderEvent]) -> Result<(), String> {
    for event in trace {
        let ProviderEvent::ProviderMetadata { metadata } = event else {
            continue;
        };
        let entries = metadata.iter().filter(|(key, _)| {
            !matches!(key.as_str(), "request_id" | "response_ms" | "transport_id")
        });
        *metadata = metadata_from(entries)?;
    }
    Ok(())
}

/// pi-ai 0.82.1 does not expose OpenAI `system_fingerprint`; only its absence is optional.
fn normalize_optional_host_fingerprint(
    native: &mut [ProviderEvent],
    host: &mut [ProviderEvent],
) -> Result<(), String> {
    for (native_event, host_event) in native.iter_mut().zip(host.iter_mut()) {
        let (
            ProviderEvent::ProviderMetadata {
                metadata: native_metadata,
            },
            ProviderEvent::ProviderMetadata {
                metadata: host_metadata,
            },
        ) = (native_event, host_event)
        else {
            continue;
        };
        let fingerprint =
            ProviderName::new("system_fingerprint".into()).map_err(|error| error.to_string())?;
        if native_metadata.get(&fingerprint).is_some() && host_metadata.get(&fingerprint).is_none()
        {
            let entries = native_metadata
                .iter()
                .filter(|(key, _)| key.as_str() != fingerprint.as_str());
            *native_metadata = metadata_from(entries)?;
        }
    }
    Ok(())
}

fn metadata_from<'a>(
    entries: impl IntoIterator<Item = (&'a ProviderName, &'a Value)>,
) -> Result<ProviderMetadata, String> {
    let inputs = entries
        .into_iter()
        .map(|(key, value)| {
            Ok(ProviderMetadataInput::Persist {
                key: ProviderName::new(key.as_str().to_owned())
                    .map_err(|error| error.to_string())?,
                value: BoundedJsonValue::new(value.clone()).map_err(|error| error.to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    ProviderMetadata::classified(inputs).map_err(|error| error.to_string())
}

fn trace_mutations(trace: &[ProviderEvent]) -> Vec<Vec<ProviderEvent>> {
    let mut out = Vec::new();
    for key in ["id", "model", "system_fingerprint"] {
        out.push(mutate_metadata(trace, key));
    }
    out.push(mutate_usage(trace));
    out.push(mutate_content(trace));
    out.push(mutate_order(trace));
    out.push(mutate_tool_arguments(trace));
    out
}

fn mutate_metadata(trace: &[ProviderEvent], key: &str) -> Vec<ProviderEvent> {
    let mut out = trace.to_vec();
    for event in &mut out {
        let ProviderEvent::ProviderMetadata { metadata } = event else {
            continue;
        };
        let entries = metadata.iter().map(|(name, value)| {
            let value = if name.as_str() == key {
                json!("mutated")
            } else {
                value.clone()
            };
            (name, value)
        });
        let values = entries
            .map(|(name, value)| ProviderMetadataInput::Persist {
                key: ProviderName::new(name.as_str().to_owned()).unwrap(),
                value: BoundedJsonValue::new(value).unwrap(),
            })
            .collect::<Vec<_>>();
        *metadata = ProviderMetadata::classified(values).unwrap();
    }
    out
}

fn mutate_usage(trace: &[ProviderEvent]) -> Vec<ProviderEvent> {
    let mut out = trace.to_vec();
    if let Some(ProviderEvent::Usage { usage }) = out
        .iter_mut()
        .find(|event| matches!(event, ProviderEvent::Usage { .. }))
    {
        usage.output_tokens += 1;
    }
    out
}

fn mutate_content(trace: &[ProviderEvent]) -> Vec<ProviderEvent> {
    let mut out = trace.to_vec();
    if let Some(ProviderEvent::TextDelta { text }) = out
        .iter_mut()
        .find(|event| matches!(event, ProviderEvent::TextDelta { .. }))
    {
        *text = lotta_runtime::boundary::ProviderEventText::new("mutated".into()).unwrap();
    }
    out
}

fn mutate_order(trace: &[ProviderEvent]) -> Vec<ProviderEvent> {
    let mut out = trace.to_vec();
    out.swap(0, 1);
    out
}

fn mutate_tool_arguments(trace: &[ProviderEvent]) -> Vec<ProviderEvent> {
    let mut out = trace.to_vec();
    if let Some(ProviderEvent::ToolCallArgumentsDelta { chunk, .. }) = out
        .iter_mut()
        .find(|event| matches!(event, ProviderEvent::ToolCallArgumentsDelta { .. }))
    {
        *chunk = lotta_runtime::boundary::ToolArgumentChunk::new(b"{}".to_vec()).unwrap();
    }
    out
}

fn semantic_request(request: &CapturedRequest) -> SemanticRequest {
    let mut body = request.body.clone();
    canonicalize_body(&mut body);
    SemanticRequest {
        method: request.method.clone(),
        endpoint: canonical_endpoint(&request.path),
        auth_present: request.headers.iter().any(|(name, value)| {
            matches!(name.as_str(), "authorization" | "x-api-key") && !value.is_empty()
        }),
        body,
    }
}

fn canonicalize_body(body: &mut Value) {
    let Value::Object(object) = body else { return };
    object.remove("stream_options");
    object.remove("store");
    if object.get("max_tokens").is_none()
        && let Some(value) = object.remove("max_completion_tokens")
    {
        object.insert("max_tokens".into(), value);
    }
    canonicalize_messages(object);
    canonicalize_anthropic_system(object);
    canonicalize_anthropic_thinking(object);
    canonicalize_tools(object);
}

fn canonicalize_messages(object: &mut Map<String, Value>) {
    let Some(Value::Array(messages)) = object.get_mut("messages") else {
        return;
    };
    for message in messages {
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        if let Value::String(text) = content {
            *content = json!([{"type":"text","text":text.clone()}]);
        }
        normalize_image_parts(content);
    }
}

fn normalize_image_parts(content: &mut Value) {
    let Value::Array(parts) = content else { return };
    for part in parts {
        let Value::Object(part) = part else { continue };
        part.remove("cache_control");
        if let Some(Value::Object(url)) = part.remove("image_url")
            && let Some(value) = url.get("url")
        {
            part.insert("image".into(), value.clone());
        }
        if part.get("type") == Some(&json!("image_url")) {
            part.insert("type".into(), json!("image"));
        }
    }
}

fn canonicalize_anthropic_system(object: &mut Map<String, Value>) {
    let Some(system) = object.get_mut("system") else {
        return;
    };
    if let Value::Array(parts) = system
        && let Some(text) = parts.first().and_then(|part| part.get("text")).cloned()
    {
        *system = text;
    }
}

fn canonicalize_anthropic_thinking(object: &mut Map<String, Value>) {
    let Some(Value::Object(thinking)) = object.get_mut("thinking") else {
        return;
    };
    thinking.remove("display");
}

fn canonicalize_tools(object: &mut Map<String, Value>) {
    let Some(Value::Array(tools)) = object.get_mut("tools") else {
        return;
    };
    for tool in tools {
        let Value::Object(tool) = tool else { continue };
        if tool.get("type") == Some(&json!("function"))
            && let Some(Value::Object(function)) = tool.remove("function")
        {
            *tool = function;
        }
        tool.remove("strict");
        tool.remove("cache_control");
        tool.remove("eager_input_streaming");
        if let Some(value) = tool.remove("parameters") {
            tool.insert("input_schema".into(), value);
        }
    }
}

fn canonical_endpoint(path: &str) -> String {
    if path.ends_with("/chat/completions") {
        "chat/completions".into()
    } else if path.ends_with("/messages") {
        "messages".into()
    } else {
        path.into()
    }
}

fn assert_semantic_request_eq(
    left: &SemanticRequest,
    right: &SemanticRequest,
) -> Result<(), String> {
    if left == right {
        Ok(())
    } else {
        Err(format!("semantic request mismatch: {left:?} != {right:?}"))
    }
}

fn assert_request_mutations_fail(request: &SemanticRequest) {
    for key in ["model", "messages", "tools", "max_tokens"] {
        assert!(
            request.body.get(key).is_some(),
            "missing mutation target {key}"
        );
        let mut mutation = request.clone();
        mutation.body[key] = json!("mutated");
        assert!(assert_semantic_request_eq(request, &mutation).is_err());
    }
    if request.body.get("tool_choice").is_some() {
        let mut mutation = request.clone();
        mutation.body["tool_choice"] = json!("mutated");
        assert!(assert_semantic_request_eq(request, &mutation).is_err());
    }
    if request.body.get("thinking").is_some() {
        assert!(request.body["thinking"].get("budget_tokens").is_some());
        let mut mutation = request.clone();
        mutation.body["thinking"]["budget_tokens"] = json!(1);
        assert!(assert_semantic_request_eq(request, &mutation).is_err());
    }
    let mut system = request.clone();
    system.body["system"] = json!("mutated");
    assert!(assert_semantic_request_eq(request, &system).is_err());
    let mut image = request.clone();
    image.body["messages"] = json!([{"role":"user","content":[{"type":"image"}]}]);
    assert!(assert_semantic_request_eq(request, &image).is_err());
    let mut auth = request.clone();
    auth.auth_present = !auth.auth_present;
    assert!(assert_semantic_request_eq(request, &auth).is_err());
}

fn assert_inventory() {
    let index = load_index(&FixtureLoader::new()).expect("provider index");
    assert_eq!(index.dimensions, CANONICAL_DIMENSIONS);
    let indexed = index
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    let classified = OVERLAP_IDS
        .into_iter()
        .chain(UNSUPPORTED_IDS)
        .collect::<BTreeSet<_>>();
    assert_eq!(indexed, classified, "new fixture must be classified");
    let overlap = index
        .cases
        .iter()
        .filter(|case| matches!(case.dialect, Dialect::OpenaiCompatible | Dialect::Anthropic))
        .map(|case| case.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        overlap, OVERLAP_IDS,
        "new overlap fixture needs a real case"
    );
}

fn load(id: &str) -> ProviderCase {
    let loader = FixtureLoader::new();
    let index = load_index(&loader).expect("provider index");
    let record = index
        .cases
        .into_iter()
        .find(|case| case.id == id)
        .expect("indexed case");
    load_case(&loader, &record).unwrap_or_else(|error| panic!("{id}: {error}"))
}

fn native_adapter(dialect: Dialect, base: &reqwest::Url) -> Box<dyn ProviderPort> {
    match dialect {
        Dialect::Anthropic => Box::new(Anthropic::new(base, "fixture-credential").unwrap()),
        _ => Box::new(OpenAiCompatible::new(base, "fixture-credential").unwrap()),
    }
}

async fn run_trace(provider: &dyn ProviderPort, request: ProviderRequest) -> Vec<ProviderEvent> {
    let cancellation = request.cancellation.clone();
    let (sink, mut receiver) = provider_event_channel(256, &cancellation).expect("event channel");
    let producer = provider.stream(request, sink);
    let consumer = async {
        let mut events = Vec::new();
        loop {
            match receiver.receive().await {
                Ok(Some(event)) => events.push(event),
                Ok(None) | Err(lotta_runtime::RuntimeError::Cancelled { .. }) => break,
                Err(error) => panic!("event receive: {error:?}"),
            }
        }
        events
    };
    let (result, events) = tokio::join!(producer, consumer);
    if let Err(error) = result {
        assert!(
            matches!(error, lotta_runtime::RuntimeError::Cancelled { .. }),
            "provider stream: {error:?}"
        );
    }
    events
}

async fn host_provider(
    case: &ProviderCase,
    base: &reqwest::Url,
) -> (HostProvider, ProviderRequest) {
    let (config, _root) = host_config();
    let provider_id = if case.record.dialect == Dialect::Anthropic {
        "task53-anthropic"
    } else {
        "task53-openai"
    };
    let model_id = "fixture-model";
    let api = if case.record.dialect == Dialect::Anthropic {
        "anthropic-messages"
    } else {
        "openai-completions"
    };
    let mut request = case.request.clone();
    request.model.provider_id = NonEmptyString::new(provider_id).unwrap();
    request.model.handle = NonEmptyString::new(format!("{provider_id}/{model_id}")).unwrap();
    let options = HostOptions {
        base_url: Some(host_base(case.record.dialect, base)),
        env: json!({"lotta_api": api}),
        ..HostOptions::default()
    };
    let provider = HostProvider::spawn(
        config,
        owner(),
        HostAuth::ApiKey {
            value: "fixture-credential".into(),
        },
        options,
    )
    .await
    .expect("real host provider");
    provider
        .register_adapter(descriptor(provider_id, model_id, api, &request))
        .await
        .expect("host adapter");
    (provider, request)
}

fn descriptor(provider: &str, model: &str, api: &str, request: &ProviderRequest) -> Value {
    json!({
        "id": provider, "name": "Task53 replay", "owner": "task53",
        "adapter": {"type": "pi_ai_api", "api": api},
        "models": [{
            "id": model, "name": "Task53 model", "api": api, "provider": provider,
            "reasoning": request.reasoning.enabled, "input": ["text", "image"],
            "contextWindow": request.model.context_window,
            "maxTokens": request.output_tokens_max.get(),
            "cost": {"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"tiers":[]}
        }]
    })
}

fn host_base(dialect: Dialect, base: &reqwest::Url) -> String {
    let mut value = base.as_str().trim_end_matches('/').to_owned();
    if dialect == Dialect::OpenaiCompatible {
        value.push_str("/v1");
    }
    value
}

fn owner() -> SidecarOwnerIdentity {
    SidecarOwnerIdentity::new("task53", "runtime", "conversation").unwrap()
}

fn host_config() -> (HostConfig, PathBuf) {
    let source = source_root();
    let package = source
        .join("node_modules/@earendil-works/pi-ai")
        .canonicalize()
        .expect("pinned pi-ai installation");
    let root = temp_dir("host");
    let script = root.join("pi-ai-host.mjs");
    materialize_host_script(&script).expect("host script");
    let scratch = temp_dir("scratch");
    (
        HostConfig {
            bun_executable: bun(),
            host_script: script.canonicalize().unwrap(),
            package_root: package,
            scratch_cwd: scratch.canonicalize().unwrap(),
            test_mode: true,
        },
        root,
    )
}

fn source_root() -> PathBuf {
    std::env::var_os("LOTTA_LETTA_CODE_CHECKOUT")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../letta-code"),
            PathBuf::from,
        )
        .canonicalize()
        .expect("pinned source checkout")
}

fn bun() -> PathBuf {
    std::env::var_os("LOTTA_BUN_EXECUTABLE")
        .map_or_else(|| PathBuf::from("/Users/stan/.bun/bin/bun"), PathBuf::from)
        .canonicalize()
        .expect("Bun executable")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lotta-task53-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir(&path).expect("temporary directory");
    path
}

struct Loopback {
    base: reqwest::Url,
    task: tokio::task::JoinHandle<CapturedRequest>,
}

impl Loopback {
    async fn start(case: &ProviderCase) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = case.raw_stream.as_bytes().to_vec();
        let response = case.record.response.clone().expect("transport response");
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            write_response(
                &mut stream,
                response.status,
                response.headers.as_slice(),
                &body,
            )
            .await;
            request
        });
        Self {
            base: reqwest::Url::parse(&format!("http://{address}/")).unwrap(),
            task,
        }
    }

    fn base(&self) -> &reqwest::Url {
        &self.base
    }

    async fn recorded(self) -> CapturedRequest {
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("transport timeout")
            .expect("transport task")
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        bytes.extend_from_slice(&buffer[..read]);
        if read == 0 || request_complete(&bytes) {
            break;
        }
    }
    parse_request(&bytes)
}

fn request_complete(bytes: &[u8]) -> bool {
    let Some(start) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&bytes[..start]);
    let length = head
        .lines()
        .find_map(|line| line.split_once(':'))
        .and_then(|_| {
            head.lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    bytes.len() >= start + 4 + length
}

fn parse_request(bytes: &[u8]) -> CapturedRequest {
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = std::str::from_utf8(&bytes[..end]).unwrap();
    let mut lines = head.lines();
    let mut start = lines.next().unwrap().split_whitespace();
    let method = start.next().unwrap().to_owned();
    let path = start.next().unwrap().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let body = serde_json::from_slice(&bytes[end + 4..]).unwrap_or(Value::Null);
    CapturedRequest {
        method,
        path,
        headers,
        body,
    }
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    headers: &[lotta_testkit::fixtures::providers::HeaderFixture],
    body: &[u8],
) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for header in headers {
        response.push_str(&format!("{}: {}\r\n", header.name, header.value));
    }
    response.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    ));
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let _ = stream.shutdown().await;
}
