//! Bounded canonical JSON/JSONL query API.

mod projection_impl;
mod scan;
mod search;
mod types;

pub use types::*;

use crate::adapter::{read_record, run_blocking};
use crate::transcript::transcript_paths;
use crate::{LocalStore, StoreError, StoreErrorKind};
use lotta_domain::{Agent, AgentId, Conversation, ConversationId, MessageId, RuntimeScope};
use std::collections::BTreeSet;

const LOCAL_AGENT_PREFIX: &str = "agent-local-";

impl LocalStore {
    /// Directly reads one canonical local agent record.
    ///
    /// Wrong-prefix, malformed, traversal-like, and absent IDs all produce the same safe `NotFound`
    /// class at this API boundary.
    ///
    /// # Errors
    /// Returns `NotFound` or a typed confined storage failure.
    pub async fn query_agent(&self, id: &str) -> Result<Agent, StoreError> {
        if !valid_local_agent_id(id) {
            return Err(not_found("agent"));
        }
        let id = AgentId::accept(id).map_err(|_| not_found("agent"))?;
        let path = self
            .paths()
            .agent_record(&id)
            .map_err(|_| not_found("agent"))?;
        run_blocking(self.blocking_pool(), move || {
            let value: Agent = read_record(&path).map_err(|error| {
                if error.kind() == StoreErrorKind::NotFound {
                    not_found("agent")
                } else {
                    error
                }
            })?;
            if value.id != id {
                return Err(not_found("agent"));
            }
            Ok(value)
        })
        .await
    }

    /// Lists canonical agents using bounded typed filters and deterministic total ordering.
    ///
    /// # Errors
    /// Returns typed bound, confinement, malformed-record, or filesystem failures.
    pub async fn query_agents(
        &self,
        filters: AgentFilters,
        page: PageRequest,
    ) -> Result<Page<Agent>, StoreError> {
        validate_agent_filters(&filters, &page)?;
        let paths = self.paths().clone();
        let directory = paths.agents();
        run_blocking(self.blocking_pool(), move || {
            let mut values = scan::agents(&paths)?;
            values.retain(|agent| agent_matches(agent, &filters));
            values.sort_by(|left, right| left.id.cmp(&right.id));
            page_values(values, page, |agent| agent.id.as_str(), &directory)
        })
        .await
    }

    /// Lists exact-agent conversations with stable newest-first ordering and opaque paging.
    ///
    /// # Errors
    /// Returns typed bound, confinement, malformed-record, or filesystem failures.
    pub async fn query_conversations(
        &self,
        agent: &AgentId,
        filters: ConversationFilters,
        page: PageRequest,
    ) -> Result<Page<Conversation>, StoreError> {
        validate_conversation_filters(&filters, &page)?;
        self.query_agent(agent.as_str()).await?;
        let directory = self.paths().conversations();
        let agent = agent.clone();
        run_blocking(self.blocking_pool(), move || {
            let mut values = scan::conversations(&directory, Some(&agent), true, false)?;
            values.retain(|conversation| conversation_matches(conversation, &filters));
            values.sort_by(conversation_order);
            page_values(values, page, |value| value.id.as_str(), &directory)
        })
        .await
    }

