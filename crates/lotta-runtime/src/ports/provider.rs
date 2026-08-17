use super::{PortFuture, ProviderEvent};
use crate::RuntimeError;
use crate::boundary::{ProviderEventText, ProviderImageBytes, ProviderName, ProviderText};
use crate::bounds::{
    PROVIDER_CONTENT_PARTS_MAX, PROVIDER_MESSAGES_MAX, PROVIDER_REQUEST_BYTES_MAX,
    PROVIDER_STREAM_EVENTS_MAX, PROVIDER_TOOLS_MAX, TOOL_ARGUMENT_BYTES_MAX,
};
use lotta_domain::{BoundedJsonValue, BoundedVec, ModelDescriptor};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Write};
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

/// Ordered, owning message collection bounded before provider-request retention.
pub type ProviderMessages = BoundedVec<ProviderMessage, { PROVIDER_MESSAGES_MAX.value }>;
/// Ordered, owning content-part collection bounded before message retention.
pub type ProviderContent = BoundedVec<ProviderContentPart, { PROVIDER_CONTENT_PARTS_MAX.value }>;
/// Ordered, owning tool-definition collection bounded before request retention.
pub type ProviderTools = BoundedVec<ProviderToolDefinition, { PROVIDER_TOOLS_MAX.value }>;

/// Normalized author role for one provider message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMessageRole {
    /// End-user input.
    User,
    /// Model-generated output supplied as prior context.
    Assistant,
    /// Tool result associated with a prior call identifier.
    Tool,
}
/// Policy applied when an adapter cannot represent request images.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePolicy {
    /// Reject unsupported images rather than silently changing the request.
    Strict,
    /// Remove only unsupported image parts while preserving all retained-part order.
    Drop,
}
/// One normalized, ordered part of a provider message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderContentPart {
    /// Bounded textual content.
    Text(ProviderText),
    /// Inline bounded image bytes and their MIME media type.
    Image {
        /// MIME media type used by the adapter when mapping the image.
        media_type: ProviderName,
        /// Owning image payload, bounded before this part can retain it.
        bytes: ProviderImageBytes,
    },
}

