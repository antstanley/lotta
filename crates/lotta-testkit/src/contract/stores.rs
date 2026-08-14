use crate::contract::common::assert_pending_once;
use lotta_domain::{
    Agent, AgentId, Conversation, ConversationId, TranscriptEntry, TranscriptManifest,
};
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{AgentStore, ConversationStore, TranscriptItem, TranscriptStore};
use serde_json::json;
use std::future::Future;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Runs complete agent-store semantics against isolated clean adapters.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn agent_store_contract<F, Fut, Adapter>(factory: F, first: Agent, second: Agent)
where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: AgentStore,
{
    let store = factory().await;
    assert_not_found(&store.load(&first.id).await);
    store.save(&first).await.expect("save exact agent");
    assert_eq!(
        store.load(&first.id).await.expect("load exact agent"),
        first
    );
    let replacement = replacement_agent(&first);
    store.save(&replacement).await.expect("replace agent");
    store.save(&second).await.expect("save second agent");
    assert_eq!(
        collect_agents(&store, CancellationToken::new()).await,
        sorted_agents(&replacement, &second)
    );
    assert_agent_stream_semantics(&store).await;
    store.delete(&first.id).await.expect("delete exact agent");
    assert_not_found(&store.load(&first.id).await);
    assert_not_found(&store.delete(&first.id).await);
}

/// Runs complete scoped conversation-store semantics against clean adapters.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn conversation_store_contract<F, Fut, Adapter>(
    factory: F,
    first: Conversation,
    second: Conversation,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: ConversationStore,
{
    let store = factory().await;
    assert_not_found(&store.load(&first.agent_id, &first.id).await);
    store.save(&first).await.expect("save exact conversation");
    assert_eq!(
        store
            .load(&first.agent_id, &first.id)
            .await
            .expect("load exact conversation"),
        first
    );
    let replacement = replacement_conversation(&first);
    store
        .save(&replacement)
        .await
        .expect("replace conversation");
    store.save(&second).await.expect("save second conversation");
    assert_eq!(
        collect_conversations(&store, &first.agent_id).await,
        sorted_conversations(&replacement, &second)
    );
    let other = AgentId::accept("agent-other").expect("other agent");
    let scoped = rehome_conversation(&first, &other);
    store
        .save(&scoped)
        .await
        .expect("save same ID in other scope");
    assert_eq!(
        store
            .load(&other, &first.id)
            .await
            .expect("load other scope"),
        scoped
    );
    assert_not_found(&store.delete(&other, &second.id).await);
    assert_eq!(
        store
            .load(&first.agent_id, &second.id)
            .await
            .expect("correct scope survives"),
        second
    );
    assert_conversation_stream_semantics(&store, &first.agent_id).await;
    store
        .delete(&first.agent_id, &first.id)
        .await
        .expect("delete conversation");
    assert_not_found(&store.load(&first.agent_id, &first.id).await);
    assert_not_found(&store.delete(&first.agent_id, &first.id).await);
    assert_eq!(
        store
            .load(&other, &first.id)
            .await
            .expect("other scope survives"),
        scoped
    );
}

/// Runs complete transcript initialization, ordering, scoping, and stream semantics.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn transcript_store_contract<F, Fut, Adapter>(
    factory: F,
    agent_id: AgentId,
    conversation_id: ConversationId,
    manifest: TranscriptManifest,
    session: TranscriptEntry,
    appended: TranscriptEntry,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: TranscriptStore,
{
    let store = factory().await;
    assert_transcript_missing(&store, &agent_id, &conversation_id).await;
    assert_not_found(&store.append(&agent_id, &conversation_id, &appended).await);
    store
        .initialize(&agent_id, &conversation_id, &manifest, &session)
        .await
        .expect("initialize transcript");
    assert_eq!(
        collect_transcript(&store, &agent_id, &conversation_id).await,
        vec![
            TranscriptItem::Manifest(manifest.clone()),
            TranscriptItem::Entry(session.clone())
        ]
    );
    let conflict = store
        .initialize(&agent_id, &conversation_id, &manifest, &appended)
        .await;
    assert!(matches!(conflict, Err(RuntimeError::Conflict { .. })));
    assert_eq!(
        collect_transcript(&store, &agent_id, &conversation_id)
            .await
            .len(),
        2
    );
    store
        .append(&agent_id, &conversation_id, &appended)
        .await
        .expect("append first entry");
    store
        .append(&agent_id, &conversation_id, &session)
        .await
        .expect("append second entry");
    let expected = vec![
        TranscriptItem::Manifest(manifest),
        TranscriptItem::Entry(session.clone()),
        TranscriptItem::Entry(appended),
        TranscriptItem::Entry(session),
    ];
    assert_eq!(
        collect_transcript(&store, &agent_id, &conversation_id).await,
        expected
    );
    assert_transcript_stream_semantics(&store, &agent_id, &conversation_id).await;
    let other_agent = AgentId::accept("agent-other").expect("other agent");
    let other_conversation =
        ConversationId::accept("conversation-other").expect("other conversation");
    assert_transcript_missing(&store, &other_agent, &conversation_id).await;
    assert_transcript_missing(&store, &agent_id, &other_conversation).await;
}

