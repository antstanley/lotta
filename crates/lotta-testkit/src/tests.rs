use crate::contract as contracts;
use crate::contract::fixtures as contract_fixtures;
use crate::fakes::*;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    ProcessOutputChunk, ProviderEventText, ProviderName, ToolArgumentChunk,
};
use lotta_runtime::ports::*;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn run_clock_contract() {
    let timestamp = contract_fixtures::timestamp();
    contracts::clock_contract(|| async { FakeClock::new(timestamp) }, timestamp).await;
    crate::clock::fake_clock_contract(FakeClock::new);
}

async fn run_id_generator_contract() {
    contracts::id_generator_contract(
        || async { FakeIdGenerator::new() },
        || async { FakeIdGenerator::with_next(u64::MAX) },
    )
    .await;
}

async fn run_agent_store_contract() {
    contracts::agent_store_contract(
        || async { FakeAgentStore::default() },
        contract_fixtures::agent("agent-a"),
        contract_fixtures::agent("agent-b"),
    )
    .await;
}

async fn run_conversation_store_contract() {
    contracts::conversation_store_contract(
        || async { FakeConversationStore::default() },
        contract_fixtures::conversation("agent-a", "conversation-a"),
        contract_fixtures::conversation("agent-a", "conversation-b"),
    )
    .await;
}

async fn run_transcript_store_contract() {
    let (manifest, session, appended) = contract_fixtures::transcript();
    contracts::transcript_store_contract(
        || async { FakeTranscriptStore::default() },
        lotta_domain::AgentId::accept("agent-a").expect("agent"),
        lotta_domain::ConversationId::accept("conversation-a").expect("conversation"),
        manifest,
        session,
        appended,
    )
    .await;
}

async fn run_memfs_contract() {
    contracts::memfs_contract(
        || async { FakeMemFs::default() },
        lotta_domain::AgentId::accept("agent-a").expect("agent"),
        contract_fixtures::memory_blocks(),
    )
    .await;
}

fn process_events() -> Vec<ProcessEvent> {
    vec![
        ProcessEvent::Stdout(ProcessOutputChunk::new(b"out".to_vec()).expect("stdout")),
        ProcessEvent::Stderr(ProcessOutputChunk::new(b"err".to_vec()).expect("stderr")),
    ]
}

fn process_outcome() -> ProcessOutcome {
    ProcessOutcome {
        exit_code: Some(0),
        timed_out: false,
    }
}

async fn run_sandbox_contract() {
    contracts::sandbox_contract(
        |scenario| async move {
            let fake = FakeSandbox::default();
            let configured = match scenario {
                contracts::ProcessContractScenario::Success
                | contracts::ProcessContractScenario::Backpressure
                | contracts::ProcessContractScenario::Cancelled
                | contracts::ProcessContractScenario::ReceiverClosed => {
                    (process_events(), Ok(process_outcome()))
                }
                contracts::ProcessContractScenario::NonZero => (
                    Vec::new(),
                    Ok(ProcessOutcome {
                        exit_code: Some(7),
                        timed_out: false,
                    }),
                ),
                contracts::ProcessContractScenario::TimedOut => (
                    Vec::new(),
                    Ok(ProcessOutcome {
                        exit_code: None,
                        timed_out: true,
                    }),
                ),
                contracts::ProcessContractScenario::AdapterError => (
                    Vec::new(),
                    Err(RuntimeError::AdapterFailure {
                        code: "configured",
                        context: "process contract".into(),
                    }),
                ),
            };
            fake.configure(configured.0, configured.1)
                .expect("configure");
            fake
        },
        contract_fixtures::process_request(),
    )
    .await;
}

async fn run_child_process_contract() {
    contracts::child_process_contract(
        |scenario| async move {
            let fake = FakeChildProcess::default();
            let configured = match scenario {
                contracts::ProcessContractScenario::Success
                | contracts::ProcessContractScenario::Backpressure
                | contracts::ProcessContractScenario::Cancelled
                | contracts::ProcessContractScenario::ReceiverClosed => {
                    (process_events(), Ok(process_outcome()))
                }
                contracts::ProcessContractScenario::NonZero => (
                    Vec::new(),
                    Ok(ProcessOutcome {
                        exit_code: Some(7),
                        timed_out: false,
                    }),
                ),
                contracts::ProcessContractScenario::TimedOut => (
                    Vec::new(),
                    Ok(ProcessOutcome {
                        exit_code: None,
                        timed_out: true,
                    }),
                ),
                contracts::ProcessContractScenario::AdapterError => (
                    Vec::new(),
                    Err(RuntimeError::AdapterFailure {
                        code: "configured",
                        context: "process contract".into(),
                    }),
                ),
            };
            fake.configure(configured.0, configured.1)
                .expect("configure");
            fake
        },
        contract_fixtures::process_request(),
    )
    .await;
}

