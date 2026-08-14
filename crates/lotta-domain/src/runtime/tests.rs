use super::*;
use crate::DomainError;
use crate::{AgentId, BoundedJsonValue, ConversationId, Timestamp};
use jsonschema::Validator;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fmt::Debug};

fn run_id() -> RunId {
    RunId::generate_sequence(1).unwrap_or_else(|error| panic!("test run ID: {error}"))
}

fn stop_reason(value: &str) -> StopReason {
    StopReason::new(value).unwrap_or_else(|error| panic!("test stop reason: {error}"))
}

fn owner_in(kind: TurnStateKind) -> (TurnLifecycle, Option<TurnLease>) {
    let mut owner = TurnLifecycle::new();
    let lease = match kind {
        TurnStateKind::Idle => None,
        TurnStateKind::Command => Some(owner.start_command().expect("command starts")),
        TurnStateKind::Active => Some(
            owner
                .start_turn("turn-1".into(), run_id())
                .expect("turn starts"),
        ),
        TurnStateKind::Cancelling => {
            let lease = owner
                .start_turn("turn-1".into(), run_id())
                .expect("turn starts");
            owner
                .request_cancellation(&lease)
                .expect("cancellation starts");
            Some(lease)
        }
    };
    (owner, lease)
}

fn attempt(from: TurnStateKind, to: TurnStateKind) -> bool {
    let (mut owner, lease) = owner_in(from);
    match to {
        TurnStateKind::Idle if from == TurnStateKind::Command => owner
            .finish_command(lease.as_ref().expect("occupied lease"))
            .is_ok(),
        TurnStateKind::Idle if lease.is_some() => owner
            .finish_turn(
                lease.as_ref().expect("occupied lease"),
                stop_reason("complete"),
            )
            .is_ok(),
        TurnStateKind::Command => owner.start_command().is_ok(),
        TurnStateKind::Active => owner.start_turn("turn-1".into(), run_id()).is_ok(),
        TurnStateKind::Cancelling if lease.is_some() => owner
            .request_cancellation(lease.as_ref().expect("occupied lease"))
            .is_ok(),
        TurnStateKind::Idle | TurnStateKind::Cancelling => false,
    }
}

pub(super) fn assert_turn_state_exhaustive_transition_matrix() {
    let kinds = [
        TurnStateKind::Idle,
        TurnStateKind::Command,
        TurnStateKind::Active,
        TurnStateKind::Cancelling,
    ];
    let legal = [
        (TurnStateKind::Idle, TurnStateKind::Command),
        (TurnStateKind::Idle, TurnStateKind::Active),
        (TurnStateKind::Command, TurnStateKind::Idle),
        (TurnStateKind::Active, TurnStateKind::Idle),
        (TurnStateKind::Active, TurnStateKind::Cancelling),
        (TurnStateKind::Cancelling, TurnStateKind::Idle),
    ];
    let mut pairs = 0;
    for from in kinds {
        for to in kinds {
            if from == to {
                continue;
            }
            assert_eq!(attempt(from, to), legal.contains(&(from, to)));
            pairs += 1;
        }
    }
    assert_eq!(pairs, 12);
}

pub(super) fn assert_turn_state_idle_command_idle() {
    let mut owner = TurnLifecycle::new();
    let lease = owner.start_command().expect("legal transition");
    owner.finish_command(&lease).expect("current lease");
    assert_eq!(owner.state().kind(), TurnStateKind::Idle);
}

pub(super) fn assert_turn_state_idle_active_idle() {
    let mut owner = TurnLifecycle::new();
    let lease = owner
        .start_turn("turn-1".into(), run_id())
        .expect("legal transition");
    owner
        .finish_turn(&lease, stop_reason("complete"))
        .expect("current lease");
    assert_eq!(owner.state().kind(), TurnStateKind::Idle);
}

pub(super) fn assert_turn_state_active_cancelling_idle() {
    let mut owner = TurnLifecycle::new();
    let lease = owner
        .start_turn("turn-1".into(), run_id())
        .expect("legal transition");
    owner.request_cancellation(&lease).expect("current lease");
    owner
        .finish_turn(&lease, stop_reason("cancelled"))
        .expect("current lease");
    assert_eq!(owner.state().kind(), TurnStateKind::Idle);
}

pub(super) fn assert_turn_state_idle_cancelling_fails() {
    let mut owner = TurnLifecycle::new();
    let mut other = TurnLifecycle::new();
    let lease = other.start_command().expect("lease");
    assert!(owner.request_cancellation(&lease).is_err());
    assert_eq!(owner.state().kind(), TurnStateKind::Idle);
}