fn assert_not_found<T>(result: &Result<T, RuntimeError>) {
    assert!(matches!(result, Err(RuntimeError::NotFound { .. })));
}

fn replacement_agent(agent: &Agent) -> Agent {
    let mut value = serde_json::to_value(agent).expect("serialize agent");
    value["name"] = json!("replacement-agent");
    value["hidden"] = json!(true);
    serde_json::from_value(value).expect("replacement agent")
}

fn replacement_conversation(conversation: &Conversation) -> Conversation {
    let mut value = serde_json::to_value(conversation).expect("serialize conversation");
    value["archived"] = json!(true);
    value["updated_at"] = json!("2026-08-15T00:00:00Z");
    serde_json::from_value(value).expect("replacement conversation")
}

fn rehome_conversation(conversation: &Conversation, agent: &AgentId) -> Conversation {
    let mut value = serde_json::to_value(conversation).expect("serialize conversation");
    value["agent_id"] = json!(agent.as_str());
    serde_json::from_value(value).expect("rehome conversation")
}

fn sorted_agents(first: &Agent, second: &Agent) -> Vec<Agent> {
    let mut values = vec![first.clone(), second.clone()];
    values.sort_by(|left, right| left.id.cmp(&right.id));
    values
}

fn sorted_conversations(first: &Conversation, second: &Conversation) -> Vec<Conversation> {
    let mut values = vec![first.clone(), second.clone()];
    values.sort_by(|left, right| left.id.cmp(&right.id));
    values
}

async fn assert_transcript_missing(
    store: &impl TranscriptStore,
    agent: &AgentId,
    conversation: &ConversationId,
) {
    let (sender, _receiver) = mpsc::channel(1);
    assert_not_found(
        &store
            .load(agent, conversation, sender, CancellationToken::new())
            .await,
    );
}

macro_rules! assert_stream {
    ($stream:expr, $closed:expr) => {{
        let (sender, mut receiver) = mpsc::channel(1);
        let token = CancellationToken::new();
        let stream = $stream(sender, token.clone());
        tokio::pin!(stream);
        assert_pending_once(stream.as_mut()).await;
        token.cancel();
        assert!(matches!(stream.await, Err(RuntimeError::Cancelled { .. })));
        assert!(receiver.recv().await.is_some());
        assert!(receiver.recv().await.is_none());
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        assert!(matches!(
            $closed(sender).await,
            Err(RuntimeError::AdapterFailure {
                code: "testkit_channel_closed",
                ..
            })
        ));
    }};
}

async fn assert_agent_stream_semantics(store: &impl AgentStore) {
    assert_stream!(|sender, token| store.list(sender, token), |sender| store
        .list(sender, CancellationToken::new()));
}

async fn assert_conversation_stream_semantics(store: &impl ConversationStore, agent: &AgentId) {
    assert_stream!(
        |sender, token| store.list_for_agent(agent, sender, token),
        |sender| store.list_for_agent(agent, sender, CancellationToken::new())
    );
}

async fn assert_transcript_stream_semantics(
    store: &impl TranscriptStore,
    agent: &AgentId,
    conversation: &ConversationId,
) {
    assert_stream!(
        |sender, token| store.load(agent, conversation, sender, token),
        |sender| store.load(agent, conversation, sender, CancellationToken::new())
    );
}

async fn collect_conversations(
    store: &impl ConversationStore,
    agent: &AgentId,
) -> Vec<Conversation> {
    let (sender, mut receiver) = mpsc::channel(1);
    let listing = store.list_for_agent(agent, sender, CancellationToken::new());
    tokio::pin!(listing);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut listing => {
                result.expect("conversation listing");
                while let Some(value) = receiver.recv().await {
                    values.push(value);
                }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}

async fn collect_agents(store: &impl AgentStore, cancellation: CancellationToken) -> Vec<Agent> {
    let (sender, mut receiver) = mpsc::channel(1);
    let listing = store.list(sender, cancellation);
    tokio::pin!(listing);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut listing => {
                result.expect("agent listing");
                while let Some(value) = receiver.recv().await { values.push(value); }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}

async fn collect_transcript(
    store: &impl TranscriptStore,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Vec<TranscriptItem> {
    let (sender, mut receiver) = mpsc::channel(1);
    let loading = store.load(agent, conversation, sender, CancellationToken::new());
    tokio::pin!(loading);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut loading => {
                result.expect("transcript load");
                while let Some(value) = receiver.recv().await { values.push(value); }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}
