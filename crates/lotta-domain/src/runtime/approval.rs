use crate::{BoundedMap, BoundedVec, NonEmptyString, UNBOUNDED_MAP_FIELDS_MAX};
use serde::{Deserialize, Serialize};

use crate::bounds::{DIFFS_ITEMS_MAX, PERMISSION_SUGGESTIONS_ITEMS_MAX};

/// Fixed approval request subtype.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ApprovalSubtype {
    /// Permission request for a tool invocation.
    #[serde(rename = "can_use_tool")]
    CanUseTool,
}

/// Pending approval attached to an active turn.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    /// Stable approval request identifier.
    pub request_id: NonEmptyString,
    /// Fixed permission-request subtype.
    pub subtype: ApprovalSubtype,
    /// Requested tool name.
    pub tool_name: NonEmptyString,
    /// Validated tool input.
    pub input: BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    /// Correlated tool call identifier.
    pub tool_call_id: NonEmptyString,
    /// Suggested permission changes.
    pub permission_suggestions:
        BoundedVec<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>, PERMISSION_SUGGESTIONS_ITEMS_MAX>,
    /// Optional blocked path.
    pub blocked_path: Option<String>,
    /// Optional precomputed diff previews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diffs: Option<BoundedVec<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>, DIFFS_ITEMS_MAX>>,
}

/// Controller-owned external tool definition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExternalToolRegistration {
    /// Tool name.
    pub name: NonEmptyString,
    /// Optional display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Tool description.
    pub description: NonEmptyString,
    /// JSON Schema parameters object.
    pub parameters: BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>,
    /// Compatible fields unknown to this version.
    #[serde(flatten)]
    pub extras: crate::EntityExtras,
}
