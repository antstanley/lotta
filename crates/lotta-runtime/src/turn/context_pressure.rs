use super::{CompactionPort, CompactionProgress, RequestRefreshPort, TurnPorts, run_turn};
use crate::ports::{
    PortFuture, ProviderContext, ProviderContextOverflowDetail, ProviderEvent, ProviderRequest,
    StopReason,
};
use crate::turn::test_support::{
    RecordingEffects, ScriptedProvider, SequencingTool, catalog, request, runtime, text,
};
use crate::{RuntimeError, TurnRunOutcome};
use lotta_domain::TurnLease;
use std::collections::VecDeque;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct ContextSeam {
    compactions: AtomicUsize,
    refreshes: AtomicUsize,
    requests: Mutex<VecDeque<ProviderRequest>>,
    triggers: Mutex<Vec<crate::compaction::CompactionTrigger>>,
}

impl ContextSeam {
    fn new(requests: Vec<ProviderRequest>) -> Self {
        Self {
            compactions: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            requests: Mutex::new(requests.into()),
            triggers: Mutex::new(Vec::new()),
        }
    }
}

impl CompactionPort for ContextSeam {
    fn compact(
        &self,
        request: ProviderRequest,
        _: TurnLease,
        _: ProviderContextOverflowDetail,
        trigger: crate::compaction::CompactionTrigger,
    ) -> PortFuture<'_, CompactionProgress> {
        self.compactions.fetch_add(1, Ordering::SeqCst);
        self.triggers.lock().unwrap().push(trigger);
        Box::pin(async move {
            Ok(CompactionProgress {
                tokens_before: request.normalized_wire_bytes()? as u64,
                tokens_after: 1,
                messages_before: request.messages.len(),
                messages_after: 0,
            })
        })
    }
}

impl RequestRefreshPort for ContextSeam {
    fn refresh(&self, _: ProviderRequest, _: TurnLease) -> PortFuture<'static, ProviderRequest> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        let request = self.requests.lock().unwrap().pop_front();
        Box::pin(async move {
            request.ok_or_else(|| RuntimeError::InvalidData {
                context: "missing refreshed request".into(),
            })
        })
    }
}

fn success() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: text("done") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]
}

fn pressured() -> ProviderRequest {
    let mut request = request();
    request.system_prompt = Some(crate::boundary::ProviderText::new("x".repeat(12_000)).unwrap());
    request.context = Some(ProviderContext {
        server_max: Some(1_400),
        catalog_max: Some(1_300),
        agent_max: Some(1_200),
        conversation_max: Some(1_000),
        ..ProviderContext::default()
    });
    request
}

fn compacted() -> ProviderRequest {
    let mut request = request();
    request.output_tokens_max = crate::ports::TokenLimit::new(64).unwrap();
    request.context = Some(ProviderContext {
        server_max: Some(4_096),
        catalog_max: Some(4_096),
        ..ProviderContext::default()
    });
    request
}

#[tokio::test]
async fn preflight_compacts_rebuilds_and_retries_once() {
    let provider = ScriptedProvider::new(vec![success()]);
    let seam = std::sync::Arc::new(ContextSeam::new(vec![compacted()]));
    let tools = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let catalog = catalog(&[]);
    let ports = TurnPorts::new(&provider, &tools, &catalog, &effects)
        .with_context_ports(seam.as_ref(), std::sync::Arc::clone(&seam) as _);
    let outcome = run_turn(&mut runtime, handle, lease, pressured(), ports)
        .await
        .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(seam.compactions.load(Ordering::SeqCst), 1);
    assert_eq!(
        super::turn_loop::effective_context_limit(&pressured()),
        1_000
    );
    assert_eq!(seam.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *seam.triggers.lock().unwrap(),
        vec![crate::compaction::CompactionTrigger::Pressure]
    );
}

fn overflow_detail() -> ProviderContextOverflowDetail {
    ProviderContextOverflowDetail {
        measured: None,
        estimated: crate::ports::ProviderContextTokenCount {
            tokens: 5_000,
            provenance: crate::ports::ProviderContextTokenProvenance::Measured,
        },
        limit: 4_096,
        provider: "fake-provider".into(),
        model: "fake".into(),
        attempt: 1,
        compactions_completed: 0,
    }
}

fn overflow_error() -> RuntimeError {
    RuntimeError::ContextOverflow {
        detail: overflow_detail(),
    }
}

#[tokio::test]
async fn compaction_without_progress_is_terminal_before_refresh_or_resend() {
    struct NoProgress(AtomicUsize);
    impl CompactionPort for NoProgress {
        fn compact(
            &self,
            request: ProviderRequest,
            _: TurnLease,
            _: ProviderContextOverflowDetail,
            _: crate::compaction::CompactionTrigger,
        ) -> PortFuture<'_, CompactionProgress> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(CompactionProgress {
                    tokens_before: request.normalized_wire_bytes()? as u64,
                    tokens_after: request.normalized_wire_bytes()? as u64,
                    messages_before: request.messages.len(),
                    messages_after: request.messages.len(),
                })
            })
        }
    }
    impl RequestRefreshPort for NoProgress {
        fn refresh(
            &self,
            request: ProviderRequest,
            _: TurnLease,
        ) -> PortFuture<'static, ProviderRequest> {
            Box::pin(async move { Ok(request) })
        }
    }
    let provider = ScriptedProvider::new(vec![success()]);
    let seam = std::sync::Arc::new(NoProgress(AtomicUsize::new(0)));
    let tools = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let catalog = catalog(&[]);
    let ports = TurnPorts::new(&provider, &tools, &catalog, &effects)
        .with_context_ports(seam.as_ref(), std::sync::Arc::clone(&seam) as _);
    assert!(matches!(
        run_turn(&mut runtime, handle, lease, pressured(), ports).await,
        Err(RuntimeError::ContextOverflow { .. })
    ));
    assert_eq!(seam.0.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_overflow_compacts_max_three_then_typed() {
    use crate::turn::test_support::ProviderScript;
    let provider = ScriptedProvider::configured(vec![
        ProviderScript::Error(overflow_error()),
        ProviderScript::Error(overflow_error()),
        ProviderScript::Error(overflow_error()),
        ProviderScript::Error(overflow_error()),
    ]);
    let seam = std::sync::Arc::new(ContextSeam::new(vec![
        compacted(),
        compacted(),
        compacted(),
    ]));
    let tools = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let catalog = catalog(&[]);
    let ports = TurnPorts::new(&provider, &tools, &catalog, &effects)
        .with_context_ports(seam.as_ref(), std::sync::Arc::clone(&seam) as _);
    let error = run_turn(&mut runtime, handle, lease, compacted(), ports)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::ContextOverflow { .. }));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
    assert_eq!(seam.compactions.load(Ordering::SeqCst), 3);
    assert!(
        matches!(error, RuntimeError::ContextOverflow { detail } if detail.attempt == 4
        && detail.compactions_completed == 3)
    );
    assert_eq!(seam.refreshes.load(Ordering::SeqCst), 3);
    assert_eq!(
        *seam.triggers.lock().unwrap(),
        vec![crate::compaction::CompactionTrigger::ProviderOverflow; 3]
    );
}
