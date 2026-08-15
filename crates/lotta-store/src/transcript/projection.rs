//! Baseline-compatible active-message projection and repair.

use crate::{StoreError, StoreErrorKind};
use lotta_domain::{BoundedJsonValue, LocalMessage, LocalMessageRole, MessageId};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// Baseline maximum UTF-16 code units retained in one tool-result text part.
pub const REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX: usize = 40_000;
const TRUNCATION_MARKER_PREFIX: &str =
    "\n[Tool result truncated during local transcript repair: omitted ";
const TRUNCATION_MARKER_SUFFIX: &str = " chars]\n";

#[derive(Debug)]
pub(crate) struct Projection {
    pub(crate) messages: Vec<LocalMessage>,
    pub(crate) removed: BTreeSet<MessageId>,
    pub(crate) clipped: bool,
}

pub(crate) fn active(
    messages: Vec<LocalMessage>,
    context: &[MessageId],
    path: &Path,
) -> Result<Projection, StoreError> {
    let selected = select_latest(messages, context, path)?;
    let (messages, removed) = remove_orphans(selected, path)?;
    let (messages, clipped) = clip_tool_results(messages, path)?;
    Ok(Projection {
        messages,
        removed,
        clipped,
    })
}

fn select_latest(
    messages: Vec<LocalMessage>,
    context: &[MessageId],
    path: &Path,
) -> Result<Vec<LocalMessage>, StoreError> {
    let mut latest = BTreeMap::new();
    let mut order = Vec::new();
    order
        .try_reserve_exact(messages.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    for message in messages {
        let id = message.id.clone();
        if latest.insert(id.clone(), message).is_some() {
            order.retain(|existing| existing != &id);
        }
        order.push(id);
    }
    let capacity = if context.is_empty() {
        order.len()
    } else {
        context.len()
    };
    let mut selected = Vec::new();
    selected
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    if context.is_empty() {
        selected.extend(order.into_iter().filter_map(|id| latest.remove(&id)));
    } else {
        selected.extend(context.iter().filter_map(|id| latest.get(id).cloned()));
    }
    Ok(selected)
}

fn remove_orphans(
    messages: Vec<LocalMessage>,
    path: &Path,
) -> Result<(Vec<LocalMessage>, BTreeSet<MessageId>), StoreError> {
    let mut pending = BTreeSet::new();
    let mut repaired = Vec::new();
    repaired
        .try_reserve_exact(messages.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut removed = BTreeSet::new();
    for message in messages {
        match message.role {
            LocalMessageRole::Assistant => {
                pending.clear();
                if assistant_contributes(&message) {
                    collect_tool_calls(&message, &mut pending);
                }
                repaired.push(message);
            }
            LocalMessageRole::User => {
                pending.clear();
                repaired.push(message);
            }
            LocalMessageRole::ToolResult => {
                if tool_result_matches(&message, &pending) {
                    repaired.push(message);
                } else {
                    removed.insert(message.id);
                }
            }
        }
    }
    Ok((repaired, removed))
}

fn assistant_contributes(message: &LocalMessage) -> bool {
    let stop = message
        .extras
        .get("stopReason")
        .and_then(serde_json::Value::as_str);
    !matches!(stop, Some("error" | "aborted"))
}

fn collect_tool_calls(message: &LocalMessage, pending: &mut BTreeSet<String>) {
    let Some(content) = message
        .content
        .as_ref()
        .and_then(|value| value.as_value().as_array())
    else {
        return;
    };
    for part in content {
        if part.get("type").and_then(serde_json::Value::as_str) != Some("toolCall") {
            continue;
        }
        if let Some(id) = part.get("id").and_then(serde_json::Value::as_str) {
            add_lookup_keys(pending, id);
        }
    }
}

fn tool_result_matches(message: &LocalMessage, pending: &BTreeSet<String>) -> bool {
    let Some(id) = message
        .extras
        .get("toolCallId")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    lookup_keys(id).iter().any(|key| pending.contains(*key))
}

fn add_lookup_keys(ids: &mut BTreeSet<String>, id: &str) {
    for key in lookup_keys(id) {
        ids.insert(key.to_owned());
    }
}

fn lookup_keys(id: &str) -> Vec<&str> {
    match id.split_once('|') {
        Some((base, _)) if !base.is_empty() => vec![id, base],
        _ => vec![id],
    }
}

fn clip_tool_results(
    messages: Vec<LocalMessage>,
    path: &Path,
) -> Result<(Vec<LocalMessage>, bool), StoreError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(messages.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut clipped = false;
    for mut message in messages {
        if message.role == LocalMessageRole::ToolResult {
            clipped |= clip_message(&mut message, path)?;
        }
        output.push(message);
    }
    Ok((output, clipped))
}

fn clip_message(message: &mut LocalMessage, path: &Path) -> Result<bool, StoreError> {
    let Some(content) = message.content.as_ref() else {
        return Ok(false);
    };
    let mut value = content.as_value().clone();
    let Some(parts) = value.as_array_mut() else {
        return Ok(false);
    };
    let mut clipped = false;
    for part in parts {
        if part.get("type").and_then(serde_json::Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = part.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(replacement) = truncate_utf16(text, REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX, path)?
        {
            part["text"] = serde_json::Value::String(replacement);
            clipped = true;
        }
    }
    message.content = Some(
        BoundedJsonValue::new(value).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?,
    );
    Ok(clipped)
}

pub(crate) fn truncate_utf16(
    text: &str,
    maximum: usize,
    path: &Path,
) -> Result<Option<String>, StoreError> {
    let length = text.encode_utf16().count();
    if length <= maximum {
        return Ok(None);
    }
    let initial = marker(length.saturating_sub(maximum), path)?;
    let keep = maximum.saturating_sub(initial.encode_utf16().count());
    let (_, _, omitted) = retained_units(text, keep);
    let provisional_marker = marker(omitted, path)?;
    let keep = maximum.saturating_sub(provisional_marker.encode_utf16().count());
    let (head, tail, omitted) = retained_units(text, keep);
    let marker = marker(omitted, path)?;
    let capacity = text.len().min(maximum.saturating_mul(3));
    let mut result = String::new();
    result
        .try_reserve(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    push_utf16_prefix(&mut result, text, head);
    result.push_str(&marker);
    push_utf16_suffix(&mut result, text, tail);
    Ok(Some(result))
}

fn retained_units(text: &str, keep: usize) -> (usize, usize, usize) {
    let requested_head = keep.div_ceil(2);
    let requested_tail = keep.saturating_sub(requested_head);
    let head = prefix_units(text, requested_head);
    let tail = suffix_units(text, requested_tail, head);
    let omitted = text
        .encode_utf16()
        .count()
        .saturating_sub(head)
        .saturating_sub(tail);
    (head, tail, omitted)
}

fn prefix_units(text: &str, maximum: usize) -> usize {
    text.chars()
        .map(char::len_utf16)
        .scan(0_usize, |used, width| {
            *used = used.saturating_add(width);
            Some(*used)
        })
        .take_while(|used| *used <= maximum)
        .last()
        .unwrap_or(0)
}

fn suffix_units(text: &str, maximum: usize, head: usize) -> usize {
    let remaining = text.encode_utf16().count().saturating_sub(head);
    text.chars()
        .rev()
        .map(char::len_utf16)
        .scan(0_usize, |used, width| {
            *used = used.saturating_add(width);
            Some(*used)
        })
        .take_while(|used| *used <= maximum && *used <= remaining)
        .last()
        .unwrap_or(0)
}

fn marker(omitted: usize, path: &Path) -> Result<String, StoreError> {
    let digits = omitted.checked_ilog10().unwrap_or(0) as usize + 1;
    let capacity = TRUNCATION_MARKER_PREFIX
        .len()
        .checked_add(TRUNCATION_MARKER_SUFFIX.len())
        .and_then(|value| value.checked_add(digits))
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut marker = String::new();
    marker
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    write!(
        marker,
        "{TRUNCATION_MARKER_PREFIX}{omitted}{TRUNCATION_MARKER_SUFFIX}"
    )
    .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    Ok(marker)
}

fn push_utf16_prefix(output: &mut String, text: &str, units: usize) {
    let mut used = 0_usize;
    for character in text.chars() {
        let width = character.len_utf16();
        if used.saturating_add(width) > units {
            break;
        }
        output.push(character);
        used += width;
    }
}

fn push_utf16_suffix(output: &mut String, text: &str, units: usize) {
    let mut used = 0_usize;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let width = character.len_utf16();
        if used.saturating_add(width) > units {
            break;
        }
        start = index;
        used += width;
    }
    if used > 0 {
        output.push_str(&text[start..]);
    }
}
