use lotta_domain::{
    Agent, AgentId, Conversation, ConversationId, ModelDescriptor, NonEmptyString, ProviderStack,
    RuntimeScope, SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat,
};
use lotta_runtime::boundary::{
    ConfinedPath, InitialMemoryBlocks, ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax,
    Program,
};
use lotta_runtime::ports::{
    ImagePolicy, ProcessRequest, ProviderDeadline, ProviderMessages, ProviderRequest,
    ProviderToolChoice, ProviderTools, ReasoningControls, TokenLimit,
};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Returns a deterministic valid agent fixture.
///
/// # Panics
/// Panics only if the canonical fixture stops satisfying domain validation.
#[must_use]
pub fn agent(id: &str) -> Agent {
    serde_json::from_value(json!({
        "id": id, "name": id, "description": null, "system": "system", "tags": [],
        "model": "test/model", "model_settings": {}, "hidden": false,
        "compaction_settings": null
    }))
    .expect("agent fixture")
}

/// Returns a deterministic scoped conversation fixture.
///
/// # Panics
/// Panics only if the supplied identifiers or canonical fixture fail domain validation.
#[must_use]
pub fn conversation(agent: &str, id: &str) -> Conversation {
    serde_json::from_value(json!({
        "id": id, "agent_id": agent, "archived": false,
        "created_at": "2026-08-14T00:00:00Z", "updated_at": "2026-08-14T00:00:00Z",
        "in_context_message_ids": []
    }))
    .expect("conversation fixture")
}

/// Returns a deterministic transcript manifest and two ordered entries.
///
/// # Panics
/// Panics only if a canonical non-empty fixture identifier becomes invalid.
#[must_use]
pub fn transcript() -> (TranscriptManifest, TranscriptEntry, TranscriptEntry) {
    let timestamp = timestamp();
    let manifest = TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: timestamp,
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    };
    let session = session_entry("session", timestamp);
    let message = session_entry("session-two", timestamp);
    (manifest, session, message)
}

fn session_entry(id: &str, timestamp: Timestamp) -> TranscriptEntry {
    TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: NonEmptyString::new(id).expect("session"),
        timestamp,
        cwd: "/contract".into(),
    })
}

/// Returns deterministic initial memory blocks.
///
/// # Panics
/// Panics only if an empty initial-memory collection becomes invalid.
#[must_use]
pub fn memory_blocks() -> InitialMemoryBlocks {
    InitialMemoryBlocks::new(Vec::new()).expect("empty blocks")
}

/// Returns a deterministic process request.
///
/// # Panics
/// Panics only if canonical process fixture values stop satisfying boundary validation.
#[must_use]
pub fn process_request() -> ProcessRequest {
    ProcessRequest::new(
        RuntimeScope::new(
            AgentId::accept("agent-contract").expect("agent"),
            ConversationId::accept("conversation-contract").expect("conversation"),
            None,
        ),
        Program::new("program".into()).expect("program"),
        ProcessArguments::new(Vec::new()).expect("arguments"),
        ConfinedPath::new("/contract".into(), "/contract".into()).expect("directory"),
        ProcessEnvironment::new(Vec::new()).expect("environment"),
        None,
        ProcessOutputBytesMax::new(128).expect("output bound"),
        Duration::from_secs(1),
    )
    .expect("request")
}

/// Returns a normalized provider request with deterministic values.
///
/// # Panics
/// Panics only if canonical provider fixture values stop satisfying port validation.
#[must_use]
pub fn provider_request(cancellation: CancellationToken) -> ProviderRequest {
    ProviderRequest {
        model: model_descriptor(),
        system_prompt: None,
        messages: ProviderMessages::new(Vec::new()).expect("messages"),
        tools: ProviderTools::new(Vec::new()).expect("tools"),
        tool_choice: ProviderToolChoice::Auto,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(4_096).expect("context"),
        output_tokens_max: TokenLimit::new(1_024).expect("output"),
        reasoning: ReasoningControls {
            enabled: true,
            effort: None,
            tier: None,
        },
        cancellation,
        deadline: ProviderDeadline::new(Duration::from_secs(1)).expect("deadline"),
    }
}

fn model_descriptor() -> ModelDescriptor {
    ModelDescriptor {
        handle: NonEmptyString::new("test/model").expect("handle"),
        provider_id: NonEmptyString::new("test").expect("provider"),
        available: true,
        context_window: Some(4_096),
        model_settings: None,
    }
}

/// Returns a deterministic UTC timestamp.
///
/// # Panics
/// Panics only if the canonical RFC 3339 fixture becomes invalid.
#[must_use]
pub fn timestamp() -> Timestamp {
    Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("timestamp")
}
