use super::test_support::*;
use super::*;
use crate::boundary::ProviderEventText;
use crate::ports::*;
use lotta_domain::{Agent, Conversation, ModelDescriptor, NonEmptyString};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct CountingModelRepository {
    agent: Arc<Agent>,
    conversation: Arc<Conversation>,
    updates: AtomicUsize,
}

impl CountingModelRepository {
    fn load(&self) -> (Arc<Agent>, Arc<Conversation>) {
        (Arc::clone(&self.agent), Arc::clone(&self.conversation))
    }

    fn derive_request(&self) -> ProviderRequest {
        let (agent, conversation) = self.load();
        let handle = conversation
            .model
            .as_ref()
            .and_then(|value| value.as_ref())
            .map_or_else(
                || agent.model.clone(),
                |value| NonEmptyString::new(value.clone()).unwrap(),
            );
        let mut request = request(10_000);
        request.model = ModelDescriptor {
            handle,
            provider_id: NonEmptyString::new("persisted-provider").unwrap(),
            available: true,
            context_window: Some(4_096),
            model_settings: None,
        };
        request
    }

    fn update(&self, _agent: Arc<Agent>, _conversation: Arc<Conversation>) {
        self.updates.fetch_add(1, Ordering::SeqCst);
    }
}

async fn execute_fallback(events: &Events) -> (RetryTerminal, Vec<ProviderEvent>, usize, usize) {
    let time = FakeTime::default();
    let source = ScriptedPort::new(vec![
        vec![failure(ProviderFailureKind::Transient)],
        vec![failure(ProviderFailureKind::Busy)],
    ]);
    let destination = ScriptedPort::new(vec![vec![
        ProviderEvent::TextDelta {
            text: ProviderEventText::new("fallback".into()).unwrap(),
        },
        stop(),
    ]]);
    let source_route = ProviderRoute::new("native", "anthropic");
    let destination_route = ProviderRoute::new("bedrock", "anthropic");
    let destination_model = ModelDescriptor {
        handle: NonEmptyString::new("bedrock-model").unwrap(),
        provider_id: NonEmptyString::new("bedrock-provider").unwrap(),
        available: true,
        context_window: Some(4_096),
        model_settings: None,
    };
    let route =
        FallbackRoute::new(source_route.clone(), destination_route, destination_model).unwrap();
    let req = request(10_000);
    let (sink, mut receiver) = provider_event_channel(8, &req.cancellation).unwrap();
    let executor = RetryExecutor::new(&time, &time, events, RetryPolicy::default(), Some(&route));
    let terminal = executor
        .execute(source_route, &source, Some(&destination), req, sink)
        .await
        .unwrap();
    let mut output = Vec::new();
    while let Some(event) = receiver.receive().await.unwrap() {
        output.push(event);
    }
    assert_eq!(source.models(), ["source-model", "source-model"]);
    assert_eq!(destination.models(), ["bedrock-model"]);
    (terminal, output, source.calls(), destination.calls())
}

#[tokio::test]
async fn emits_named_retry_event() {
    let events = Events::default();
    let (terminal, output, source_calls, destination_calls) = execute_fallback(&events).await;
    assert_eq!(terminal, RetryTerminal::Success);
    assert_eq!(output.len(), 2);
    assert!(matches!(output[1], ProviderEvent::Stop { .. }));
    assert_eq!((source_calls, destination_calls), (2, 1));
    let values = events.values.lock().unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values[1].source, ProviderRoute::new("native", "anthropic"));
    assert_eq!(
        values[1].destination,
        ProviderRoute::new("bedrock", "anthropic")
    );
    assert_eq!(values[1].reason, RetryReason::TransportFallback);
    assert_eq!(values[1].attempt, 2);
}

#[tokio::test]
async fn does_not_modify_persisted_model() {
    let agent: Agent = serde_json::from_value(serde_json::json!({
        "id": "agent-persisted", "name": "agent-persisted", "description": null,
        "system": "system", "tags": [], "model": "persisted-source-model",
        "model_settings": {}, "hidden": false, "compaction_settings": null
    }))
    .unwrap();
    let conversation: Conversation = serde_json::from_value(serde_json::json!({
        "id": "conversation-persisted", "agent_id": "agent-persisted", "archived": false,
        "created_at": "2026-08-14T00:00:00Z", "updated_at": "2026-08-14T00:00:00Z",
        "in_context_message_ids": []
    }))
    .unwrap();
    let repository = CountingModelRepository {
        agent: Arc::new(agent),
        conversation: Arc::new(conversation),
        updates: AtomicUsize::new(0),
    };
    repository.update(
        Arc::clone(&repository.agent),
        Arc::clone(&repository.conversation),
    );
    repository.updates.store(0, Ordering::SeqCst);
    let (agent_before, conversation_before) = repository.load();
    let agent_bytes = serde_json::to_vec(agent_before.as_ref()).unwrap();
    let conversation_bytes = serde_json::to_vec(conversation_before.as_ref()).unwrap();
    let request = repository.derive_request();

    let time = FakeTime::default();
    let events = Events::default();
    let source = ScriptedPort::new(vec![
        vec![failure(ProviderFailureKind::Transient)],
        vec![failure(ProviderFailureKind::Busy)],
    ]);
    let destination = ScriptedPort::new(vec![vec![
        ProviderEvent::TextDelta {
            text: ProviderEventText::new("fallback".into()).unwrap(),
        },
        stop(),
    ]]);
    let source_route = ProviderRoute::new("native", "openai");
    let fallback_model = ModelDescriptor {
        handle: NonEmptyString::new("fallback-model").unwrap(),
        provider_id: NonEmptyString::new("fallback-provider").unwrap(),
        available: true,
        context_window: Some(4_096),
        model_settings: None,
    };
    let route = FallbackRoute::new(
        source_route.clone(),
        ProviderRoute::new("fallback", "openai"),
        fallback_model,
    )
    .unwrap();
    let (sink, mut receiver) = provider_event_channel(8, &request.cancellation).unwrap();
    let executor = RetryExecutor::new(&time, &time, &events, RetryPolicy::default(), Some(&route));
    assert_eq!(
        executor
            .execute(source_route, &source, Some(&destination), request, sink)
            .await
            .unwrap(),
        RetryTerminal::Success
    );
    while receiver.receive().await.unwrap().is_some() {}

    let (agent_after, conversation_after) = repository.load();
    assert!(Arc::ptr_eq(&agent_before, &agent_after));
    assert!(Arc::ptr_eq(&conversation_before, &conversation_after));
    assert_eq!(
        serde_json::to_vec(agent_after.as_ref()).unwrap(),
        agent_bytes
    );
    assert_eq!(
        serde_json::to_vec(conversation_after.as_ref()).unwrap(),
        conversation_bytes
    );
    assert_eq!(repository.updates.load(Ordering::SeqCst), 0);
    assert_eq!(
        source.models(),
        ["persisted-source-model", "persisted-source-model"]
    );
    assert_eq!(destination.models(), ["fallback-model"]);
}

#[tokio::test]
async fn event_failure_stops_before_fallback_or_sleep() {
    let time = FakeTime::default();
    let events = Events {
        values: Mutex::new(Vec::new()),
        fail: true,
    };
    let port = ScriptedPort::new(vec![
        vec![failure(ProviderFailureKind::Transient)],
        vec![stop()],
    ]);
    assert!(
        run(RetryPolicy::default(), &time, &events, &port)
            .await
            .is_err()
    );
    assert_eq!(port.calls(), 1);
    assert!(time.delays().is_empty());
}