/// One normalized provider message whose content preserves source part order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderMessage {
    /// Author role interpreted by the provider.
    pub role: ProviderMessageRole,
    /// Bounded ordered content parts owned by this message.
    pub content: ProviderContent,
    /// Required call identifier for a tool result, absent for ordinary messages.
    pub tool_call_id: Option<ToolCallId>,
}
/// Model-visible tool definition supplied with a provider request.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderToolDefinition {
    /// Stable model-facing tool name.
    pub name: ProviderName,
    /// Bounded model-facing tool description.
    pub description: ProviderText,
    /// Bounded JSON input schema interpreted by the provider.
    pub input_schema: BoundedJsonValue,
}
/// Normalized provider tool-selection policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderToolChoice {
    /// Let the provider choose whether and which tool to call.
    Auto,
    /// Forbid tool calls.
    None,
    /// Require some tool call.
    Required,
    /// Require the named tool.
    Named(ProviderName),
}
/// Optional normalized controls for provider reasoning features.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningControls {
    /// Whether reasoning generation is enabled.
    pub enabled: bool,
    /// Optional provider-neutral effort label.
    pub effort: Option<ProviderName>,
    /// Optional provider-neutral service tier label.
    pub tier: Option<ProviderName>,
}
/// Validated nonzero provider token limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenLimit(u64);
impl TokenLimit {
    /// Validates a positive token limit.
    ///
    /// # Errors
    /// Returns invalid data when `value` is zero.
    pub fn new(value: u64) -> Result<Self, RuntimeError> {
        if value == 0 {
            Err(invalid("provider token limit"))
        } else {
            Ok(Self(value))
        }
    }
    /// Returns the validated positive token count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
/// Validated finite, positive duration available to one provider operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderDeadline(Duration);
impl ProviderDeadline {
    /// Validates a duration representable as wire milliseconds.
    ///
    /// # Errors
    /// Returns invalid data for zero or a duration whose milliseconds exceed `u64`.
    pub fn new(value: Duration) -> Result<Self, RuntimeError> {
        if value.is_zero() || value.as_millis() > u128::from(u64::MAX) {
            Err(invalid("provider deadline"))
        } else {
            Ok(Self(value))
        }
    }
    /// Returns the validated operation duration.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

/// Fully normalized, owning input to one provider operation.
///
/// Callers must construct all bounded fields before this value retains them and validate the
/// compact aggregate wire size before dispatch. Ownership transfers to [`ProviderPort::stream`];
/// the shared cancellation token is terminal and applies to the request and event channel.
#[derive(Clone, Debug)]
pub struct ProviderRequest {
    /// Canonical domain model descriptor; no vendor model type crosses the port.
    pub model: ModelDescriptor,
    /// Optional bounded system instruction.
    pub system_prompt: Option<ProviderText>,
    /// Bounded ordered conversation messages.
    pub messages: ProviderMessages,
    /// Bounded ordered model-visible tools.
    pub tools: ProviderTools,
    /// Tool-selection policy.
    pub tool_choice: ProviderToolChoice,
    /// Behavior when the adapter lacks image support.
    pub image_policy: ImagePolicy,
    /// Maximum context tokens admitted for this operation.
    pub context_tokens_max: TokenLimit,
    /// Maximum output tokens requested for this operation.
    pub output_tokens_max: TokenLimit,
    /// Provider-neutral reasoning controls.
    pub reasoning: ReasoningControls,
    /// Terminal cancellation shared with the bounded event channel.
    pub cancellation: CancellationToken,
    /// Maximum operation duration enforced by the adapter/runtime owner.
    pub deadline: ProviderDeadline,
}

#[derive(Serialize)]
struct RequestWire<'a> {
    model: &'a ModelDescriptor,
    system_prompt: Option<&'a str>,
    messages: VecWire<'a, ProviderMessage>,
    tools: VecWire<'a, ProviderToolDefinition>,
    tool_choice: ToolChoiceWire<'a>,
    image_policy: ImagePolicy,
    context_tokens_max: u64,
    output_tokens_max: u64,
    reasoning: ReasoningWire<'a>,
    deadline_millis: u64,
}
struct VecWire<'a, T>(&'a [T]);
impl<T: WireSerialize> Serialize for VecWire<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for item in self.0 {
            seq.serialize_element(&item.wire())?;
        }
        seq.end()
    }
}
trait WireSerialize {
    type Wire<'a>: Serialize
    where
        Self: 'a;
    fn wire(&self) -> Self::Wire<'_>;
}
#[derive(Serialize)]
struct MessageWire<'a> {
    role: ProviderMessageRole,
    content: ContentWire<'a>,
    tool_call_id: Option<&'a str>,
}
struct ContentWire<'a>(&'a [ProviderContentPart]);
impl Serialize for ContentWire<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeSeq, SerializeStruct};
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for part in self.0 {
            struct Part<'a>(&'a ProviderContentPart);
            impl Serialize for Part<'_> {
                fn serialize<S2: serde::Serializer>(&self, s: S2) -> Result<S2::Ok, S2::Error> {
                    match self.0 {
                        ProviderContentPart::Text(value) => {
                            let mut st = s.serialize_struct("Part", 2)?;
                            st.serialize_field("type", "text")?;
                            st.serialize_field("text", value.as_str())?;
                            st.end()
                        }
                        ProviderContentPart::Image { media_type, bytes } => {
                            let mut st = s.serialize_struct("Part", 3)?;
                            st.serialize_field("type", "image")?;
                            st.serialize_field("media_type", media_type.as_str())?;
                            st.serialize_field("bytes", bytes.as_slice())?;
                            st.end()
                        }
                    }
                }
            }
            seq.serialize_element(&Part(part))?;
        }
        seq.end()
    }
}
impl WireSerialize for ProviderMessage {
    type Wire<'a> = MessageWire<'a>;
    fn wire(&self) -> MessageWire<'_> {
        MessageWire {
            role: self.role,
            content: ContentWire(self.content.as_slice()),
            tool_call_id: self.tool_call_id.as_ref().map(ToolCallId::as_str),
        }
    }
}
#[derive(Serialize)]
struct ToolWire<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a BoundedJsonValue,
}
impl WireSerialize for ProviderToolDefinition {
    type Wire<'a> = ToolWire<'a>;
    fn wire(&self) -> ToolWire<'_> {
        ToolWire {
            name: self.name.as_str(),
            description: self.description.as_str(),
            input_schema: &self.input_schema,
        }
    }
}
#[derive(Serialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
enum ToolChoiceWire<'a> {
    Auto,
    None,
    Required,
    Named(&'a str),
}
#[derive(Serialize)]
struct ReasoningWire<'a> {
    enabled: bool,
    effort: Option<&'a str>,
    tier: Option<&'a str>,
}

