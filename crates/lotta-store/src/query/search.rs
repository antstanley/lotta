use super::projection_impl;
use super::scan;
use super::types::{
    ProjectedMessage, QUERY_PROJECTED_MESSAGES_MAX, QUERY_TEXT_BYTES_MAX, SearchHit,
    TranscriptSearch,
};
use crate::transcript::{TranscriptPaths, transcript_paths};
use crate::{LocalStore, StoreError, StoreErrorKind};
use lotta_domain::Conversation;
use serde_json::Value;

pub(crate) fn validate(input: &TranscriptSearch) -> Result<(), StoreError> {
    if input.query.is_empty()
        || input.query.len() > QUERY_TEXT_BYTES_MAX
        || input.limit == 0
        || input.limit > super::types::QUERY_PAGE_ITEMS_MAX
        || input
            .conversation_id
            .as_ref()
            .is_some_and(lotta_domain::ConversationId::is_default)
            && input.agent_id.is_none()
    {
        return Err(StoreError::new(StoreErrorKind::Limit, "transcript-search"));
    }
    Ok(())
}

pub(crate) fn run(
    store: &LocalStore,
    input: &TranscriptSearch,
) -> Result<Vec<SearchHit>, StoreError> {
    validate(input)?;
    let query = ParsedQuery::parse(&input.query);
    if query.empty() {
        return Ok(Vec::new());
    }
    let conversations = scan::conversations(
        store.paths().conversations().as_path(),
        input.agent_id.as_ref(),
        false,
        true,
    )?;
    let mut hits = Vec::new();
    for conversation in conversations {
        if !matches_scope(&conversation, input) {
            continue;
        }
        let paths = transcript_paths(store.paths(), &conversation.agent_id, &conversation.id)?;
        collect(&paths, &conversation, &query, &mut hits, input.limit)?;
    }
    Ok(hits)
}

fn matches_scope(conversation: &Conversation, input: &TranscriptSearch) -> bool {
    input
        .agent_id
        .as_ref()
        .is_none_or(|agent| &conversation.agent_id == agent)
        && input
            .conversation_id
            .as_ref()
            .is_none_or(|id| &conversation.id == id)
        && (input.include_hidden || conversation.hidden != Some(true))
}

fn collect(
    paths: &TranscriptPaths,
    conversation: &Conversation,
    query: &ParsedQuery,
    output: &mut Vec<SearchHit>,
    limit: usize,
) -> Result<(), StoreError> {
    let messages =
        crate::transcript::load::load_search_nonmutating(paths, QUERY_PROJECTED_MESSAGES_MAX)?;
    let projected = projection_impl::messages(
        &messages,
        &conversation.agent_id,
        &conversation.id,
        &paths.messages,
    )?;
    for message in projected {
        let text = searchable_text(&message, &paths.messages)?;
        if let Some(score) = query.score(&text) {
            insert_hit(
                output,
                SearchHit {
                    source: message.source.clone(),
                    message,
                    score,
                },
                limit,
                &paths.messages,
            )?;
        }
    }
    Ok(())
}

fn hit_order(left: &SearchHit, right: &SearchHit) -> std::cmp::Ordering {
    left.score
        .cmp(&right.score)
        .then_with(|| {
            right
                .message
                .timestamp_ms
                .total_cmp(&left.message.timestamp_ms)
        })
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| {
            left.message
                .projection_ordinal
                .cmp(&right.message.projection_ordinal)
        })
}

fn insert_hit(
    output: &mut Vec<SearchHit>,
    hit: SearchHit,
    limit: usize,
    path: &std::path::Path,
) -> Result<(), StoreError> {
    output
        .try_reserve(1)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let index = output
        .binary_search_by(|value| hit_order(value, &hit))
        .unwrap_or_else(|value| value);
    output.insert(index, hit);
    output.truncate(limit);
    Ok(())
}

