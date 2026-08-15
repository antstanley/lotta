//! Conversation-only persistence for active-projection repairs.

use super::TranscriptPaths;
use super::projection::Projection;
use crate::adapter::write_record_locked;
use crate::atomic::FileRevision;
use crate::{LottaStorageLock, StoreError, StoreErrorKind};
use lotta_domain::{BoundedVec, Conversation, MessageId};

pub(crate) fn persist_context(
    paths: &TranscriptPaths,
    conversation: &mut Conversation,
    revision: &FileRevision,
    projection: &Projection,
) -> Result<(), StoreError> {
    if projection.removed.is_empty() {
        return Ok(());
    }
    let values = repaired_ids(conversation, projection, paths)?;
    let repaired = BoundedVec::new(values)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.conversation))?;
    let lock = LottaStorageLock::try_acquire_confined(&paths.root)?;
    let actual = FileRevision::sample_path(&paths.conversation)?;
    if &actual != revision {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.conversation,
        ));
    }
    conversation.in_context_message_ids = repaired;
    write_record_locked(&paths.conversation, conversation, revision, &lock)
}

fn repaired_ids(
    conversation: &Conversation,
    projection: &Projection,
    paths: &TranscriptPaths,
) -> Result<Vec<MessageId>, StoreError> {
    let current = conversation.in_context_message_ids.as_slice();
    let capacity = if current.is_empty() {
        projection.messages.len()
    } else {
        current.len()
    };
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.conversation))?;
    if current.is_empty() {
        values.extend(projection.messages.iter().map(|message| message.id.clone()));
    } else {
        values.extend(
            current
                .iter()
                .filter(|id| !projection.removed.contains(*id))
                .cloned(),
        );
    }
    Ok(values)
}

#[cfg(test)]
#[path = "repair_orphan_tests.rs"]
mod orphan_tool_results;
#[cfg(test)]
#[path = "repair_oversized_tests.rs"]
mod oversized_tool_result;