struct BoundedCounter {
    count: usize,
    max: usize,
}
impl Write for BoundedCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .count
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("provider request size overflow"))?;
        if next > self.max {
            return Err(io::Error::other("provider request size limit"));
        }
        self.count = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl ProviderRequest {
    /// Validates the exact compact normalized JSON wire object before dispatch.
    ///
    /// Cancellation is excluded because it is control-only.
    ///
    /// # Errors
    /// Returns the request byte-limit error when serialization overflows or exceeds 32 MiB.
    pub fn validate_bytes(&self) -> Result<(), RuntimeError> {
        self.normalized_wire_bytes().map(|_| ())
    }
    /// Measures the exact compact normalized JSON wire size without allocating that wire.
    ///
    /// # Errors
    /// Returns the request byte-limit error if the deadline is unrepresentable, serialization
    /// overflows, or the compact wire exceeds 32 MiB.
    pub fn normalized_wire_bytes(&self) -> Result<usize, RuntimeError> {
        let deadline_millis =
            u64::try_from(self.deadline.get().as_millis()).map_err(|_| limit_request())?;
        let wire = RequestWire {
            model: &self.model,
            system_prompt: self.system_prompt.as_ref().map(ProviderText::as_str),
            messages: VecWire(self.messages.as_slice()),
            tools: VecWire(self.tools.as_slice()),
            tool_choice: match &self.tool_choice {
                ProviderToolChoice::Auto => ToolChoiceWire::Auto,
                ProviderToolChoice::None => ToolChoiceWire::None,
                ProviderToolChoice::Required => ToolChoiceWire::Required,
                ProviderToolChoice::Named(v) => ToolChoiceWire::Named(v.as_str()),
            },
            image_policy: self.image_policy,
            context_tokens_max: self.context_tokens_max.get(),
            output_tokens_max: self.output_tokens_max.get(),
            reasoning: ReasoningWire {
                enabled: self.reasoning.enabled,
                effort: self.reasoning.effort.as_ref().map(ProviderName::as_str),
                tier: self.reasoning.tier.as_ref().map(ProviderName::as_str),
            },
            deadline_millis,
        };
        measure_wire_with_limit(&wire, PROVIDER_REQUEST_BYTES_MAX.value)
    }
    /// Maps content for an adapter's image capability without reordering retained parts.
    ///
    /// # Errors
    /// Returns invalid data when images are unsupported and this request uses strict policy, or if
    /// reconstructing the bounded retained content fails.
    pub fn content_for_image_support(
        &self,
        content: &ProviderContent,
        supported: bool,
    ) -> Result<ProviderContent, RuntimeError> {
        if supported {
            return Ok(content.clone());
        }
        let mut retained = Vec::with_capacity(content.len());
        for part in content.as_slice() {
            match part {
                ProviderContentPart::Text(_) => retained.push(part.clone()),
                ProviderContentPart::Image { .. } if self.image_policy == ImagePolicy::Drop => (),
                ProviderContentPart::Image { .. } => {
                    return Err(invalid("provider image unsupported"));
                }
            }
        }
        ProviderContent::new(retained).map_err(|_| invalid("provider content"))
    }
}
fn measure_wire_with_limit(value: &impl Serialize, max: usize) -> Result<usize, RuntimeError> {
    let mut counter = BoundedCounter { count: 0, max };
    serde_json::to_writer(&mut counter, value).map_err(|_| limit_request())?;
    Ok(counter.count)
}

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
#[cfg(test)]
pub(crate) fn measure_test_value_with_limit(
    value: &impl Serialize,
    max: usize,
) -> Result<usize, RuntimeError> {
    measure_wire_with_limit(value, max)
}

