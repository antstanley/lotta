use lotta_domain::{AgentId, ConversationId, Timestamp};
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{Clock, IdGenerator};
use std::future::Future;

/// Runs parsing and deterministic initial-value clock assertions.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn clock_contract<F, Fut, Adapter>(factory: F, expected: Timestamp)
where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: Clock,
{
    let first = factory().await;
    let second = factory().await;
    assert_eq!(first.now(), expected);
    assert_eq!(first.now().to_string(), second.now().to_string());
    assert_eq!(
        first
            .parse_timestamp("2025-02-03T04:05:06.123456789Z")
            .expect("valid persisted timestamp")
            .to_string(),
        "2025-02-03T04:05:06.123456789Z"
    );
    assert_eq!(
        first
            .parse_timestamp("2025-02-03T05:35:06.123456789+01:30")
            .expect("offset timestamp")
            .to_string(),
        "2025-02-03T04:05:06.123456789Z"
    );
    assert!(first.parse_timestamp("not-a-timestamp").is_err());
}

/// Runs complete deterministic identifier traces against clean and boundary factories.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn id_generator_contract<F, Fut, M, MFut, Adapter>(factory: F, max_factory: M)
where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    M: Fn() -> MFut,
    MFut: Future<Output = Adapter>,
    Adapter: IdGenerator,
{
    let first = id_trace(&factory().await).await;
    let second = id_trace(&factory().await).await;
    assert_eq!(first, second);
    assert_eq!(first, expected_id_trace());
    assert_eq!(first.len(), 12);
    let mut unique = first.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), first.len());
    assert_id_exhaustion(&max_factory().await).await;
}

async fn id_trace(ids: &impl IdGenerator) -> Vec<String> {
    let mut values = Vec::with_capacity(12);
    for _ in 0..2 {
        let agent = ids.agent_id().await.expect("agent ID");
        let conversation = ids.conversation_id().await.expect("conversation ID");
        let message = ids.message_id().await.expect("message ID");
        let run = ids.run_id().await.expect("run ID");
        let incident = ids.incident_id().await.expect("incident ID");
        let owner = ids.turn_lifecycle_owner_id().await.expect("owner ID");
        assert_uuid_suffix(agent.as_str(), "agent-local-");
        assert!(AgentId::accept(agent.as_str()).is_ok());
        assert!(ConversationId::accept(conversation.as_str()).is_ok());
        assert!(lotta_domain::MessageId::accept(message.as_str()).is_ok());
        assert!(lotta_domain::RunId::accept(run.as_str()).is_ok());
        assert_uuid_v4(incident);
        assert_uuid_v4(owner);
        values.extend([
            agent.into_string(),
            conversation.into_string(),
            message.into_string(),
            run.into_string(),
            incident.to_string(),
            owner.to_string(),
        ]);
    }
    values
}

fn assert_uuid_suffix(value: &str, prefix: &str) {
    let uuid = uuid::Uuid::parse_str(value.strip_prefix(prefix).expect("canonical prefix"))
        .expect("canonical UUID suffix");
    assert_uuid_v4(uuid);
}

fn assert_uuid_v4(uuid: uuid::Uuid) {
    assert_eq!(uuid.get_version(), Some(uuid::Version::Random));
    assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
}

fn expected_id_trace() -> Vec<String> {
    [
        "agent-local-00000000-0000-4000-8000-000000000001",
        "local-conv-2",
        "ui-msg-3",
        "local-run-00000000-0000-4000-8000-000000000004",
        "00000000-0000-4000-8000-000000000005",
        "00000000-0000-4000-8000-000000000006",
        "agent-local-00000000-0000-4000-8000-000000000007",
        "local-conv-8",
        "ui-msg-9",
        "local-run-00000000-0000-4000-8000-00000000000a",
        "00000000-0000-4000-8000-00000000000b",
        "00000000-0000-4000-8000-00000000000c",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

async fn assert_id_exhaustion(ids: &impl IdGenerator) {
    assert_eq!(
        ids.conversation_id()
            .await
            .expect("last reservation")
            .as_str(),
        "local-conv-18446744073709551615"
    );
    for _ in 0..2 {
        assert!(matches!(
            ids.conversation_id().await,
            Err(RuntimeError::LimitExceeded { ref context })
                if context == "deterministic_id_sequence"
        ));
    }
}
