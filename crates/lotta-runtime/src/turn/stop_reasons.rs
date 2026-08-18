use super::*;
use crate::RuntimeError;
use crate::boundary::ProviderName;
use crate::ports::{ProviderError, ProviderErrorContext, ProviderEvent};
use crate::retry::{ProviderFailure, ProviderFailureKind};
use crate::turn::test_support::{
    RecordingEffects, ScriptedProvider, SequencingTool, catalog, request, runtime, text,
};
use lotta_domain::TurnStateKind;

fn context(code: &str) -> ProviderErrorContext {
    ProviderErrorContext::new(ProviderName::new(code.into()).unwrap(), text("scrubbed"))
}

struct EmptyExecutor(ProviderFailure);

impl ProviderTurnExecutorPort for EmptyExecutor {
    fn execute<'a>(
        &'a self,
        _: crate::retry::ProviderRoute,
        _: &'a dyn crate::ports::ProviderPort,
        _: &'a [&'a dyn crate::ports::ProviderPort],
        _: crate::ports::ProviderRequest,
        _: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, crate::retry::RetryTerminal> {
        let failure = self.0.clone();
        Box::pin(async move {
            Ok(crate::retry::RetryTerminal::Failure {
                failure,
                attempt_count: 3,
            })
        })
    }
}

async fn run_direct(error: ProviderError) -> (TurnStopRecord, RecordingEffects) {
    let provider = ScriptedProvider::new(vec![vec![ProviderEvent::Error { error }]]);
    let effects = RecordingEffects::default();
    let tool = SequencingTool::new(Vec::new());
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::direct(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(*effects.order.lock().unwrap(), ["stop", "event"]);
    assert_eq!(effects.events.lock().unwrap().len(), 1);
    let record = effects.stops.lock().unwrap().first().unwrap().clone();
    let encoded = serde_json::to_vec(&record).unwrap();
    let reloaded: TurnStopRecord = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(record, reloaded);
    (record, effects)
}

#[tokio::test]
async fn empty_response() {
    let failure = ProviderFailure::new(ProviderFailureKind::Empty, "empty");
    let provider = ScriptedProvider::new(vec![Vec::new()]);
    let executor = std::sync::Arc::new(EmptyExecutor(failure));
    let effects = RecordingEffects::default();
    let tool = SequencingTool::new(Vec::new());
    let (mut runtime, handle, lease) = runtime();
    let ports = TurnPorts {
        provider: TurnProvider::Retrying {
            route: crate::retry::ProviderRoute::new("direct", "test"),
            source: &provider,
            fallbacks: Vec::new(),
            executor,
        },
        provider_start: None,
        tools: &tool,
        controller_tools: None,
        approvals: None,
        catalog: &catalog(&[]),
        effects: &effects,
        compaction: None,
        request_refresh: None,
        children: None,
        post_turn: None,
    };
    run_turn(&mut runtime, handle, lease, request(), ports)
        .await
        .unwrap();
    assert_eq!(
        effects.stops.lock().unwrap()[0].reason,
        TurnStopReason::EmptyResponse
    );
}

#[tokio::test]
async fn context_overflow() {
    let (record, _) = run_direct(ProviderError::ContextOverflow(context("context"))).await;
    assert_eq!(record.reason, TurnStopReason::ContextOverflow);
    assert_eq!(record.reason.wire_value(), "context_overflow");
}

#[tokio::test]
async fn transport_failure() {
    let (record, _) = run_direct(ProviderError::Unavailable(context("transport"))).await;
    assert_eq!(record.reason, TurnStopReason::TransportFailure);
    assert_eq!(record.reason.wire_value(), "transport_failure");
}

#[tokio::test]
async fn provider_quota_error() {
    let (record, _) = run_direct(ProviderError::Quota(context("quota"))).await;
    assert_eq!(record.reason, TurnStopReason::ProviderQuotaError);
    assert_eq!(record.reason.wire_value(), "provider_quota_error");
}

#[tokio::test]
async fn user_cancellation() {
    let (record, _) = run_direct(ProviderError::Cancelled(context("cancelled"))).await;
    assert_eq!(record.reason, TurnStopReason::UserCancellation);
    assert_eq!(record.reason.wire_value(), "user_cancellation");
}

#[tokio::test]
async fn quota_vs_rate_limit() {
    let (quota, _) = run_direct(ProviderError::Quota(context("quota"))).await;
    let (rate, _) = run_direct(ProviderError::RateLimit(context("rate_limit"))).await;
    assert_eq!(quota.reason, TurnStopReason::ProviderQuotaError);
    assert_eq!(rate.reason, TurnStopReason::TransportFailure);
}

#[tokio::test]
async fn direct_error_cancel_mutation() {
    let (transport, _) = run_direct(ProviderError::Unavailable(context("same"))).await;
    let (cancelled, _) = run_direct(ProviderError::Cancelled(context("same"))).await;
    assert_ne!(transport.reason, cancelled.reason);
}

#[tokio::test]
async fn persistence_failure_leaves_lease_active_and_emits_nothing() {
    #[derive(Default)]
    struct FailingEffects(RecordingEffects);
    impl TurnEffectPort for FailingEffects {
        fn persist_projection(&self, value: TurnProjection) -> Result<(), RuntimeError> {
            self.0.persist_projection(value)
        }
        fn persist_stop_reason(&self, _: TurnStopRecord) -> Result<(), RuntimeError> {
            Err(RuntimeError::AdapterFailure {
                code: "persist_stop",
                context: "durable stop".into(),
            })
        }
        fn emit(&self, value: TurnEvent) -> Result<(), RuntimeError> {
            self.0.emit(value)
        }
        fn append_tool_result(&self, value: ToolResultRecord) -> Result<(), RuntimeError> {
            self.0.append_tool_result(value)
        }
        fn persist_controller_request(
            &self,
            _: super::ControllerToolRequestRecord,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn persist_compaction_request(
            &self,
            value: &crate::turn::CompactionRequest,
        ) -> Result<(), RuntimeError> {
            self.0.persist_compaction_request(value)
        }

        fn persist_control_request(&self, value: &ControlRequest) -> Result<(), RuntimeError> {
            self.0.persist_control_request(value)
        }
    }
    let effects = FailingEffects::default();
    let provider = ScriptedProvider::new(vec![vec![ProviderEvent::Error {
        error: ProviderError::Quota(context("quota")),
    }]]);
    let tool = SequencingTool::new(Vec::new());
    let (mut runtime, handle, lease) = runtime();
    assert!(
        run_turn(
            &mut runtime,
            handle.clone(),
            lease,
            request(),
            TurnPorts::direct(&provider, &tool, &catalog(&[]), &effects),
        )
        .await
        .is_err()
    );
    assert!(effects.0.events.lock().unwrap().is_empty());
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Active
    );
}