    /// Reads one canonical conversation using the exact `(agent, conversation)` scope pair.
    ///
    /// # Errors
    /// Returns `NotFound` for absence or ownership mismatch and typed storage failures otherwise.
    pub async fn query_conversation(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Result<Conversation, StoreError> {
        let path = self
            .paths()
            .conversation_dir(agent, conversation)?
            .join("conversation.json");
        let expected_agent = agent.clone();
        let expected_conversation = conversation.clone();
        run_blocking(self.blocking_pool(), move || {
            let value: Conversation = read_record(&path).map_err(map_not_found)?;
            if value.agent_id != expected_agent || value.id != expected_conversation {
                return Err(not_found("conversation"));
            }
            Ok(value)
        })
        .await
    }

    /// Lists the Task25 active message projection for one scoped conversation.
    ///
    /// # Errors
    /// Returns typed scope, transcript, repair, cursor, or bound failures.
    pub async fn query_messages_for_conversation(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        options: MessageListOptions,
    ) -> Result<Vec<ProjectedMessage>, StoreError> {
        validate_message_options(&options)?;
        self.query_conversation(agent, conversation).await?;
        let loaded = self.load_transcript(agent, conversation).await?;
        let path = transcript_paths(self.paths(), agent, conversation)?.messages;
        let mut messages =
            projection_impl::messages(loaded.messages(), agent, conversation, &path)?;
        apply_message_options(&mut messages, &options);
        Ok(messages)
    }

    /// Merges active messages from all and only the agent's canonical conversations.
    ///
    /// # Errors
    /// Returns typed scope, transcript, cursor, confinement, or bound failures.
    pub async fn query_messages_for_agent(
        &self,
        agent: &AgentId,
        options: MessageListOptions,
    ) -> Result<Vec<ProjectedMessage>, StoreError> {
        validate_message_options(&options)?;
        self.query_agent(agent.as_str()).await?;
        let directory = self.paths().conversations();
        let expected = agent.clone();
        let conversations = run_blocking(self.blocking_pool(), move || {
            scan::conversations(&directory, Some(&expected), true, true)
        })
        .await?;
        let mut messages = Vec::new();
        for conversation in conversations {
            let loaded = self.load_transcript(agent, &conversation.id).await?;
            let path = transcript_paths(self.paths(), agent, &conversation.id)?.messages;
            append_projected(
                &mut messages,
                projection_impl::messages(loaded.messages(), agent, &conversation.id, &path)?,
                &path,
            )?;
        }
        messages.sort_by(message_order);
        ensure_unique_projection_ids(&messages)?;
        apply_message_options(&mut messages, &options);
        Ok(messages)
    }

    /// Resolves a generated projection ID in one scope to its canonical source record.
    ///
    /// # Errors
    /// Returns `NotFound` when the projection does not exist in the scoped active transcript.
    pub async fn query_message_by_projected_id(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        id: &MessageId,
    ) -> Result<MessageLookup, StoreError> {
        let loaded = self.load_transcript(agent, conversation).await?;
        let path = transcript_paths(self.paths(), agent, conversation)?.messages;
        let projected = projection_impl::messages(loaded.messages(), agent, conversation, &path)?;
        let value = projected
            .into_iter()
            .find(|message| &message.id == id)
            .ok_or_else(|| not_found("message"))?;
        lookup_source(loaded.messages(), value.source.clone(), Some(value))
    }

    /// Resolves a persisted source local-message ID in one scope.
    ///
    /// # Errors
    /// Returns `NotFound` when the source does not exist in the scoped active transcript.
    pub async fn query_message_by_source_id(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        id: &MessageId,
    ) -> Result<MessageLookup, StoreError> {
        let loaded = self.load_transcript(agent, conversation).await?;
        let ordinal = loaded
            .messages()
            .iter()
            .position(|message| &message.id == id)
            .ok_or_else(|| not_found("message"))?;
        let source = SourceMessageKey {
            agent_id: agent.clone(),
            conversation_id: conversation.clone(),
            source_id: id.clone(),
            source_ordinal: ordinal,
        };
        lookup_source(loaded.messages(), source, None)
    }

    /// Returns a scoped conversation and its newest bounded active message suffix.
    ///
    /// # Errors
    /// Returns typed scope, transcript, or limit failures.
    pub async fn query_resume_tail(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        limit: usize,
    ) -> Result<ResumeTail, StoreError> {
        validate_resume_tail_limit(limit)?;
        let conversation_value = self.query_conversation(agent, conversation).await?;
        let mut messages = self
            .query_messages_for_conversation(
                agent,
                conversation,
                MessageListOptions {
                    order: MessageOrder::Ascending,
                    ..MessageListOptions::default()
                },
            )
            .await?;
        let start = if messages.len() > limit {
            messages.len() - limit
        } else {
            0
        };
        messages.drain(0..start);
        Ok(ResumeTail {
            conversation: conversation_value,
            messages,
        })
    }

    /// Searches persisted source transcript rows without loading or repairing Task25 state.
    ///
    /// # Errors
    /// Returns typed bound, confinement, or filesystem failures; malformed transcript rows are
    /// skipped defensively as in the pinned local baseline.
    pub async fn query_transcript_search(
        &self,
        input: TranscriptSearch,
    ) -> Result<Vec<SearchHit>, StoreError> {
        search::validate(&input)?;
        let store = self.clone();
        run_blocking(self.blocking_pool(), move || search::run(&store, &input)).await
    }
}

/// Returns initialized, live subscribers to `scope` in stable ordinal/ID order.
///
/// # Errors
/// Returns a typed limit failure when the external snapshot exceeds its bound.
pub fn query_runtime_subscribers(
    connections: &[RuntimeConnectionSnapshot],
    scope: &RuntimeScope,
) -> Result<Vec<RuntimeSubscriber>, StoreError> {
    if connections.len() > QUERY_RUNTIME_CONNECTIONS_MAX {
        return Err(StoreError::new(
            StoreErrorKind::Limit,
            "runtime-connections",
        ));
    }
    let mut output: Vec<_> = connections
        .iter()
        .filter(|value| {
            value.live
                && value.connection.initialized
                && value.connection.subscriptions.as_slice().contains(scope)
        })
        .map(|value| RuntimeSubscriber {
            id: value.connection.id.as_str().to_owned(),
            ordinal: value.connection.ordinal,
        })
        .collect();
    output.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(output)
}

fn validate_resume_tail_limit(limit: usize) -> Result<(), StoreError> {
    if limit > RESUME_TAIL_MESSAGES_MAX {
        Err(StoreError::new(StoreErrorKind::Limit, "resume-tail"))
    } else {
        Ok(())
    }
}

fn valid_local_agent_id(value: &str) -> bool {
    value.starts_with(LOCAL_AGENT_PREFIX)
        && value.len() <= crate::paths::STORE_KEY_BYTES_MAX
        && !value.contains(['/', '\\', '\0'])
        && value != LOCAL_AGENT_PREFIX
}

fn validate_agent_filters(filters: &AgentFilters, page: &PageRequest) -> Result<(), StoreError> {
    validate_page(page)?;
    validate_text(filters.query.as_deref())?;
    validate_text(filters.name.as_ref().map(|value| value.0.as_str()))?;
    validate_tags(&filters.tags)
}

fn validate_conversation_filters(
    filters: &ConversationFilters,
    page: &PageRequest,
) -> Result<(), StoreError> {
    validate_page(page)?;
    validate_tags(&filters.tags)
}

fn validate_page(page: &PageRequest) -> Result<(), StoreError> {
    if page
        .limit
        .is_some_and(|limit| limit == 0 || limit > QUERY_PAGE_ITEMS_MAX)
        || page
            .after
            .as_ref()
            .is_some_and(|cursor| cursor.as_str().len() > QUERY_TEXT_BYTES_MAX)
    {
        return Err(StoreError::new(StoreErrorKind::Limit, "query-page"));
    }
    Ok(())
}

fn validate_text(value: Option<&str>) -> Result<(), StoreError> {
    if value.is_some_and(|text| text.len() > QUERY_TEXT_BYTES_MAX) {
        Err(StoreError::new(StoreErrorKind::Limit, "query-text"))
    } else {
        Ok(())
    }
}

fn validate_tags(tags: &[String]) -> Result<(), StoreError> {
    if tags.len() > QUERY_TAGS_MAX || tags.iter().any(|tag| tag.len() > QUERY_TEXT_BYTES_MAX) {
        Err(StoreError::new(StoreErrorKind::Limit, "query-tags"))
    } else {
        Ok(())
    }
}

fn agent_matches(agent: &Agent, filters: &AgentFilters) -> bool {
    let hidden = agent.hidden.flatten().unwrap_or(false);
    if !tri_matches(hidden, filters.hidden) || !all_tags(agent.tags.as_slice(), &filters.tags) {
        return false;
    }
    if let Some((name, mode)) = &filters.name {
        let matches = match mode {
            NameMatch::Exact => agent.name.as_str() == name,
            NameMatch::Substring => agent
                .name
                .as_str()
                .to_lowercase()
                .contains(&name.to_lowercase()),
        };
        if !matches {
            return false;
        }
    }
    filters.query.as_ref().is_none_or(|query| {
        let description = agent
            .description
            .as_ref()
            .and_then(|value| value.as_deref());
        [
            Some(agent.name.as_str()),
            description,
            Some(agent.id.as_str()),
            Some(agent.model.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.to_lowercase().contains(&query.to_lowercase()))
    })
}

fn conversation_matches(value: &Conversation, filters: &ConversationFilters) -> bool {
    tri_matches(value.archived, filters.archived)
        && tri_matches(value.hidden.unwrap_or(false), filters.hidden)
        && all_tags(
            value.tags.as_ref().map_or(&[], |tags| tags.as_slice()),
            &filters.tags,
        )
}

fn tri_matches(value: bool, filter: TriState) -> bool {
    match filter {
        TriState::Any => true,
        TriState::True => value,
        TriState::False => !value,
    }
}

fn all_tags(actual: &[String], required: &[String]) -> bool {
    required.iter().all(|tag| actual.contains(tag))
}

fn conversation_order(left: &Conversation, right: &Conversation) -> std::cmp::Ordering {
    let left_date = left.last_message_at.flatten().unwrap_or(left.updated_at);
    let right_date = right.last_message_at.flatten().unwrap_or(right.updated_at);
    right_date
        .cmp(&left_date)
        .then_with(|| left.id.cmp(&right.id))
}

fn page_values<T>(
    mut values: Vec<T>,
    page: PageRequest,
    id: impl Fn(&T) -> &str,
    path: &std::path::Path,
) -> Result<Page<T>, StoreError> {
    if let Some(cursor) = page.after {
        let index = values
            .iter()
            .position(|value| id(value) == cursor.as_str())
            .ok_or_else(|| StoreError::new(StoreErrorKind::NotFound, "cursor"))?;
        values.drain(0..=index);
    }
    let limit = page.limit.unwrap_or(QUERY_PAGE_ITEMS_MAX);
    let has_more = values.len() > limit;
    values.truncate(limit);
    let next = if has_more {
        values.last().map(|value| Cursor::new(id(value).to_owned()))
    } else {
        None
    };
    if values.len() > QUERY_PAGE_ITEMS_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(Page {
        items: values,
        next,
    })
}

fn validate_message_options(options: &MessageListOptions) -> Result<(), StoreError> {
    if options
        .limit
        .is_some_and(|limit| limit == 0 || limit > QUERY_PAGE_ITEMS_MAX)
        || options.return_types.len() > ReturnMessageType::ALL.len()
    {
        return Err(StoreError::new(StoreErrorKind::Limit, "message-list"));
    }
    Ok(())
}

impl ReturnMessageType {
    const ALL: [Self; 6] = [
        Self::User,
        Self::Assistant,
        Self::Reasoning,
        Self::ApprovalRequest,
        Self::ToolReturn,
        Self::Summary,
    ];
}

fn apply_message_options(messages: &mut Vec<ProjectedMessage>, options: &MessageListOptions) {
    if !options.return_types.is_empty() {
        let included: BTreeSet<_> = options.return_types.iter().copied().collect();
        messages.retain(|message| included.contains(&message.message_type));
    }
    apply_cursor(messages, options.before.as_ref(), true);
    apply_cursor(messages, options.after.as_ref(), false);
    if options.order == MessageOrder::Descending {
        messages.reverse();
    }
    if let Some(limit) = options.limit {
        messages.truncate(limit);
    }
}

fn apply_cursor(messages: &mut Vec<ProjectedMessage>, cursor: Option<&MessageId>, before: bool) {
    let Some(index) =
        cursor.and_then(|cursor| messages.iter().position(|message| &message.id == cursor))
    else {
        return;
    };
    if before {
        messages.truncate(index);
    } else {
        messages.drain(0..=index);
    }
}

fn message_order(left: &ProjectedMessage, right: &ProjectedMessage) -> std::cmp::Ordering {
    left.timestamp_ms
        .total_cmp(&right.timestamp_ms)
        .then_with(|| {
            left.source
                .conversation_id
                .cmp(&right.source.conversation_id)
        })
        .then_with(|| left.source.source_ordinal.cmp(&right.source.source_ordinal))
        .then_with(|| left.projection_ordinal.cmp(&right.projection_ordinal))
}

fn append_projected(
    output: &mut Vec<ProjectedMessage>,
    mut values: Vec<ProjectedMessage>,
    path: &std::path::Path,
) -> Result<(), StoreError> {
    let length = output
        .len()
        .checked_add(values.len())
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    if length > QUERY_PROJECTED_MESSAGES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    output
        .try_reserve(values.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    output.append(&mut values);
    Ok(())
}

fn ensure_unique_projection_ids(messages: &[ProjectedMessage]) -> Result<(), StoreError> {
    let mut ids = BTreeSet::new();
    for message in messages {
        if !ids.insert(message.id.clone()) {
            return Err(StoreError::new(
                StoreErrorKind::StorageConflict,
                "message-projection",
            ));
        }
    }
    Ok(())
}

fn lookup_source(
    messages: &[lotta_domain::LocalMessage],
    source: SourceMessageKey,
    projection: Option<ProjectedMessage>,
) -> Result<MessageLookup, StoreError> {
    let message = messages
        .get(source.source_ordinal)
        .filter(|message| message.id == source.source_id)
        .cloned()
        .ok_or_else(|| not_found("message"))?;
    Ok(MessageLookup {
        source,
        message,
        projection,
    })
}

fn map_not_found(error: StoreError) -> StoreError {
    if error.kind() == StoreErrorKind::NotFound {
        not_found("conversation")
    } else {
        error
    }
}

fn not_found(entity: &'static str) -> StoreError {
    StoreError::new(StoreErrorKind::NotFound, entity)
}

#[cfg(test)]
mod review_evidence;
#[cfg(test)]
mod search_review_evidence;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[tokio::test]
async fn wrong_prefix_404() {
    tests::wrong_prefix_404_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn deterministic_order() {
    tests::deterministic_order_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn cursor_covers_all() {
    tests::cursor_covers_all_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn scope_isolation() {
    tests::scope_isolation_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn messages_for_conversation() {
    tests::messages_for_conversation_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn messages_for_agent() {
    tests::messages_for_agent_body().await;
}
#[cfg(test)]
pub(crate) mod projection {
    #[tokio::test]
    async fn same_source_message() {
        super::tests::projection_body::same_source_message_body().await;
    }
}
#[cfg(test)]
#[tokio::test]
async fn resume_tail() {
    tests::resume_tail_body().await;
}
#[cfg(test)]
#[tokio::test]
async fn transcript_search() {
    tests::transcript_search_body().await;
}
#[cfg(test)]
#[test]
fn runtime_subscribers() {
    tests::runtime_subscribers_body();
}
