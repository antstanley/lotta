#![allow(clippy::option_option)] // Three-state schema fields require absent/null/value.

use super::common::{EntityExtras, serialize_with_extras};
use super::{
    BoundedJsonValue, BoundedMap, BoundedVec, STRING_ITEMS_MAX, UNBOUNDED_COLLECTION_ITEMS_MAX,
    UNBOUNDED_MAP_FIELDS_MAX,
};
use crate::{AgentId, ConversationId, MessageId, NonEmptyString, RunId, Timestamp};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const AGENT_FIELDS: &[&str] = &[
    "id",
    "name",
    "description",
    "system",
    "tags",
    "model",
    "model_settings",
    "hidden",
    "compaction_settings",
];
const CONVERSATION_FIELDS: &[&str] = &[
    "id",
    "agent_id",
    "archived",
    "archived_at",
    "created_at",
    "updated_at",
    "last_message_at",
    "summary",
    "in_context_message_ids",
    "model",
    "model_settings",
    "context_window_limit",
    "hidden",
    "tags",
];

/// Agent-creation memory file input.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryBlockInput {
    /// Memory file label.
    pub label: NonEmptyString,
    /// Memory file contents.
    pub value: String,
    /// Optional description; explicit null remains distinct from absence.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
}

/// Persisted local agent definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    /// Agent identifier.
    pub id: AgentId,
    /// Display name.
    pub name: NonEmptyString,
    /// Optional description with explicit-null preservation.
    pub description: Option<Option<String>>,
    /// System prompt source.
    pub system: String,
    /// Tags.
    pub tags: BoundedVec<String, STRING_ITEMS_MAX>,
    /// Model handle.
    pub model: NonEmptyString,
    /// Provider model settings.
    pub model_settings: BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    /// Optional hidden flag with explicit-null preservation.
    pub hidden: Option<Option<bool>>,
    /// Optional compaction settings with explicit-null preservation.
    pub compaction_settings: Option<Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>>,
    /// Compatible fields unknown to this version.
    pub extras: EntityExtras,
}

#[derive(Deserialize, Serialize)]
struct AgentKnown {
    id: AgentId,
    name: NonEmptyString,
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    description: Option<Option<String>>,
    system: String,
    tags: BoundedVec<String, STRING_ITEMS_MAX>,
    model: NonEmptyString,
    model_settings: BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    hidden: Option<Option<bool>>,
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    compaction_settings: Option<Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>>,
    #[serde(flatten)]
    extras: EntityExtras,
}

impl From<AgentKnown> for Agent {
    fn from(value: AgentKnown) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            system: value.system,
            tags: value.tags,
            model: value.model,
            model_settings: value.model_settings,
            hidden: value.hidden,
            compaction_settings: value.compaction_settings,
            extras: value.extras,
        }
    }
}

impl From<&Agent> for AgentKnown {
    fn from(value: &Agent) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            description: value.description.clone(),
            system: value.system.clone(),
            tags: value.tags.clone(),
            model: value.model.clone(),
            model_settings: value.model_settings.clone(),
            hidden: value.hidden,
            compaction_settings: value.compaction_settings.clone(),
            extras: EntityExtras::default(),
        }
    }
}

impl Serialize for Agent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_with_extras(
            &AgentKnown::from(self),
            &self.extras,
            AGENT_FIELDS,
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for Agent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        AgentKnown::deserialize(deserializer).map(Into::into)
    }
}

/// Persisted agent conversation.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Conversation {
    /// Conversation identifier.
    pub id: ConversationId,
    /// Owning agent.
    pub agent_id: AgentId,
    /// Archive state.
    pub archived: bool,
    /// Optional archive timestamp with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub archived_at: Option<Option<Timestamp>>,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Update timestamp.
    pub updated_at: Timestamp,
    /// Optional latest-message timestamp with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_message_at: Option<Option<Timestamp>>,
    /// Optional summary with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub summary: Option<Option<String>>,
    /// Message IDs retained in context.
    pub in_context_message_ids: BoundedVec<MessageId, UNBOUNDED_COLLECTION_ITEMS_MAX>,
    /// Optional model override with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<String>>,
    /// Optional model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
    /// Optional positive context-window limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_limit: Option<u64>,
    /// Optional hidden state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Optional tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<BoundedVec<String, STRING_ITEMS_MAX>>,
    /// Compatible fields unknown to this version.
    #[serde(flatten)]
    pub extras: EntityExtras,
}

/// Provider-facing local message role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LocalMessageRole {
    /// User input.
    #[serde(rename = "user")]
    User,
    /// Assistant output.
    #[serde(rename = "assistant")]
    Assistant,
    /// Tool result.
    #[serde(rename = "toolResult")]
    ToolResult,
}

/// Provider-facing message retained in transcript entries.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LocalMessage {
    /// Message identifier.
    pub id: MessageId,
    /// Message role.
    pub role: LocalMessageRole,
    /// Optional role-specific content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<BoundedJsonValue>,
    /// Numeric millisecond timestamp.
    pub timestamp: f64,
    /// Optional metadata object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
}

/// Run lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Provider execution is active.
    Running,
    /// Provider execution completed.
    Completed,
    /// Provider execution failed.
    Failed,
    /// Provider execution was cancelled.
    Cancelled,
}

/// Persisted provider-backed turn execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Run {
    /// Run identifier.
    pub id: RunId,
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Lifecycle status.
    pub status: RunStatus,
    /// Optional stop reason with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_reason: Option<Option<String>>,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Optional completion timestamp with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<Option<Timestamp>>,
    /// Optional background flag with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub background: Option<Option<bool>>,
    /// Optional metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
    /// Optional usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
}

impl Serialize for Conversation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Known<'a> {
            id: &'a ConversationId,
            agent_id: &'a AgentId,
            archived: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            archived_at: &'a Option<Option<Timestamp>>,
            created_at: Timestamp,
            updated_at: Timestamp,
            #[serde(skip_serializing_if = "Option::is_none")]
            last_message_at: &'a Option<Option<Timestamp>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            summary: &'a Option<Option<String>>,
            in_context_message_ids: &'a BoundedVec<MessageId, UNBOUNDED_COLLECTION_ITEMS_MAX>,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: &'a Option<Option<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            model_settings: &'a Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            context_window_limit: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            hidden: Option<bool>,
            #[serde(skip_serializing_if = "Option::is_none")]
            tags: &'a Option<BoundedVec<String, STRING_ITEMS_MAX>>,
        }
        let known = Known {
            id: &self.id,
            agent_id: &self.agent_id,
            archived: self.archived,
            archived_at: &self.archived_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_message_at: &self.last_message_at,
            summary: &self.summary,
            in_context_message_ids: &self.in_context_message_ids,
            model: &self.model,
            model_settings: &self.model_settings,
            context_window_limit: self.context_window_limit,
            hidden: self.hidden,
            tags: &self.tags,
        };
        serialize_with_extras(&known, &self.extras, CONVERSATION_FIELDS, serializer)
    }
}
