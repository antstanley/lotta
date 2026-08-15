use super::types::{ProjectedMessage, ReturnMessageType, SourceMessageKey};
use crate::{StoreError, StoreErrorKind};
use lotta_domain::{AgentId, ConversationId, LocalMessage, LocalMessageRole, MessageId};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) fn messages(
    source: &[LocalMessage],
    agent: &AgentId,
    conversation: &ConversationId,
    path: &Path,
) -> Result<Vec<ProjectedMessage>, StoreError> {
    let mut output = Vec::new();
    let mut ids = BTreeSet::new();
    let mut source_ids = BTreeSet::new();
    for (source_ordinal, message) in source.iter().enumerate() {
        if !source_ids.insert(message.id.clone()) {
            return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
        }
        let key = SourceMessageKey {
            agent_id: agent.clone(),
            conversation_id: conversation.clone(),
            source_id: message.id.clone(),
            source_ordinal,
        };
        let values = source_projections(message);
        for (projection_ordinal, (message_type, value)) in values.into_iter().enumerate() {
            if output.len() >= super::types::QUERY_PROJECTED_MESSAGES_MAX {
                return Err(StoreError::new(StoreErrorKind::Limit, path));
            }
            let id = projection_id(&key, projection_ordinal, path)?;
            if !ids.insert(id.clone()) {
                return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
            }
            output
                .try_reserve(1)
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
            output.push(ProjectedMessage {
                id,
                source: key.clone(),
                projection_ordinal,
                message_type,
                timestamp_ms: message.timestamp,
                value,
            });
        }
    }
    Ok(output)
}

fn source_projections(message: &LocalMessage) -> Vec<(ReturnMessageType, Value)> {
    if compaction_summary(message).is_some() {
        return vec![(
            ReturnMessageType::Summary,
            json!({"summary": compaction_summary(message)}),
        )];
    }
    match message.role {
        LocalMessageRole::User => vec![(
            ReturnMessageType::User,
            json!({"role":"user","content":message.content}),
        )],
        LocalMessageRole::ToolResult => vec![(
            ReturnMessageType::ToolReturn,
            json!({
                "tool_call_id": message.extras.get("toolCallId"),
                "tool_return": message.content,
                "is_error": message.extras.get("isError")
            }),
        )],
        LocalMessageRole::Assistant => assistant_projections(message),
    }
}

fn assistant_projections(message: &LocalMessage) -> Vec<(ReturnMessageType, Value)> {
    let mut output = Vec::new();
    let Some(parts) = message
        .content
        .as_ref()
        .and_then(|value| value.as_value().as_array())
    else {
        return output;
    };
    let mut pending_text = Vec::new();
    let mut pending_reasoning = Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                flush_reasoning(&mut output, &mut pending_reasoning);
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    pending_text.push(text.to_owned());
                }
            }
            Some("thinking") => {
                flush_text(&mut output, &mut pending_text);
                if let Some(text) = part.get("thinking").and_then(Value::as_str) {
                    pending_reasoning.push(text.to_owned());
                }
            }
            Some("toolCall") => {
                flush_text(&mut output, &mut pending_text);
                flush_reasoning(&mut output, &mut pending_reasoning);
                output.push((ReturnMessageType::ApprovalRequest, part.clone()));
            }
            _ => {}
        }
    }
    flush_text(&mut output, &mut pending_text);
    flush_reasoning(&mut output, &mut pending_reasoning);
    output
}

fn flush_text(output: &mut Vec<(ReturnMessageType, Value)>, pending: &mut Vec<String>) {
    if !pending.is_empty() {
        output.push((ReturnMessageType::Assistant, json!({"content": pending})));
        pending.clear();
    }
}

fn flush_reasoning(output: &mut Vec<(ReturnMessageType, Value)>, pending: &mut Vec<String>) {
    if !pending.is_empty() {
        output.push((
            ReturnMessageType::Reasoning,
            json!({"reasoning": pending.join("\n\n")}),
        ));
        pending.clear();
    }
}

fn compaction_summary(message: &LocalMessage) -> Option<&str> {
    message
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("compaction"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_str)
}

fn projection_id(
    source: &SourceMessageKey,
    variant: usize,
    path: &Path,
) -> Result<MessageId, StoreError> {
    projection_id_with_fallback(source, variant, path, fallback_base)
}

fn projection_id_with_fallback(
    source: &SourceMessageKey,
    variant: usize,
    path: &Path,
    fallback: impl FnOnce(&SourceMessageKey, u64, &Path) -> Result<u64, StoreError>,
) -> Result<MessageId, StoreError> {
    let slots = u64::try_from(super::types::QUERY_PROJECTED_MESSAGES_MAX)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let variant =
        u64::try_from(variant).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    if variant >= slots {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let projected = canonical_sequence(source.source_id.as_str())
        .map_or_else(
            || fallback(source, slots, path),
            |sequence| canonical_base(sequence, slots, path),
        )?
        .checked_add(variant)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    MessageId::generate_projection(projected)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))
}

fn canonical_sequence(value: &str) -> Option<u64> {
    let decimal = value.strip_prefix("ui-msg-")?;
    if decimal.is_empty()
        || decimal.starts_with('0')
        || !decimal.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    decimal.parse::<u64>().ok().filter(|value| *value > 0)
}

fn canonical_base(sequence: u64, slots: u64, path: &Path) -> Result<u64, StoreError> {
    let base = sequence
        .checked_sub(1)
        .and_then(|value| value.checked_mul(slots))
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    if base.checked_add(slots).is_none_or(|end| end > u64::MAX / 2) {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(base)
}

#[cfg(test)]
pub(crate) fn messages_with_fallback_group(
    source: &[LocalMessage],
    agent: &AgentId,
    conversation: &ConversationId,
    path: &Path,
    group: u64,
) -> Result<Vec<ProjectedMessage>, StoreError> {
    let mut output = messages(source, agent, conversation, path)?;
    for value in &mut output {
        if canonical_sequence(value.source.source_id.as_str()).is_none() {
            value.id = projection_id_with_fallback(
                &value.source,
                value.projection_ordinal,
                path,
                |_, slots, path| fallback_group_base(group, slots, path),
            )?;
        }
    }
    Ok(output)
}

fn fallback_group_base(group: u64, slots: u64, path: &Path) -> Result<u64, StoreError> {
    let namespace = u64::MAX / 2 + 1;
    let groups = (u64::MAX - namespace) / slots;
    let group = group % groups;
    namespace
        .checked_add(
            group
                .checked_mul(slots)
                .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?,
        )
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))
}

fn fallback_base(source: &SourceMessageKey, slots: u64, path: &Path) -> Result<u64, StoreError> {
    let mut digest = Sha256::new();
    for value in [
        source.agent_id.as_str(),
        source.conversation_id.as_str(),
        source.source_id.as_str(),
    ] {
        digest.update(
            u64::try_from(value.len())
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?
                .to_be_bytes(),
        );
        digest.update(value.as_bytes());
    }
    let bytes: [u8; 32] = digest.finalize().into();
    let hash = u64::from_be_bytes(
        bytes[..8]
            .try_into()
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?,
    );
    fallback_group_base(hash, slots, path)
}