fn searchable_text(
    message: &ProjectedMessage,
    path: &std::path::Path,
) -> Result<String, StoreError> {
    let value = &message.value;
    let mut output = String::new();
    let message_type = serde_json::to_value(message.message_type)
        .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?;
    if let Some(message_type) = message_type.as_str() {
        append_text(message_type, &mut output, path)?;
    }
    for field in ["reasoning", "summary"] {
        if let Some(text) = value.get(field).and_then(Value::as_str) {
            append_text(text, &mut output, path)?;
        }
    }
    append_content(value.get("content"), &mut output, path)?;
    if message.message_type == super::types::ReturnMessageType::ApprovalRequest {
        if let Some(name) = value.get("name").and_then(Value::as_str) {
            append_text(name, &mut output, path)?;
        }
        if let Some(arguments) = value.get("arguments") {
            append_json(arguments, &mut output, path)?;
        }
    }
    if message.message_type == super::types::ReturnMessageType::ToolReturn {
        for field in ["tool_return", "func_response"] {
            if let Some(value) = value.get(field) {
                append_json(value, &mut output, path)?;
            }
        }
    }
    Ok(output)
}

fn append_content(
    value: Option<&Value>,
    output: &mut String,
    path: &std::path::Path,
) -> Result<(), StoreError> {
    match value {
        Some(Value::String(text)) => append_text(text, output, path),
        Some(Value::Array(parts)) => {
            for part in parts {
                if let Some(text) = part
                    .as_str()
                    .or_else(|| part.as_object()?.get("text")?.as_str())
                {
                    append_text(text, output, path)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn append_json(
    value: &Value,
    output: &mut String,
    path: &std::path::Path,
) -> Result<(), StoreError> {
    if let Some(text) = value.as_str() {
        append_text(text, output, path)
    } else {
        let text = serde_json::to_string(value)
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?;
        append_text(&text, output, path)
    }
}

fn append_text(text: &str, output: &mut String, path: &std::path::Path) -> Result<(), StoreError> {
    let additional = text
        .len()
        .checked_add(1)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    output
        .try_reserve(additional)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    output.push_str(text);
    output.push('\n');
    Ok(())
}

struct ParsedQuery {
    terms: Vec<String>,
    phrases: Vec<String>,
}

impl ParsedQuery {
    fn parse(input: &str) -> Self {
        let mut terms = Vec::new();
        let mut phrases = Vec::new();
        let mut quoted = false;
        let mut buffer = String::new();
        for character in input.trim().chars() {
            if character == '"' {
                push_token(&mut terms, &mut phrases, &mut buffer, quoted);
                quoted = !quoted;
            } else if character.is_whitespace() && !quoted {
                push_token(&mut terms, &mut phrases, &mut buffer, false);
            } else {
                buffer.push(character);
            }
        }
        push_token(&mut terms, &mut phrases, &mut buffer, quoted);
        if quoted {
            terms = input.split_whitespace().map(str::to_owned).collect();
            phrases.clear();
        }
        Self { terms, phrases }
    }

    fn empty(&self) -> bool {
        self.terms.is_empty() && self.phrases.is_empty()
    }

    fn score(&self, text: &str) -> Option<u64> {
        let text = normalize(text);
        let mut score = 0_u64;
        for phrase in &self.phrases {
            score = score.checked_add(position(&text, phrase)? / 10)?;
        }
        for term in &self.terms {
            let normalized = normalize(term);
            score = score.checked_add(position(&text, &normalized)?)?;
        }
        Some(score)
    }
}

fn push_token(
    terms: &mut Vec<String>,
    phrases: &mut Vec<String>,
    buffer: &mut String,
    quoted: bool,
) {
    let value = buffer.trim();
    if !value.is_empty() {
        if quoted {
            phrases.push(value.to_owned());
        } else {
            terms.push(value.to_owned());
        }
    }
    buffer.clear();
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn position(text: &str, query: &str) -> Option<u64> {
    text.find(&normalize(query))
        .and_then(|value| u64::try_from(value).ok())
}
