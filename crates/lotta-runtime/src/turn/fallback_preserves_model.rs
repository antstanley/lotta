use super::*;
use crate::ports::{ProviderError, ProviderErrorContext, ProviderEvent, StopReason};
use crate::retry::{FallbackRoute, ProviderRoute};
use crate::turn::test_support::*;
use lotta_domain::{ModelDescriptor, NonEmptyString};
use std::sync::atomic::Ordering;

fn failure() -> Vec<ProviderEvent> {
    vec![ProviderEvent::Error {
        error: ProviderError::Unavailable(ProviderErrorContext {
            retry_after: Some(crate::retry::RetryAfter::Milliseconds(0)),
            code: crate::boundary::ProviderName::new("temporary".into()).unwrap(),
            context: text("temporary"),
        }),
    }]
}

fn success() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta {
            text: text("fallback"),
        },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]
}

fn fallback_route() -> FallbackRoute {
    FallbackRoute::new(
        ProviderRoute::new("native", "openai"),
        ProviderRoute::new("fallback", "openai"),
        ModelDescriptor {
            handle: NonEmptyString::new("fallback-model").unwrap(),
            provider_id: NonEmptyString::new("fallback-provider").unwrap(),
            available: true,
            context_window: Some(4_096),
            model_settings: None,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn request_scoped_fallback_uses_destination_model_only() {
    let primary = ScriptedProvider::new(vec![failure(), failure(), failure(), failure()]);
    let fallback = ScriptedProvider::new(vec![success()]);
    let tools = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let catalog = catalog(&[]);
    let request = request();
    let source_model = request.model.clone();
    let ports = TurnPorts::configured(
        ProviderRoute::new("native", "openai"),
        &primary,
        vec![ConfiguredFallback {
            route: fallback_route(),
            provider: &fallback,
        }],
        &tools,
        &catalog,
        &effects,
    );
    assert_eq!(
        run_turn(&mut runtime, handle, lease, request, ports)
            .await
            .unwrap(),
        TurnRunOutcome::Completed
    );
    assert_eq!(primary.calls.load(Ordering::SeqCst), 4);
    assert_eq!(fallback.calls.load(Ordering::SeqCst), 1);
    assert!(
        primary
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.model == source_model)
    );
    assert_eq!(
        fallback.requests.lock().unwrap()[0].model.handle.as_str(),
        "fallback-model"
    );
}