#[cfg(test)]
pub(crate) fn checked_count_for_test(
    current: usize,
    amount: usize,
    max: usize,
) -> Result<usize, RuntimeError> {
    let next = current.checked_add(amount).ok_or_else(limit_request)?;
    if next > max {
        return Err(limit_request());
    }
    Ok(next)
}

fn limit_request() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: PROVIDER_REQUEST_BYTES_MAX.name.into(),
    }
}

/// Stable, scrubbed context attached to a normalized provider failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderErrorContext {
    /// Stable machine-readable code with vendor secrets removed.
    pub code: ProviderName,
    /// Bounded safe diagnostic context; credentials and secret metadata are forbidden.
    pub context: ProviderEventText,
    /// Optional provider-directed retry timing preserved by normalization.
    pub retry_after: Option<crate::retry::RetryAfter>,
}

impl ProviderErrorContext {
    /// Creates scrubbed adapter context with no provider-directed delay.
    #[must_use]
    pub const fn new(code: ProviderName, context: ProviderEventText) -> Self {
        Self {
            code,
            context,
            retry_after: None,
        }
    }

    /// Preserves a normalized retry-after value supplied by an adapter mapper.
    #[must_use]
    pub const fn with_retry_after(mut self, retry_after: crate::retry::RetryAfter) -> Self {
        self.retry_after = Some(retry_after);
        self
    }
}

/// Stable provider failure taxonomy; vendor error types never cross this boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    /// Provider rejected supplied credentials.
    Authentication(ProviderErrorContext),
    /// Authenticated principal lacks permission.
    Authorization(ProviderErrorContext),
    /// Normalized request is invalid for the provider.
    InvalidRequest(ProviderErrorContext),
    /// Provider rate limit was reached.
    RateLimit(ProviderErrorContext),
    /// Account quota was exhausted.
    Quota(ProviderErrorContext),
    /// Provider operation exceeded its deadline.
    Timeout(ProviderErrorContext),
    /// Request exceeded provider context capacity.
    ContextOverflow(ProviderErrorContext),
    /// Provider is temporarily overloaded.
    Overloaded(ProviderErrorContext),
    /// Provider service or route is unavailable.
    Unavailable(ProviderErrorContext),
    /// Provider response violated its protocol.
    Protocol(ProviderErrorContext),
    /// Request was cancelled before completion.
    Cancelled(ProviderErrorContext),
    /// Failure cannot be assigned another stable kind.
    Unknown(ProviderErrorContext),
}

impl ProviderError {
    /// Converts adapter-facing error kinds into canonical retry categories.
    #[must_use]
    pub fn into_failure(self) -> crate::retry::ProviderFailure {
        use crate::retry::{ProviderFailure, ProviderFailureKind};
        let (kind, context) = match self {
            Self::Authentication(value) | Self::Authorization(value) => {
                (ProviderFailureKind::Auth, value)
            }
            Self::InvalidRequest(value) => (ProviderFailureKind::Invalid, value),
            Self::RateLimit(value) | Self::Overloaded(value) => (ProviderFailureKind::Busy, value),
            Self::Timeout(value) | Self::Unavailable(value) => {
                (ProviderFailureKind::Transient, value)
            }
            Self::Protocol(value) => (ProviderFailureKind::Schema, value),
            Self::Quota(value)
            | Self::ContextOverflow(value)
            | Self::Cancelled(value)
            | Self::Unknown(value) => (ProviderFailureKind::Terminal, value),
        };
        let failure = ProviderFailure::new(
            kind,
            format!("{}: {}", context.code.as_str(), context.context.as_str()),
        );
        match context.retry_after {
            Some(value) => failure.with_retry_after(value),
            None => failure,
        }
    }
}