fn provider_events() -> Vec<ProviderEvent> {
    let text = |value: &str| ProviderEventText::new(value.into()).expect("event text");
    let call_id = ToolCallId::from_name(
        lotta_runtime::boundary::ProviderName::new("call-1".into()).expect("call ID"),
    );
    let metadata = ProviderMetadata::classified([ProviderMetadataInput::Persist {
        key: ProviderName::new("cursor".into()).expect("key"),
        value: lotta_domain::BoundedJsonValue::new(json!("next")).expect("metadata"),
    }])
    .expect("classified metadata");
    vec![
        ProviderEvent::TextDelta {
            text: text("hello"),
        },
        ProviderEvent::ReasoningDelta {
            text: text("think"),
        },
        ProviderEvent::RedactedReasoning {
            marker: text("hidden"),
        },
        ProviderEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: text("tool"),
        },
        ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            chunk: ToolArgumentChunk::new(b"{}".to_vec()).expect("arguments"),
        },
        ProviderEvent::ToolCallEnd { call_id },
        ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: 2,
                output_tokens: 3,
                cached_input_tokens: 1,
                reasoning_tokens: 1,
            },
        },
        ProviderEvent::ProviderMetadata { metadata },
        ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: 3,
                output_tokens: 5,
                cached_input_tokens: 1,
                reasoning_tokens: 2,
            },
        },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]
}

async fn run_provider_contract() {
    contracts::provider_contract(
        |scenario| async move {
            let fake = FakeProvider::default();
            match scenario {
                contracts::ProviderContractScenario::Success
                | contracts::ProviderContractScenario::Cancelled
                | contracts::ProviderContractScenario::ReceiverClosed => {
                    fake.configure(provider_events(), None).expect("configure");
                }
                contracts::ProviderContractScenario::TerminalError => {
                    let text = ProviderEventText::new("prefix".into()).expect("text");
                    let context = ProviderErrorContext {
                        retry_after: None,
                        code: ProviderName::new("normalized".into()).expect("code"),
                        context: ProviderEventText::new("safe".into()).expect("context"),
                    };
                    fake.configure(
                        vec![
                            ProviderEvent::TextDelta { text },
                            ProviderEvent::Error {
                                error: ProviderError::Unknown(context),
                            },
                        ],
                        None,
                    )
                    .expect("configure");
                }
            }
            fake
        },
        contract_fixtures::provider_request(CancellationToken::new()),
    )
    .await;
}

fn tool_outcomes() -> Vec<Result<ToolOutcome, RuntimeError>> {
    let message = || ToolOutcomeMessage::new("message".into()).expect("message");
    let limit = ToolOutputLimit::new(1_024, 1_024).expect("limit");
    vec![
        Ok(ToolOutcome::Success {
            content: ToolResultText::new("success".into(), limit).expect("result"),
        }),
        Ok(ToolOutcome::UserDenied { message: message() }),
        Ok(ToolOutcome::Interrupted { message: message() }),
        Ok(ToolOutcome::Timeout { message: message() }),
        Ok(ToolOutcome::ValidationFailure { message: message() }),
        Ok(ToolOutcome::SandboxDenied { message: message() }),
        Ok(ToolOutcome::SpawnFailure { message: message() }),
        Ok(ToolOutcome::ToolDefinedError {
            code: ToolOutcomeCode::new("code".into()).expect("code"),
            message: message(),
        }),
        Err(RuntimeError::AdapterFailure {
            code: "configured",
            context: "tool contract".into(),
        }),
    ]
}

