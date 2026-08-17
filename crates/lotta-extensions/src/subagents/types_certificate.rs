use super::types::*;
use lotta_domain::{AgentId, ConversationId};
use std::path::PathBuf;

fn parent() -> ParentScope {
    ParentScope {
        agent_id: AgentId::accept("parent").unwrap(),
        conversation_id: ConversationId::accept("conversation").unwrap(),
        runtime_id: "runtime".into(),
    }
}

pub(super) fn request_for_process_test() -> SubagentRequest {
    request(SubagentType::GeneralPurpose)
}

fn request(kind: SubagentType) -> SubagentRequest {
    let memory = matches!(
        kind,
        SubagentType::Reflection
            | SubagentType::Memory
            | SubagentType::HistoryAnalyzer
            | SubagentType::Init
    );
    SubagentRequest {
        subagent_type: kind,
        description: "description".into(),
        prompt: "prompt".into(),
        resolved_context: None,
        model: ModelPolicy::Inherit("parent-model".into()),
        background: false,
        silent: false,
        filesystem_roots: Vec::new(),
        reflection_worktree: None,
        max_turns: Some(10),
        tools: match kind {
            SubagentType::Recall => recall_tools(),
            SubagentType::Reflection => reflection_tools(),
            _ => ToolPolicy::All,
        },
        memory_scope: memory.then(|| MemoryScope {
            primary_root: PathBuf::from("/primary"),
            readonly_roots: Vec::new(),
            writable_roots: vec![PathBuf::from("/worktree")],
        }),
        parent_scope: parent(),
        existing_agent_id: None,
        existing_conversation_id: None,
    }
}

macro_rules! type_case {
    ($name:ident, $kind:expr) => {
        #[test]
        fn $name() {
            let value = request($kind);
            assert!(value.validate().is_ok());
            let wire = serde_json::to_value(&value).unwrap();
            let roundtrip: SubagentRequest = serde_json::from_value(wire).unwrap();
            assert_eq!(roundtrip.subagent_type, $kind);
        }
    };
}

type_case!(
    general_purpose_request_is_real,
    SubagentType::GeneralPurpose
);
type_case!(fork_request_is_real, SubagentType::Fork);
type_case!(recall_request_is_real, SubagentType::Recall);
type_case!(reflection_request_is_real, SubagentType::Reflection);
type_case!(memory_request_is_real, SubagentType::Memory);
type_case!(
    history_analyzer_request_is_real,
    SubagentType::HistoryAnalyzer
);
type_case!(init_request_is_real, SubagentType::Init);

#[test]
fn general_purpose_starts_isolated() {
    assert_eq!(
        request(SubagentType::GeneralPurpose)
            .context_plan(None)
            .unwrap(),
        ContextPlan::Isolated
    );
}

#[test]
fn fork_serializes_parent_conversation_and_scope() {
    let plan = request(SubagentType::Fork)
        .context_plan(Some(br#"{"messages":["parent"]}"#.to_vec()))
        .unwrap();
    let ContextPlan::Fork(context) = plan else {
        panic!("fork plan")
    };
    assert!(
        context
            .conversation_json
            .windows(6)
            .any(|bytes| bytes == b"parent")
    );
    assert_eq!(context.parent_scope, parent());
}

#[test]
fn recall_has_exact_read_only_parent_capability() {
    assert_eq!(
        request(SubagentType::Recall).context_plan(None).unwrap(),
        ContextPlan::Recall(parent())
    );
    let mut invalid = request(SubagentType::Recall);
    invalid.tools = ToolPolicy::All;
    assert_eq!(invalid.validate(), Err(RequestError::Combination));
}

#[test]
fn reflection_requires_worktree_memory_scope() {
    assert_eq!(
        request(SubagentType::Reflection)
            .context_plan(None)
            .unwrap(),
        ContextPlan::Reflection
    );
    let mut invalid = request(SubagentType::Reflection);
    invalid.memory_scope = None;
    assert_eq!(invalid.validate(), Err(RequestError::Combination));
}

#[test]
fn existing_deploy_is_general_purpose_only() {
    let mut general = request(SubagentType::GeneralPurpose);
    general.existing_agent_id = Some(AgentId::accept("existing").unwrap());
    assert!(matches!(
        general.context_plan(None),
        Ok(ContextPlan::Existing { .. })
    ));
    let mut fork = request(SubagentType::Fork);
    fork.existing_agent_id = Some(AgentId::accept("existing").unwrap());
    assert_eq!(fork.validate(), Err(RequestError::Combination));
}

#[test]
fn request_bounds_reject_at_above_and_malformed() {
    let mut at = request(SubagentType::GeneralPurpose);
    at.description = "d".repeat(SUBAGENT_DESCRIPTION_BYTES_MAX);
    assert!(at.validate().is_ok());
    at.description.push('d');
    assert_eq!(at.validate(), Err(RequestError::Bound));
    assert!(
        serde_json::from_value::<SubagentRequest>(serde_json::json!({"unknown": true})).is_err()
    );
}
