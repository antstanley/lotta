use crate::{BoundedJsonValue, EntityExtras, NonEmptyString, Timestamp};
use serde::{Deserialize, Serialize};

/// Kind of pending runtime input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueItemKind {
    /// User or system message.
    Message,
    /// Background task completion.
    TaskNotification,
    /// Scheduled prompt.
    CronPrompt,
    /// Tool approval result.
    ApprovalResult,
    /// Overlay interaction.
    OverlayAction,
    /// Mod continuation.
    ModContinue,
}

/// Source that produced a queue item.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueItemSource {
    /// Direct user input.
    User,
    /// Background task notification.
    TaskNotification,
    /// Scheduler.
    Cron,
    /// Subagent result.
    Subagent,
    /// Internal runtime input.
    System,
    /// Channel input.
    Channel,
}

/// Public disposition emitted when an item leaves the queue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueRemovalDisposition {
    /// Item was selected for processing.
    Dequeued,
    /// Item was removed without processing.
    Cancelled,
}

/// Internal reason why an item was dropped without processing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueDropReason {
    /// Queue capacity prevented retention.
    BufferLimit,
    /// The item's captured lease generation is stale.
    StaleGeneration,
}

/// Bounded pending runtime input.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QueueItem {
    /// Stable queue item identifier.
    pub id: NonEmptyString,
    /// Originating client submission identifier.
    pub client_message_id: NonEmptyString,
    /// Content kind.
    pub kind: QueueItemKind,
    /// Content source.
    pub source: QueueItemSource,
    /// Bounded JSON content.
    pub content: BoundedJsonValue,
    /// UTC enqueue timestamp.
    pub enqueued_at: Timestamp,
    /// Compatible fields unknown to this version.
    #[serde(flatten)]
    pub extras: EntityExtras,
}