pub(super) fn assert_turn_state_command_active_fails() {
    let mut owner = TurnLifecycle::new();
    owner.start_command().expect("command");
    assert!(owner.start_turn("turn-1".into(), run_id()).is_err());
    assert_eq!(owner.state().kind(), TurnStateKind::Command);
}

#[test]
fn turn_state_kind_wire_names_and_projections() {
    for (kind, spelling) in [
        TurnStateKind::Idle,
        TurnStateKind::Command,
        TurnStateKind::Active,
        TurnStateKind::Cancelling,
    ]
    .into_iter()
    .zip(["idle", "command", "active", "cancelling"])
    {
        assert_eq!(serde_json::to_value(kind).expect("encode"), json!(spelling));
    }
    let mut owner = TurnLifecycle::new();
    assert!(!owner.state().is_processing());
    owner.start_turn("turn-1".into(), run_id()).expect("turn");
    assert_eq!(owner.state().loop_status(), LoopStatus::SendingApiRequest);
    assert_eq!(owner.state().active_run_ids(), &[run_id()]);
}

#[test]
fn stale_and_cross_owner_leases_preserve_exact_state() {
    let mut owner = TurnLifecycle::new();
    let stale = owner.start_command().expect("generation one");
    owner.finish_command(&stale).expect("settle command");
    let current = owner
        .start_turn("turn-2".into(), run_id())
        .expect("generation two");
    let before_kind = owner.state().kind();
    let before_runs = owner.state().active_run_ids().to_vec();
    assert_eq!(
        owner.request_cancellation(&stale),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), before_kind);
    assert_eq!(owner.state().active_run_ids(), before_runs);
    assert_eq!(owner.last_stop_reason(), None);

    let mut other = TurnLifecycle::new();
    let cross_owner_same_generation = other
        .start_turn("other".into(), run_id())
        .expect("generation one");
    assert_eq!(
        owner.finish_turn(&cross_owner_same_generation, stop_reason("wrong")),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), before_kind);
    assert_eq!(owner.state().active_run_ids(), before_runs);
    assert_eq!(owner.last_stop_reason(), None);
    owner
        .finish_turn(&current, stop_reason("complete"))
        .expect("current lease succeeds");
}

#[test]
fn lease_checked_settlements_preserve_state_on_failure() {
    let mut owner = TurnLifecycle::new();
    let current = owner.start_command().expect("command");
    let mut other = TurnLifecycle::new();
    let wrong = other.start_command().expect("cross-owner same generation");
    assert_eq!(
        owner.finish_command(&wrong),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), TurnStateKind::Command);
    owner.finish_command(&current).expect("current command");

    let current = owner.start_turn("turn".into(), run_id()).expect("active");
    assert_eq!(
        owner.request_cancellation(&wrong),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), TurnStateKind::Active);
    owner
        .request_cancellation(&current)
        .expect("current cancel");
    assert_eq!(
        owner.finish_turn(&wrong, stop_reason("wrong")),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), TurnStateKind::Cancelling);
    assert_eq!(owner.last_stop_reason(), None);
}

#[test]
fn lease_generation_exhaustion_is_typed_and_atomic() {
    let mut owner = TurnLifecycle::new();
    owner.test_set_generation(u64::MAX - 1);
    let last = owner.start_command().expect("last generation is valid");
    assert_eq!(owner.test_generation(), u64::MAX);
    assert_eq!(owner.state().kind(), TurnStateKind::Command);
    owner.finish_command(&last).expect("last lease settles");

    let before_generation = owner.test_generation();
    let before_kind = owner.state().kind();
    let before_processing = owner.state().is_processing();
    let before_status = owner.state().loop_status();
    let before_runs = owner.state().active_run_ids().to_vec();
    let before_reason = owner.last_stop_reason().cloned();
    assert_eq!(
        owner.start_command(),
        Err(DomainError::TurnLeaseGenerationExhausted)
    );
    assert_eq!(owner.test_generation(), before_generation);
    assert_eq!(owner.state().kind(), before_kind);
    assert_eq!(owner.state().is_processing(), before_processing);
    assert_eq!(owner.state().loop_status(), before_status);
    assert_eq!(owner.state().active_run_ids(), before_runs);
    assert_eq!(owner.last_stop_reason(), before_reason.as_ref());

    assert_eq!(
        owner.start_turn("unallocated".into(), run_id()),
        Err(DomainError::TurnLeaseGenerationExhausted)
    );
    assert_eq!(owner.test_generation(), before_generation);
    assert_eq!(owner.state().kind(), before_kind);
    assert_eq!(owner.state().is_processing(), before_processing);
    assert_eq!(owner.state().loop_status(), before_status);
    assert_eq!(owner.state().active_run_ids(), before_runs);
    assert_eq!(owner.last_stop_reason(), before_reason.as_ref());
}