pub(crate) fn tool_request() -> ToolExecutionRequest {
    let schema = ToolInputSchema::new(
        lotta_domain::BoundedJsonValue::new(json!({"type":"object"})).expect("schema JSON"),
    )
    .expect("schema");
    let fields = lotta_domain::BoundedVec::new(vec![
        SecretFieldPath::new("/secret".into()).expect("secret path"),
    ])
    .expect("fields");
    let definition = ToolDefinition::new(
        InternalToolName::new("contract".into()).expect("internal name"),
        ModelFacingToolName::new("Contract".into()).expect("model name"),
        schema,
        ToolDescriptionAsset::new("description".into()).expect("description"),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("contract".into()).expect("permission"),
        ToolTimeout::new(Duration::from_secs(1)).expect("timeout"),
        ToolOutputLimit::new(1_024, 1_024).expect("output"),
        SecretRedactionSpec::new(fields, SecretRedactionPolicy::Redact).expect("redaction"),
    );
    ToolExecutionRequest {
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("contract-call".into()).expect("call name"),
        ),
        approval_grant: lotta_runtime::ports::ToolApprovalGrant::None,
        definition,
        input: ValidatedToolInput::new(
            lotta_domain::BoundedJsonValue::new(json!({"secret":"recognizable-secret"}))
                .expect("input JSON"),
        )
        .expect("validated input"),
        cancellation: CancellationToken::new(),
        deadline: ToolTimeout::new(Duration::from_millis(1)).expect("deadline"),
    }
}

fn success(value: &str) -> ToolOutcome {
    let limit = ToolOutputLimit::new(1_024, 1_024).expect("limit");
    ToolOutcome::Success {
        content: ToolResultText::new(value.into(), limit).expect("result"),
    }
}

async fn run_tool_contract() {
    let requests = (0..10).map(|_| tool_request()).collect::<Vec<_>>();
    contracts::tool_contract(
        |scenario| async move {
            let fake = FakeTool::default();
            let probe = contracts::ToolContractProbe::default();
            let responses = match scenario {
                contracts::ToolContractScenario::Success => vec![tool_outcomes().remove(0)],
                contracts::ToolContractScenario::UserDenied => vec![tool_outcomes().remove(1)],
                contracts::ToolContractScenario::Interrupted => vec![tool_outcomes().remove(2)],
                contracts::ToolContractScenario::Timeout => vec![tool_outcomes().remove(3)],
                contracts::ToolContractScenario::ValidationFailure => {
                    vec![tool_outcomes().remove(4)]
                }
                contracts::ToolContractScenario::SandboxDenied => vec![tool_outcomes().remove(5)],
                contracts::ToolContractScenario::SpawnFailure => vec![tool_outcomes().remove(6)],
                contracts::ToolContractScenario::ToolDefinedError => {
                    vec![tool_outcomes().remove(7)]
                }
                contracts::ToolContractScenario::AdapterError
                | contracts::ToolContractScenario::ObserveAscii
                | contracts::ToolContractScenario::ObserveUnicode => {
                    vec![tool_outcomes().remove(8)]
                }
                contracts::ToolContractScenario::Cancelled => vec![Ok(success("success"))],
                contracts::ToolContractScenario::Sequential => {
                    vec![Ok(success("first")), Ok(success("second"))]
                }
                contracts::ToolContractScenario::SetupBounds => Vec::new(),
            };
            fake.configure(responses).expect("configure");
            fake.attach_probe(probe.clone());
            if matches!(scenario, contracts::ToolContractScenario::Cancelled) {
                fake.hold_pending();
            }
            if matches!(scenario, contracts::ToolContractScenario::SetupBounds) {
                let mut snapshot = probe.snapshot();
                snapshot.setup_bounds_verified = true;
                probe.publish(snapshot);
            }
            contracts::ToolContractCase { port: fake, probe }
        },
        requests,
    )
    .await;
}

macro_rules! generated_port_tests {
    ($( $kind:ident, $contract:ident, $fake_test:ident, $helper:ident,
        $adapter:ident, $port:path, $fixture:ident; )*) => {
        mod contract {
            $(
                #[tokio::test]
                async fn $contract() {
                    super::$helper().await;
                }
            )*
        }

        mod fakes {
            $(
                #[tokio::test]
                async fn $fake_test() {
                    super::$helper().await;
                }
            )*
        }

        #[test]
        fn generated_structural_coercions_compile() {
            fn default_fixture<T: Default>() -> T { T::default() }
            fn clock_fixture() -> FakeClock {
                FakeClock::new(contract_fixtures::timestamp())
            }
            $(
                let adapter: $adapter = $fixture();
                let _: &dyn $port = &adapter;
            )*
        }
    };
}

crate::port_matrix!(generated_port_tests);

mod source_audit;
