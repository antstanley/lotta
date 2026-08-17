use super::*;
use lotta_domain::{
    Agent, AgentId, BoundedMap, BoundedVec, Conversation, ConversationId, EntityExtras,
    ModelDescriptor, NonEmptyString, Timestamp, bounds::UNBOUNDED_MAP_FIELDS_MAX,
};
use lotta_runtime::ports::ProviderRequest;
use serde_json::{Value, json};

fn map(value: Value) -> BoundedMap<UNBOUNDED_MAP_FIELDS_MAX> {
    serde_json::from_value(value).unwrap_or_default()
}
fn descriptor(handle: &str, setting: u64) -> ModelDescriptor {
    ModelDescriptor {
        handle: NonEmptyString::new(handle).unwrap_or_else(|error| panic!("handle: {error}")),
        provider_id: NonEmptyString::new("p").unwrap_or_else(|error| panic!("provider: {error}")),
        available: true,
        context_window: None,
        model_settings: Some(map(json!({"setting": setting}))),
    }
}
fn to_override(value: &ModelDescriptor) -> ModelOverride {
    ModelOverride {
        handle: Some(
            value
                .handle
                .as_str()
                .parse()
                .unwrap_or_else(|error| panic!("handle: {error}")),
        ),
        settings: value.model_settings.as_ref().map(|map| {
            ModelSettings::normalize(
                &serde_json::to_value(map).unwrap_or_else(|error| panic!("map: {error}")),
            )
            .unwrap_or_else(|error| panic!("settings: {error}"))
        }),
    }
}

#[test]
fn resolution_order4_actual_entities_request_fields() {
    let agent_id = AgentId::accept("agent-a").unwrap_or_else(|error| panic!("agent: {error}"));
    let agent = Agent {
        id: agent_id.clone(),
        name: NonEmptyString::new("agent").unwrap_or_else(|error| panic!("name: {error}")),
        description: None,
        system: String::new(),
        tags: BoundedVec::new(Vec::new()).unwrap_or_else(|error| panic!("tags: {error}")),
        model: NonEmptyString::new("p/agent").unwrap_or_else(|error| panic!("model: {error}")),
        model_settings: map(json!({"setting":2})),
        hidden: None,
        compaction_settings: None,
        extras: EntityExtras::default(),
    };
    let time = Timestamp::parse_persisted_rfc3339("2026-01-01T00:00:00Z")
        .unwrap_or_else(|error| panic!("time: {error}"));
    let conversation = Conversation {
        id: ConversationId::accept("conversation-c")
            .unwrap_or_else(|error| panic!("conversation: {error}")),
        agent_id,
        archived: false,
        archived_at: None,
        created_at: time,
        updated_at: time,
        last_message_at: None,
        summary: None,
        in_context_message_ids: BoundedVec::new(Vec::new())
            .unwrap_or_else(|error| panic!("messages: {error}")),
        model: Some(Some("p/conversation".into())),
        model_settings: Some(map(json!({"setting":3}))),
        context_window_limit: None,
        hidden: None,
        tags: None,
        extras: EntityExtras::default(),
    };
    let request_descriptor = descriptor("p/request", 4);
    let request_field: fn(&ProviderRequest) -> &ModelDescriptor = |request| &request.model;
    let _ = request_field;
    let resolved = resolve_model(
        &to_override(&request_descriptor),
        &ModelOverride {
            handle: conversation
                .model
                .as_ref()
                .and_then(|value| value.as_ref())
                .map(|value| {
                    value
                        .parse()
                        .unwrap_or_else(|error| panic!("handle: {error}"))
                }),
            settings: conversation.model_settings.as_ref().map(|value| {
                ModelSettings::normalize(
                    &serde_json::to_value(value).unwrap_or_else(|error| panic!("map: {error}")),
                )
                .unwrap_or_else(|error| panic!("settings: {error}"))
            }),
        },
        &ModelOverride {
            handle: Some(
                agent
                    .model
                    .as_str()
                    .parse()
                    .unwrap_or_else(|error| panic!("handle: {error}")),
            ),
            settings: Some(
                ModelSettings::normalize(
                    &serde_json::to_value(&agent.model_settings)
                        .unwrap_or_else(|error| panic!("map: {error}")),
                )
                .unwrap_or_else(|error| panic!("settings: {error}")),
            ),
        },
        &to_override(&descriptor("p/default", 1)),
    )
    .unwrap_or_else(|error| panic!("resolve: {error}"));
    assert_eq!(
        resolved.handle,
        ModelHandle::new("p", "request").unwrap_or_else(|error| panic!("handle: {error}"))
    );
    assert_eq!(resolved.settings.get("setting"), Some(&Value::from(4)));
}