#[test]
fn empty_turn_id_is_typed_and_atomic() {
    let mut owner = TurnLifecycle::new();
    let lease = owner.start_turn("valid".into(), run_id()).expect("active");
    owner
        .finish_turn(&lease, stop_reason("complete"))
        .expect("settled");
    let generation = owner.test_generation();

    assert_eq!(
        owner.start_turn(String::new(), run_id()),
        Err(DomainError::EmptyTurnId)
    );
    assert_eq!(owner.test_generation(), generation);
    assert_eq!(owner.state().kind(), TurnStateKind::Idle);
    assert_eq!(
        owner.last_stop_reason().map(StopReason::as_str),
        Some("complete")
    );
}

#[test]
fn last_stop_reason_is_terminal_and_lease_checked() {
    let mut owner = TurnLifecycle::new();
    let first = owner.start_turn("first".into(), run_id()).expect("first");
    owner
        .finish_turn(&first, stop_reason("complete"))
        .expect("settle first");
    assert_eq!(
        owner.last_stop_reason().map(StopReason::as_str),
        Some("complete")
    );
    let second = owner.start_turn("second".into(), run_id()).expect("second");
    assert_eq!(owner.last_stop_reason(), None);
    assert_eq!(
        owner.finish_turn(&first, stop_reason("stale")),
        Err(DomainError::StaleTurnLease)
    );
    assert_eq!(owner.state().kind(), TurnStateKind::Active);
    assert_eq!(owner.last_stop_reason(), None);
    owner
        .finish_turn(&second, stop_reason("cancelled"))
        .expect("settle second");
    assert_eq!(
        owner.last_stop_reason().map(StopReason::as_str),
        Some("cancelled")
    );
    assert!(StopReason::new("").is_err());
}

pub(super) fn assert_approval_keeps_active() {
    let approval = approval_sample(false);
    assert_eq!(approval.subtype, ApprovalSubtype::CanUseTool);
    let mut owner = TurnLifecycle::new();
    owner
        .start_turn("turn-1".into(), run_id())
        .expect("active turn");
    assert_eq!(owner.state().kind(), TurnStateKind::Active);
    assert!(owner.state().is_processing());
}

pub(super) fn assert_duplicate_client_message_id() {
    let id = non_empty("cm-1");
    let mut history = AdmissionHistory::default();
    assert_eq!(
        history.admit(&id, InputDisposition::Started),
        InputDisposition::Started
    );
    assert_eq!(
        history.admit(&id, InputDisposition::Rejected),
        InputDisposition::Started
    );
    assert_eq!(history.admission_count(), 1);
}

pub(super) fn assert_queue_item_wire_names() {
    for (value, spelling) in [
        (
            serde_json::to_value(QueueRemovalDisposition::Dequeued),
            "dequeued",
        ),
        (
            serde_json::to_value(QueueRemovalDisposition::Cancelled),
            "cancelled",
        ),
        (
            serde_json::to_value(QueueDropReason::BufferLimit),
            "buffer_limit",
        ),
        (
            serde_json::to_value(QueueDropReason::StaleGeneration),
            "stale_generation",
        ),
    ] {
        let encoded = value.unwrap_or_else(|error| panic!("queue enum encode: {error}"));
        assert_eq!(encoded, json!(spelling));
    }
    assert_ne!(
        std::any::TypeId::of::<QueueRemovalDisposition>(),
        std::any::TypeId::of::<QueueDropReason>()
    );
}

#[test]
fn conversation_archival_round_trip() {
    let archived = ConversationArchiveState::Active.archive();
    assert_eq!(archived, ConversationArchiveState::Archived);
    assert_eq!(archived.unarchive(), ConversationArchiveState::Active);
}

fn schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../../.specs/canonical-types.schema.json"
    ))
    .unwrap_or_else(|error| panic!("schema: {error}"))
}

