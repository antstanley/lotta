//! Pure runtime-domain entities and state machines.

mod approval;
mod queue_item;
#[path = "turn_state.rs"]
mod turn_state_contract;

use crate::{
    BoundedVec, EntityExtras, NonEmptyString, RunId, RuntimeScope, UNBOUNDED_COLLECTION_ITEMS_MAX,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub use approval::{ApprovalRequest, ApprovalSubtype, ExternalToolRegistration};
pub use queue_item::{
    QueueDropReason, QueueItem, QueueItemKind, QueueItemSource, QueueRemovalDisposition,
};
pub use turn_state_contract::{
    LoopStatus, StopReason, StopReasonError, TurnIdError, TurnLease, TurnLeaseError,
    TurnLeaseExhaustedError, TurnLifecycle, TurnLifecycleError, TurnStateKind, TurnStateView,
    TurnTransitionError,
};

const RUNTIME_SUBSCRIPTIONS_ITEMS_MAX: usize = 256;
const QUEUE_ITEMS_MAX: usize = 300;
const ADMISSION_HISTORY_ITEMS_MAX: usize = 300;

/// Result of submitting one client input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDisposition {
    /// Input started immediately.
    Started,
    /// Input entered the pending queue.
    Queued,
    /// Input was rejected before execution.
    Rejected,
}

/// Bounded duplicate-admission replay history.
#[derive(Debug, Default)]
pub struct AdmissionHistory {
    entries: HashMap<String, InputDisposition>,
    order: VecDeque<String>,
    admissions: u64,
}

impl AdmissionHistory {
    /// Returns a prior disposition, or records and returns a new one.
    ///
    /// # Panics
    /// Panics only if the internal map and insertion order diverge.
    #[must_use]
    pub fn admit(
        &mut self,
        client_message_id: &NonEmptyString,
        disposition: InputDisposition,
    ) -> InputDisposition {
        if let Some(prior) = self.entries.get(client_message_id.as_str()) {
            return *prior;
        }
        if self.order.len() == ADMISSION_HISTORY_ITEMS_MAX
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        let key = client_message_id.as_str().to_owned();
        self.order.push_back(key.clone());
        self.entries.insert(key, disposition);
        self.admissions += 1;
        assert_eq!(self.entries.len(), self.order.len());
        assert!(self.entries.len() <= ADMISSION_HISTORY_ITEMS_MAX);
        disposition
    }

    /// Returns the number of distinct admissions observed.
    #[must_use]
    pub const fn admission_count(&self) -> u64 {
        self.admissions
    }
}

/// Authenticated live runtime connection projection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConnection {
    /// Stable connection identifier.
    pub id: NonEmptyString,
    /// Stable connection ordering ordinal.
    pub ordinal: u64,
    /// Whether protocol initialization completed.
    pub initialized: bool,
    /// Runtime subscriptions.
    pub subscriptions: BoundedVec<RuntimeScope, RUNTIME_SUBSCRIPTIONS_ITEMS_MAX>,
    /// Next connection event sequence.
    pub event_seq: u64,
}

/// Runtime permission mode projected to clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PermissionMode {
    /// Normal approval policy.
    #[serde(rename = "standard")]
    Standard,
    /// Automatically accept edit operations.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Bypass normal permission checks.
    #[serde(rename = "unrestricted")]
    Unrestricted,
    /// Strict permission policy.
    #[serde(rename = "strict")]
    Strict,
}

/// Serializable runtime projection for one conversation scope.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConversationRuntimeSnapshot {
    /// Runtime identity.
    pub runtime: RuntimeScope,
    /// Turn-state kind derived from the owner state.
    pub turn_state: TurnStateKind,
    /// Pending input snapshot.
    pub queue: BoundedVec<QueueItem, QUEUE_ITEMS_MAX>,
    /// Current permission mode.
    pub permission_mode: PermissionMode,
    /// Optional working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// Active or cancelling provider run identifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_ids: Option<BoundedVec<RunId, UNBOUNDED_COLLECTION_ITEMS_MAX>>,
    /// Compatible fields unknown to this version.
    #[serde(flatten)]
    pub extras: EntityExtras,
}

/// Conversation archival state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationArchiveState {
    /// Conversation accepts normal activity.
    Active,
    /// Conversation is archived.
    Archived,
}

impl ConversationArchiveState {
    /// Archives an active conversation.
    #[must_use]
    pub const fn archive(self) -> Self {
        Self::Archived
    }

    /// Unarchives an archived conversation.
    #[must_use]
    pub const fn unarchive(self) -> Self {
        Self::Active
    }
}

#[cfg(test)]
mod approval_keeps_active {
    use super::*;

    #[test]
    fn pending_approval_remains_active() {
        tests::assert_approval_keeps_active();
    }
}

#[cfg(test)]
mod duplicate_client_message_id {
    use super::*;

    #[test]
    fn replays_first_disposition_once() {
        tests::assert_duplicate_client_message_id();
    }
}

#[cfg(test)]
mod queue_item_wire_names {
    use super::*;

    #[test]
    fn wire_and_internal_names_are_separate() {
        tests::assert_queue_item_wire_names();
    }
}

#[cfg(test)]
mod turn_state {
    use super::*;

    #[test]
    fn exhaustive_transition_matrix() {
        tests::assert_turn_state_exhaustive_transition_matrix();
    }

    #[test]
    fn idle_command_idle() {
        tests::assert_turn_state_idle_command_idle();
    }

    #[test]
    fn idle_active_idle() {
        tests::assert_turn_state_idle_active_idle();
    }

    #[test]
    fn active_cancelling_idle() {
        tests::assert_turn_state_active_cancelling_idle();
    }

    #[test]
    fn idle_cancelling_fails() {
        tests::assert_turn_state_idle_cancelling_fails();
    }

    #[test]
    fn command_active_fails() {
        tests::assert_turn_state_command_active_fails();
    }
}

#[cfg(test)]
mod tests;
