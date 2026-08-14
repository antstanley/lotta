//! Agent-scoped runtime identity.

use crate::{AgentId, ConversationId, NonEmptyString};
use serde::{Deserialize, Serialize};

/// Runtime identity keyed by the agent and conversation pair.
///
/// `acting_user_id` is attribution metadata and deliberately does not participate in identity,
/// equality, or hashing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeScope {
    /// Owning agent identifier.
    pub agent_id: AgentId,
    /// Conversation identifier, including the virtual agent-scoped `default` value.
    pub conversation_id: ConversationId,
    /// Optional cloud-attributed acting user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acting_user_id: Option<NonEmptyString>,
}

impl RuntimeScope {
    /// Creates a runtime scope with optional attribution metadata.
    ///
    /// # Panics
    /// Panics only if an identifier wrapper violates its non-empty invariant.
    #[must_use]
    pub fn new(
        agent_id: AgentId,
        conversation_id: ConversationId,
        acting_user_id: Option<NonEmptyString>,
    ) -> Self {
        assert!(
            !agent_id.as_str().is_empty(),
            "agent identifier invariant must hold"
        );
        assert!(
            !conversation_id.as_str().is_empty(),
            "conversation identifier invariant must hold"
        );
        Self {
            agent_id,
            conversation_id,
            acting_user_id,
        }
    }
}

impl PartialEq for RuntimeScope {
    fn eq(&self, other: &Self) -> bool {
        self.agent_id == other.agent_id && self.conversation_id == other.conversation_id
    }
}

impl Eq for RuntimeScope {}

impl std::hash::Hash for RuntimeScope {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.agent_id.hash(state);
        self.conversation_id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonschema::Validator;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    fn agent(value: &str) -> AgentId {
        match AgentId::accept(value) {
            Ok(id) => id,
            Err(error) => panic!("test agent ID must be non-empty: {error}"),
        }
    }

    fn acting_user(value: &str) -> NonEmptyString {
        match NonEmptyString::new(value) {
            Ok(user) => user,
            Err(error) => panic!("test user ID must be non-empty: {error}"),
        }
    }

    #[test]
    fn default_is_agent_scoped() {
        let first = RuntimeScope::new(
            agent("agent-local-a"),
            ConversationId::default_for_agent(),
            None,
        );
        let second = RuntimeScope::new(
            agent("agent-local-b"),
            ConversationId::default_for_agent(),
            None,
        );
        let mut scopes = HashMap::new();
        scopes.insert(first, "a");
        scopes.insert(second, "b");
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn acting_user_is_not_runtime_identity() {
        let base = RuntimeScope::new(
            agent("agent-local-a"),
            ConversationId::default_for_agent(),
            None,
        );
        let attributed = RuntimeScope::new(
            agent("agent-local-a"),
            ConversationId::default_for_agent(),
            Some(acting_user("cloud-user")),
        );
        assert_eq!(base, attributed);
    }

    #[test]
    fn schema_shape() {
        let schema: Value =
            match serde_json::from_str(include_str!("../../../.specs/canonical-types.schema.json"))
            {
                Ok(value) => value,
                Err(error) => panic!("canonical schema must parse: {error}"),
            };
        let validator = compile_runtime_scope(&schema);
        let scope = RuntimeScope::new(
            agent("agent-local-a"),
            ConversationId::default_for_agent(),
            Some(acting_user("cloud-user")),
        );
        let serialized = match serde_json::to_value(scope) {
            Ok(value) => value,
            Err(error) => panic!("runtime scope must serialize: {error}"),
        };
        assert!(validator.is_valid(&serialized));
        assert_eq!(
            serialized,
            json!({
                "agent_id": "agent-local-a",
                "conversation_id": "default",
                "acting_user_id": "cloud-user"
            })
        );
    }

    fn compile_runtime_scope(schema: &Value) -> Validator {
        assert!(schema.pointer("/$defs/RuntimeScope").is_some());
        let reference = json!({"$ref": "#/$defs/RuntimeScope"});
        let mut rooted = schema.clone();
        if let Value::Object(root) = &mut rooted {
            root.insert("allOf".into(), json!([reference]));
        }
        match jsonschema::validator_for(&rooted) {
            Ok(validator) => validator,
            Err(error) => panic!("canonical RuntimeScope schema must compile: {error}"),
        }
    }
}