impl From<ProviderError> for crate::retry::ProviderFailure {
    fn from(value: ProviderError) -> Self {
        value.into_failure()
    }
}

/// Stable provider tool-call identifier shared by start, deltas, and end.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ToolCallId(ProviderName);
impl ToolCallId {
    /// Wraps an already bounded provider name as a call identifier.
    #[must_use]
    pub const fn from_name(value: ProviderName) -> Self {
        Self(value)
    }
    /// Borrows the stable identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
/// Normalized reason for the sole successful terminal event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// Model completed the turn normally.
    EndTurn,
    /// Configured output limit ended generation.
    OutputLimit,
    /// Model stopped to request tool execution.
    ToolUse,
    /// Provider content policy ended generation.
    ContentFilter,
    /// Provider supplied no more specific stable reason.
    Other,
}

/// Classified source metadata supplied for sanitization at the adapter boundary.
pub enum ProviderMetadataInput {
    /// Candidate continuation metadata permitted to persist after secret-key inspection.
    Persist {
        /// Bounded metadata key.
        key: ProviderName,
        /// Bounded value inspected iteratively before retention.
        value: BoundedJsonValue,
    },
    /// Secret metadata that must be discarded and never enter normalized events or traces.
    Secret {
        /// Bounded source key, retained only while classifying this input.
        key: ProviderName,
        /// Bounded secret value, discarded without formatting or persistence.
        value: BoundedJsonValue,
    },
}
impl fmt::Debug for ProviderMetadataInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persist { key, .. } => f
                .debug_struct("Persist")
                .field("key", key)
                .field("value", &"[REDACTED]")
                .finish(),
            Self::Secret { key, .. } => f
                .debug_struct("Secret")
                .field("key", key)
                .field("value", &"[REDACTED]")
                .finish(),
        }
    }
}
/// Sanitized persistent provider continuation metadata.
///
/// Values are private so only classified, non-secret entries can enter normalized events.
#[derive(Clone, PartialEq)]
pub struct ProviderMetadata(BTreeMap<ProviderName, BoundedJsonValue>);
impl fmt::Debug for ProviderMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<_> = self.0.keys().map(ProviderName::as_str).collect();
        f.debug_struct("ProviderMetadata")
            .field("keys", &keys)
            .field("values", &"[REDACTED]")
            .finish()
    }
}
impl ProviderMetadata {
    /// Classifies source entries, discarding secrets and retaining safe persistent values.
    ///
    /// # Errors
    /// Returns invalid data when a purported persistent key or nested value looks secret.
    pub fn classified(
        values: impl IntoIterator<Item = ProviderMetadataInput>,
    ) -> Result<Self, RuntimeError> {
        let mut retained = BTreeMap::new();
        for input in values {
            if let ProviderMetadataInput::Persist { key, value } = input {
                if secret_key(key.as_str()) || json_has_secret_key(value.as_value()) {
                    return Err(invalid("provider metadata secret"));
                }
                retained.insert(key, value);
            }
        }
        Ok(Self(retained))
    }
    /// Borrows a retained safe value by key.
    #[must_use]
    pub fn get(&self, key: &ProviderName) -> Option<&serde_json::Value> {
        self.0.get(key).map(BoundedJsonValue::as_value)
    }
}
fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "api_key",
        "apikey",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|n| key.contains(n))
}
fn json_has_secret_key(value: &serde_json::Value) -> bool {
    let mut work = vec![value];
    while let Some(current) = work.pop() {
        match current {
            serde_json::Value::Array(values) => work.extend(values),
            serde_json::Value::Object(values) => {
                if values.keys().any(|key| secret_key(key)) {
                    return true;
                }
                work.extend(values.values());
            }
            _ => {}
        }
    }
    false
}