fn validator(name: &str) -> Validator {
    let root = schema();
    let mut definition = root["$defs"][name].clone();
    if let Value::Object(map) = &mut definition {
        map.insert("$defs".into(), root["$defs"].clone());
    }
    jsonschema::validator_for(&definition).unwrap_or_else(|error| panic!("validator: {error}"))
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn check_schema<T>(name: &str, minimal: Value, complete: Value)
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    let definition = schema()["$defs"][name].clone();
    let required: BTreeSet<String> = definition["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let properties: BTreeSet<String> = definition["properties"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys().cloned())
        .collect();
    assert_eq!(keys(&minimal), required, "{name} minimal");
    assert_eq!(keys(&complete), properties, "{name} complete");
    for sample in [minimal, complete] {
        assert!(validator(name).is_valid(&sample), "{name}: {sample}");
        let decoded: T = serde_json::from_value(sample.clone())
            .unwrap_or_else(|error| panic!("{name} decode: {error}"));
        let encoded =
            serde_json::to_value(&decoded).unwrap_or_else(|error| panic!("{name} encode: {error}"));
        assert!(validator(name).is_valid(&encoded), "{name}: {encoded}");
        let again: T = serde_json::from_value(encoded)
            .unwrap_or_else(|error| panic!("{name} re-decode: {error}"));
        assert_eq!(decoded, again);
    }
}

#[test]
fn schema_conformance_runtime_entities() {
    check_schema::<ApprovalRequest>(
        "ApprovalRequest",
        json!({"request_id":"req","subtype":"can_use_tool","tool_name":"read",
            "input":{},"tool_call_id":"call","permission_suggestions":[],"blocked_path":null}),
        json!({"request_id":"req","subtype":"can_use_tool","tool_name":"read",
            "input":{"path":"x"},"tool_call_id":"call","permission_suggestions":[{}],
            "blocked_path":"x","diffs":[{}]}),
    );
    check_schema::<RuntimeConnection>(
        "RuntimeConnection",
        json!({"id":"conn","ordinal":0,"initialized":false,"subscriptions":[],"event_seq":0}),
        json!({"id":"conn","ordinal":1,"initialized":true,
            "subscriptions":[scope_json()],"event_seq":1}),
    );
    check_schema::<ConversationRuntimeSnapshot>(
        "ConversationRuntimeSnapshot",
        json!({"runtime":scope_json(),"turn_state":"idle","queue":[],
            "permission_mode":"standard"}),
        json!({"runtime":scope_json(),"turn_state":"active","queue":[queue_json()],
            "permission_mode":"acceptEdits","working_directory":null,
            "active_run_ids":["local-run-1"]}),
    );
    check_schema::<QueueItem>("QueueItem", queue_json(), queue_json());
    check_schema::<ExternalToolRegistration>(
        "ExternalToolRegistration",
        json!({"name":"tool","description":"does work","parameters":{}}),
        json!({"name":"tool","label":"Tool","description":"does work",
            "parameters":{"type":"object"}}),
    );
}

#[test]
fn schema_runtime_enum_spellings_and_bounds() {
    let runtime = validator("ConversationRuntimeSnapshot");
    for state in ["idle", "command", "active", "cancelling"] {
        let sample = json!({"runtime":scope_json(),"turn_state":state,"queue":[],
            "permission_mode":"strict"});
        assert!(runtime.is_valid(&sample));
    }
    let mut queue = Vec::new();
    for _ in 0..=QUEUE_ITEMS_HARD_MAX.value {
        queue.push(queue_json());
    }
    let over = json!({"runtime":scope_json(),"turn_state":"idle","queue":queue,
        "permission_mode":"standard"});
    assert!(!runtime.is_valid(&over));
    assert!(serde_json::from_value::<ConversationRuntimeSnapshot>(over).is_err());
}

fn non_empty(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap_or_else(|error| panic!("test string: {error}"))
}

fn scope_json() -> Value {
    json!({"agent_id":"agent-local-test","conversation_id":"default"})
}

fn queue_json() -> Value {
    json!({"id":"item","client_message_id":"cm","kind":"message","source":"user",
        "content":"hello","enqueued_at":"2026-08-14T12:34:56Z"})
}

fn approval_sample(with_diffs: bool) -> ApprovalRequest {
    let value = if with_diffs {
        json!({"request_id":"req","subtype":"can_use_tool","tool_name":"read",
            "input":{},"tool_call_id":"call","permission_suggestions":[],
            "blocked_path":null,"diffs":[]})
    } else {
        json!({"request_id":"req","subtype":"can_use_tool","tool_name":"read",
            "input":{},"tool_call_id":"call","permission_suggestions":[],"blocked_path":null})
    };
    serde_json::from_value(value).unwrap_or_else(|error| panic!("approval sample: {error}"))
}

const _: fn(AgentId, ConversationId, Timestamp, BoundedJsonValue) =
    |agent, conversation, timestamp, value| drop((agent, conversation, timestamp, value));
