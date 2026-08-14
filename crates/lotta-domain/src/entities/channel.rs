#![allow(clippy::option_option)] // Three-state schema fields require absent/null/value.

use super::{BoundedMap, BoundedVec, UNBOUNDED_COLLECTION_ITEMS_MAX, UNBOUNDED_MAP_FIELDS_MAX};
use crate::{AgentId, ConversationId, NonEmptyString, Timestamp};

use serde::{Deserialize, Serialize};

/// Direct-message sender policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Require pairing.
    Pairing,
    /// Permit allowlisted users.
    Allowlist,
    /// Permit all users.
    Open,
}

/// Group-message sender policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicy {
    /// Permit all group participants.
    Open,
    /// Permit allowlisted group participants.
    Allowlist,
}

/// Broad channel chat surface type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelChatType {
    /// Direct-message surface.
    Direct,
    /// Group or channel surface.
    Channel,
}

/// Canonical `snake_case` channel account record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChannelAccount {
    /// Channel plugin identifier.
    pub channel_id: NonEmptyString,
    /// Account identifier.
    pub account_id: NonEmptyString,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Administrative enabled state.
    pub enabled: bool,
    /// Whether required account configuration is present.
    pub configured: bool,
    /// Whether the adapter is running.
    pub running: bool,
    /// Direct-message policy.
    pub dm_policy: DmPolicy,
    /// Optional group policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_policy: Option<GroupPolicy>,
    /// Allowed user identifiers.
    pub allowed_users: BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>,
    /// Optional administrators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_users: Option<BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>>,
    /// Optional commands allowed for non-admin users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_allowed_commands: Option<BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>>,
    /// Plugin-owned account configuration.
    pub config: BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Update timestamp.
    pub updated_at: Timestamp,
}

/// Explicit camelCase runtime projection of a channel account.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeChannelAccount<'a> {
    channel_id: &'a NonEmptyString,
    account_id: &'a NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: &'a Option<String>,
    enabled: bool,
    configured: bool,
    running: bool,
    dm_policy: DmPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_policy: Option<GroupPolicy>,
    allowed_users: &'a BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>,
    #[serde(skip_serializing_if = "Option::is_none")]
    admin_users: &'a Option<BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_allowed_commands: &'a Option<BoundedVec<String, UNBOUNDED_COLLECTION_ITEMS_MAX>>,
    config: &'a BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl ChannelAccount {
    /// Borrows this domain value as a camelCase runtime projection.
    #[must_use]
    pub fn runtime_projection(&self) -> RuntimeChannelAccount<'_> {
        RuntimeChannelAccount {
            channel_id: &self.channel_id,
            account_id: &self.account_id,
            display_name: &self.display_name,
            enabled: self.enabled,
            configured: self.configured,
            running: self.running,
            dm_policy: self.dm_policy,
            group_policy: self.group_policy,
            allowed_users: &self.allowed_users,
            admin_users: &self.admin_users,
            user_allowed_commands: &self.user_allowed_commands,
            config: &self.config,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Canonical `snake_case` channel route record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChannelRoute {
    /// Channel plugin identifier.
    pub channel_id: NonEmptyString,
    /// Channel account identifier.
    pub account_id: NonEmptyString,
    /// Platform chat identifier.
    pub chat_id: NonEmptyString,
    /// Optional broad chat type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<ChannelChatType>,
    /// Optional thread identifier with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub thread_id: Option<Option<String>>,
    /// Target agent.
    pub agent_id: AgentId,
    /// Target conversation.
    pub conversation_id: ConversationId,
    /// Whether inbound routing is enabled.
    pub enabled: bool,
    /// Optional outbound routing flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_enabled: Option<bool>,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Update timestamp.
    pub updated_at: Timestamp,
}

/// Explicit camelCase runtime projection of a channel route.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeChannelRoute<'a> {
    channel_id: &'a NonEmptyString,
    account_id: &'a NonEmptyString,
    chat_id: &'a NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_type: Option<ChannelChatType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: &'a Option<Option<String>>,
    agent_id: &'a AgentId,
    conversation_id: &'a ConversationId,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    outbound_enabled: Option<bool>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl ChannelRoute {
    /// Borrows this domain value as a camelCase runtime projection.
    #[must_use]
    pub fn runtime_projection(&self) -> RuntimeChannelRoute<'_> {
        RuntimeChannelRoute {
            channel_id: &self.channel_id,
            account_id: &self.account_id,
            chat_id: &self.chat_id,
            chat_type: self.chat_type,
            thread_id: &self.thread_id,
            agent_id: &self.agent_id,
            conversation_id: &self.conversation_id,
            enabled: self.enabled,
            outbound_enabled: self.outbound_enabled,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