/// Runtime-owned incremental tool-argument buffer keyed by stable call identifier.
///
/// Bytes are accepted without requiring each chunk to be UTF-8 or JSON. The complete bounded value
/// is parsed only at end; every protocol, limit, or terminal failure clears all retained state.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    active: BTreeMap<ToolCallId, Vec<u8>>,
    ended: BTreeSet<ToolCallId>,
    seen: BTreeSet<ToolCallId>,
}
/// Descriptive alias for the runtime's bounded incremental tool-call accumulator.
pub type ToolArgumentBuffer = ToolCallAccumulator;
impl ToolCallAccumulator {
    /// Starts buffering one previously unseen call identifier.
    ///
    /// # Errors
    /// Rejects duplicate/replayed identifiers and clears all buffered call state.
    pub fn start(&mut self, id: ToolCallId) -> Result<(), RuntimeError> {
        if self.seen.contains(&id) {
            return self.fail("duplicate provider tool call");
        }
        self.seen.insert(id.clone());
        self.active.insert(id, Vec::new());
        Ok(())
    }
    /// Appends raw source bytes without parsing, reserving only after the aggregate limit check.
    ///
    /// # Errors
    /// Rejects unknown or ended calls, arithmetic/allocator failure, or aggregate arguments above
    /// 4 MiB. Every error clears all buffered call state.
    pub fn append(&mut self, id: &ToolCallId, chunk: &[u8]) -> Result<(), RuntimeError> {
        if self.ended.contains(id) {
            return self.fail("provider event for ended tool call");
        }
        let Some(value) = self.active.get_mut(id) else {
            return self.fail("unknown provider tool call");
        };
        let remaining = TOOL_ARGUMENT_BYTES_MAX
            .value
            .checked_sub(value.len())
            .ok_or_else(limit_tool)?;
        if chunk.len() > remaining {
            return self.fail_limit();
        }
        value.try_reserve(chunk.len()).map_err(|_| limit_tool())?;
        value.extend_from_slice(chunk);
        Ok(())
    }
    /// Ends a call and parses its complete bytes as one bounded JSON value.
    ///
    /// # Errors
    /// Rejects unknown/replayed ends or malformed/unbounded JSON and clears all buffered state.
    pub fn end(&mut self, id: &ToolCallId) -> Result<BoundedJsonValue, RuntimeError> {
        if self.ended.contains(id) {
            return self.fail("duplicate provider tool call end");
        }
        let Some(bytes) = self.active.remove(id) else {
            return self.fail("unknown provider tool call");
        };
        let Ok(parsed) = serde_json::from_slice::<BoundedJsonValue>(&bytes) else {
            return self.fail("provider tool arguments JSON");
        };
        self.ended.insert(id.clone());
        Ok(parsed)
    }
    /// Discards all active, ended, and seen call state.
    pub fn clear(&mut self) {
        self.active.clear();
        self.ended.clear();
        self.seen.clear();
    }
    /// Accepts successful termination only when no call remains incomplete, then clears state.
    ///
    /// # Errors
    /// Rejects a stop with any active call and clears all buffered state.
    pub fn terminal_stop(&mut self) -> Result<(), RuntimeError> {
        if self.active.is_empty() {
            self.clear();
            Ok(())
        } else {
            self.fail("provider tool call incomplete at stop")
        }
    }
    /// Clears all buffered state after a terminal provider error or validation failure.
    pub fn terminal_error(&mut self) {
        self.clear();
    }
    fn fail<T>(&mut self, c: &'static str) -> Result<T, RuntimeError> {
        self.clear();
        Err(invalid(c))
    }
    fn fail_limit<T>(&mut self) -> Result<T, RuntimeError> {
        self.clear();
        Err(limit_tool())
    }
    #[cfg(test)]
    pub(crate) fn active_capacity_for_test(&self, id: &ToolCallId) -> usize {
        self.active.get(id).map_or(0, Vec::capacity)
    }
}
fn limit_tool() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: TOOL_ARGUMENT_BYTES_MAX.name.into(),
    }
}

/// Opaque adapter-owned half of a bounded provider event channel.
///
/// `send` applies backpressure by awaiting capacity. Cancellation wins whenever it and channel
/// progress are simultaneously ready. A send that linearizes immediately before cancellation may
/// enqueue an event, but the matching receiver's pre/post-dequeue checks suppress that event.
#[derive(Clone)]
pub struct ProviderEventSink {
    sender: Sender<ProviderEvent>,
    cancellation: CancellationToken,
}

/// Opaque runtime-owned half of a bounded provider event channel.
///
/// `receive` awaits data without polling. Cancellation is terminal, wins ties, and drains queued
/// or racing events. No event dequeued after the request token is cancelled is returned.
pub struct ProviderEventReceiver {
    receiver: Receiver<ProviderEvent>,
    cancellation: CancellationToken,
}

/// Creates opaque channel halves tied to one request cancellation token.
///
/// # Errors
/// Rejects zero capacity or capacity above the provider stream event limit.
pub fn provider_event_channel(
    capacity: usize,
    cancellation: &CancellationToken,
) -> Result<(ProviderEventSink, ProviderEventReceiver), RuntimeError> {
    if capacity == 0 || capacity > PROVIDER_STREAM_EVENTS_MAX.value {
        return Err(invalid("provider event channel capacity"));
    }
    let (sender, receiver) = mpsc::channel(capacity);
    Ok((
        ProviderEventSink {
            sender,
            cancellation: cancellation.clone(),
        },
        ProviderEventReceiver {
            receiver,
            cancellation: cancellation.clone(),
        },
    ))
}

impl ProviderEventSink {
    /// Sends one event, awaiting bounded capacity while the request remains active.
    ///
    /// # Errors
    /// Returns cancellation if the request token wins, or adapter failure if the receiver closed.
    pub async fn send(&self, event: ProviderEvent) -> Result<(), RuntimeError> {
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(cancelled()),
            result = self.sender.send(event) => result.map_err(|_| channel_closed()),
        }
    }

    /// Reports whether request cancellation has begun.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl fmt::Debug for ProviderEventSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderEventSink")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl ProviderEventReceiver {
    /// Receives one event, awaits cancellation, or reports clean channel closure.
    ///
    /// Cancellation is terminal and suppresses all queued or racing events.
    ///
    /// # Errors
    /// Returns cancellation once the shared request token is cancelled.
    pub async fn receive(&mut self) -> Result<Option<ProviderEvent>, RuntimeError> {
        let event = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                self.drain();
                return Err(cancelled());
            }
            event = self.receiver.recv() => event,
        };
        if self.cancellation.is_cancelled() {
            self.drain();
            return Err(cancelled());
        }
        Ok(event)
    }

    /// Cancels the request and synchronously discards currently queued events.
    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        self.drain();
    }

    /// Reports whether cancellation is terminal for this receiver.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    fn drain(&mut self) {
        while self.receiver.try_recv().is_ok() {}
    }
}

fn channel_closed() -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "provider_event_channel_closed",
        context: "provider stream".into(),
    }
}

fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "provider stream".into(),
    }
}

/// Object-safe provider adapter boundary.
///
/// Implementations own no channel half beyond `stream`, must normalize vendor events before
/// sending, await bounded backpressure through `ProviderEventSink::send`, and stop after a terminal
/// event or cancellation. The request and sink share one private cancellation token. An adapter
/// performs exactly one transport attempt; retry and fallback belong exclusively to the runtime.
pub trait ProviderPort: Send + Sync {
    /// Streams one normalized request into the opaque bounded event sink.
    ///
    /// # Errors
    /// Returns normalized adapter, validation, channel-closure, deadline, or cancellation errors.
    fn stream(&self, request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()>;
}

#[cfg(test)]
#[path = "provider_tests.rs"]
pub(crate) mod contract;
