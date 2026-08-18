use super::PortFuture;
use super::ToolCallId;
use crate::RuntimeError;
use crate::bounds::{
    EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_DESCRIPTION_BYTES_MAX, TOOL_INPUT_BYTES_MAX,
    TOOL_NAME_BYTES_MAX, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX,
    TOOL_SECRET_FIELDS_ITEMS_MAX,
};
use lotta_domain::{BoundedJsonValue, BoundedVec};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::io::{self, Write};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn limit(name: &'static str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: name.into(),
    }
}

macro_rules! tool_text {
    ($name:ident, $bound:ident, $empty:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            /// Validates and stores the purpose-specific text.
            ///
            /// # Errors
            /// Rejects empty input when prohibited or input above its named byte bound.
            pub fn new(value: String) -> Result<Self, RuntimeError> {
                if !$empty && value.is_empty() {
                    return Err(invalid(stringify!($name)));
                }
                if value.len() > $bound.value {
                    return Err(limit($bound.name));
                }
                Ok(Self(value))
            }
            /// Borrows the validated text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }
    };
}

tool_text!(
    InternalToolName,
    TOOL_NAME_BYTES_MAX,
    false,
    "A bounded non-empty stable internal tool name."
);
tool_text!(
    ModelFacingToolName,
    TOOL_NAME_BYTES_MAX,
    false,
    "A bounded resolved model-facing name; Task 32 owns toolset maps."
);
tool_text!(
    ToolDescriptionAsset,
    TOOL_DESCRIPTION_BYTES_MAX,
    true,
    "A bounded description asset retained by one tool definition."
);
tool_text!(
    PermissionAction,
    TOOL_NAME_BYTES_MAX,
    false,
    "A bounded permission action label interpreted by a later policy engine."
);
tool_text!(
    ParallelCertificationId,
    TOOL_NAME_BYTES_MAX,
    false,
    "A bounded explicit identifier for parallel-safety evidence."
);
/// A bounded absolute JSON Pointer identifying one secret-bearing input field.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SecretFieldPath(String);
impl SecretFieldPath {
    /// Validates a nonempty absolute JSON Pointer with nonempty segments.
    ///
    /// # Errors
    /// Rejects relative/empty pointers, empty segments, malformed escapes, NUL, or overbound input.
    pub fn new(value: String) -> Result<Self, RuntimeError> {
        if value.is_empty() || value.len() > TOOL_NAME_BYTES_MAX.value || value.contains('\0') {
            return Err(if value.len() > TOOL_NAME_BYTES_MAX.value {
                limit(TOOL_NAME_BYTES_MAX.name)
            } else {
                invalid("SecretFieldPath")
            });
        }
        if !value.starts_with('/') || value[1..].split('/').any(str::is_empty) {
            return Err(invalid("SecretFieldPath"));
        }
        for segment in value[1..].split('/') {
            let bytes = segment.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == b'~' {
                    if index + 1 >= bytes.len() || !matches!(bytes[index + 1], b'0' | b'1') {
                        return Err(invalid("SecretFieldPath"));
                    }
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
        Ok(Self(value))
    }
    /// Borrows the validated JSON Pointer.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for SecretFieldPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Debug for SecretFieldPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretFieldPath")
            .field(&self.0)
            .finish()
    }
}
tool_text!(
    ToolOutcomeCode,
    TOOL_NAME_BYTES_MAX,
    false,
    "A bounded normalized tool outcome code."
);
tool_text!(
    ToolOutcomeMessage,
    TOOL_DESCRIPTION_BYTES_MAX,
    true,
    "A bounded normalized tool outcome message."
);

/// Bounded JSON Schema retained by a tool definition.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ToolInputSchema(BoundedJsonValue);
impl ToolInputSchema {
    /// Validates the exact compact schema JSON before retention.
    ///
    /// # Errors
    /// Rejects non-object schema shapes or schema JSON above the tool-input byte ceiling.
    pub fn new(value: BoundedJsonValue) -> Result<Self, RuntimeError> {
        measure_json(value.as_value(), TOOL_INPUT_BYTES_MAX.value)?;
        validate_schema_shape(value.as_value())?;
        Ok(Self(value))
    }
    /// Borrows the validated schema JSON.
    #[must_use]
    pub fn as_value(&self) -> &serde_json::Value {
        self.0.as_value()
    }
}
impl<'de> Deserialize<'de> for ToolInputSchema {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BoundedJsonValue::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Component that owns execution of a tool definition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionOwner {
    /// Native Rust executor.
    Rust,
    /// Model Context Protocol executor.
    Mcp,
    /// Controller-owned external executor.
    Controller,
    /// Isolated mod compatibility sidecar.
    ModSidecar,
    /// Channel gateway executor.
    ChannelGateway,
}

/// Minimal approval policy carried to a later approval stage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalPolicy {
    /// No interactive approval is requested by definition policy.
    Never,
    /// An approval stage must ask before execution.
    Always,
}

/// Certification required before any future scheduler may execute a tool in parallel.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "certification_id", rename_all = "snake_case")]
pub enum ParallelSafety {
    /// Calls must execute sequentially.
    #[default]
    Sequential,
    /// Calls may execute in parallel only under the named bounded certification evidence.
    CertifiedParallel(ParallelCertificationId),
}
/// Policy applied to the secret-bearing fields named by a tool definition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretRedactionPolicy {
    /// Replace retained/logged values with a fixed redaction marker.
    Redact,
    /// Omit named fields from retained/logged projections.
    Omit,
}

/// Bounded unique secret-field paths and their one redaction policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SecretRedactionSpec {
    fields: BoundedVec<SecretFieldPath, { TOOL_SECRET_FIELDS_ITEMS_MAX.value }>,
    policy: SecretRedactionPolicy,
}
impl SecretRedactionSpec {
    /// Creates one bounded, duplicate-free secret-redaction specification.
    ///
    /// # Errors
    /// Rejects duplicate exact JSON Pointer paths.
    pub fn new(
        fields: BoundedVec<SecretFieldPath, { TOOL_SECRET_FIELDS_ITEMS_MAX.value }>,
        policy: SecretRedactionPolicy,
    ) -> Result<Self, RuntimeError> {
        let values = fields.as_slice();
        for (index, field) in values.iter().enumerate() {
            if values[..index].contains(field) {
                return Err(invalid("duplicate secret field path"));
            }
        }
        Ok(Self { fields, policy })
    }
    /// Borrows bounded secret-bearing field paths.
    #[must_use]
    pub fn fields(&self) -> &[SecretFieldPath] {
        self.fields.as_slice()
    }
    /// Returns the redaction policy.
    #[must_use]
    pub const fn policy(&self) -> SecretRedactionPolicy {
        self.policy
    }
}
impl<'de> Deserialize<'de> for SecretRedactionSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            fields: BoundedVec<SecretFieldPath, { TOOL_SECRET_FIELDS_ITEMS_MAX.value }>,
            policy: SecretRedactionPolicy,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.fields, wire.policy).map_err(serde::de::Error::custom)
    }
}

/// Validated generic tool timeout not exceeding the five-minute external-call maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolTimeout(Duration);
impl ToolTimeout {
    /// Validates a positive timeout at or below 300,000 milliseconds.
    ///
    /// # Errors
    /// Rejects zero, sub-millisecond, or above-maximum durations.
    pub fn new(value: Duration) -> Result<Self, RuntimeError> {
        let millis = value.as_millis();
        if value.is_zero() || millis == 0 {
            return Err(invalid("tool timeout"));
        }
        if millis > EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u128 {
            return Err(limit(EXTERNAL_TOOL_CALL_TIMEOUT_MS.name));
        }
        Ok(Self(value))
    }

    /// Validates a positive shell-only timeout at or below 3,600,000 milliseconds.
    ///
    /// # Errors
    /// Rejects zero, sub-millisecond, or above-shell-maximum durations.
    pub fn new_shell(value: Duration) -> Result<Self, RuntimeError> {
        const SHELL_TOOL_TIMEOUT_MS_MAX: u128 = 3_600_000;
        let millis = value.as_millis();
        if value.is_zero() || millis == 0 {
            return Err(invalid("shell tool timeout"));
        }
        if millis > SHELL_TOOL_TIMEOUT_MS_MAX {
            return Err(limit("SHELL_TOOL_TIMEOUT_MS_MAX"));
        }
        Ok(Self(value))
    }
    /// Returns the validated timeout.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

/// Independent byte and Unicode-scalar ceilings for one retained tool result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ToolOutputLimit {
    bytes_max: usize,
    model_chars_max: usize,
}
impl ToolOutputLimit {
    /// Validates nonzero byte and model-facing Unicode scalar limits.
    ///
    /// # Errors
    /// Rejects zero or values above the canonical byte/character ceilings.
    pub fn new(bytes_max: usize, model_chars_max: usize) -> Result<Self, RuntimeError> {
        if bytes_max == 0 || model_chars_max == 0 {
            return Err(invalid("tool output limit"));
        }
        if bytes_max > TOOL_RESULT_BYTES_MAX.value {
            return Err(limit(TOOL_RESULT_BYTES_MAX.name));
        }
        if model_chars_max > TOOL_RESULT_MODEL_CHARS_MAX.value {
            return Err(limit(TOOL_RESULT_MODEL_CHARS_MAX.name));
        }
        Ok(Self {
            bytes_max,
            model_chars_max,
        })
    }
    /// Returns the byte ceiling.
    #[must_use]
    pub const fn bytes_max(self) -> usize {
        self.bytes_max
    }
    /// Returns the Unicode scalar ceiling used for model-facing text.
    #[must_use]
    pub const fn model_chars_max(self) -> usize {
        self.model_chars_max
    }
}
impl<'de> Deserialize<'de> for ToolOutputLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            bytes_max: usize,
            model_chars_max: usize,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.bytes_max, wire.model_chars_max).map_err(serde::de::Error::custom)
    }
}

/// Tool definition with exactly eleven public fields.
///
/// The specification's grouped bullets count names separately, approval and permission separately,
/// timeout and output separately, and combine secret fields plus policy into one specification.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    /// Stable internal name.
    pub internal_name: InternalToolName,
    /// Already-resolved model-facing name; toolset mappings belong to Task 32.
    pub model_name: ModelFacingToolName,
    /// Bounded JSON Schema input definition.
    pub input_schema: ToolInputSchema,
    /// Bounded description asset.
    pub description: ToolDescriptionAsset,
    /// Execution owner.
    pub execution_owner: ToolExecutionOwner,
    /// Approval policy.
    pub approval_policy: ToolApprovalPolicy,
    /// Permission action label; no permission behavior is implemented here.
    pub permission_action: PermissionAction,
    /// Parallel-safety classification.
    pub parallel_safety: ParallelSafety,
    /// Validated execution timeout.
    pub timeout: ToolTimeout,
    /// Independent result byte and model-character limits.
    pub output_limit: ToolOutputLimit,
    /// One bounded secret-field and redaction-policy specification.
    pub secret_redaction: SecretRedactionSpec,
}
impl ToolDefinition {
    /// Constructs a definition with conservative sequential execution.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "the exact typed contract intentionally exposes ten required constructor inputs"
    )]
    pub const fn new(
        internal_name: InternalToolName,
        model_name: ModelFacingToolName,
        input_schema: ToolInputSchema,
        description: ToolDescriptionAsset,
        execution_owner: ToolExecutionOwner,
        approval_policy: ToolApprovalPolicy,
        permission_action: PermissionAction,
        timeout: ToolTimeout,
        output_limit: ToolOutputLimit,
        secret_redaction: SecretRedactionSpec,
    ) -> Self {
        Self {
            internal_name,
            model_name,
            input_schema,
            description,
            execution_owner,
            approval_policy,
            permission_action,
            parallel_safety: ParallelSafety::Sequential,
            timeout,
            output_limit,
            secret_redaction,
        }
    }
    /// Replaces sequential execution with explicit bounded certification evidence.
    #[must_use]
    pub fn with_parallel_certification(mut self, id: ParallelCertificationId) -> Self {
        self.parallel_safety = ParallelSafety::CertifiedParallel(id);
        self
    }
}

/// Validated owning tool input whose exact compact JSON is at most 4 MiB.
#[derive(Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ValidatedToolInput(BoundedJsonValue);
impl ValidatedToolInput {
    /// Validates exact compact JSON size without allocating a measurement buffer.
    ///
    /// # Errors
    /// Rejects malformed domain values or compact JSON above 4 MiB.
    pub fn new(value: BoundedJsonValue) -> Result<Self, RuntimeError> {
        measure_json(value.as_value(), TOOL_INPUT_BYTES_MAX.value)?;
        Ok(Self(value))
    }
    /// Borrows validated input for an adapter implementation.
    #[must_use]
    pub fn as_value(&self) -> &serde_json::Value {
        self.0.as_value()
    }
}
impl<'de> Deserialize<'de> for ValidatedToolInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BoundedJsonValue::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Debug for ValidatedToolInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedToolInput([REDACTED])")
    }
}

/// Internal proof that one exact tool call received interactive approval.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ToolApprovalGrant {
    /// No interactive approval was granted for this execution.
    #[default]
    None,
    /// Approval was granted for this exact call identity and definition.
    Granted {
        /// Provider-generated call identity.
        tool_call_id: ToolCallId,
        /// Stable internal tool identity approved by the manager.
        internal_name: InternalToolName,
    },
}

impl ToolApprovalGrant {
    /// Mints a grant after the approval manager has claimed execution.
    #[must_use]
    pub(crate) fn granted(tool_call_id: ToolCallId, definition: &ToolDefinition) -> Self {
        Self::Granted {
            tool_call_id,
            internal_name: definition.internal_name.clone(),
        }
    }

    /// Returns whether this grant names the exact current call and definition.
    #[must_use]
    pub fn matches(&self, tool_call_id: &ToolCallId, definition: &ToolDefinition) -> bool {
        matches!(
            self,
            Self::Granted { tool_call_id: granted_call, internal_name }
                if granted_call == tool_call_id && internal_name == &definition.internal_name
        )
    }
}

/// Owning execution request passed across the tool port.
///
/// The definition identifies and attributes external execution. Input is schema-validated before
/// construction, cancellation is explicit and terminal, and the adapter must enforce the deadline.
#[derive(Clone)]
pub struct ToolExecutionRequest {
    /// Genuine provider invocation identity.
    pub tool_call_id: ToolCallId,
    /// Internal exact-call approval proof, or no approval.
    pub approval_grant: ToolApprovalGrant,
    /// Full definition needed by external owners.
    pub definition: ToolDefinition,
    /// Validated bounded tool input.
    pub input: ValidatedToolInput,
    /// Explicit terminal cancellation token.
    pub cancellation: CancellationToken,
    /// Positive duration bounded by the external-tool maximum.
    pub deadline: ToolTimeout,
}
impl fmt::Debug for ToolExecutionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolExecutionRequest")
            .field("tool_call_id", &self.tool_call_id)
            .field("approval_grant", &self.approval_grant)
            .field("definition", &self.definition)
            .field("input", &"[REDACTED]")
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("deadline", &self.deadline)
            .finish()
    }
}

/// Bounded UTF-8 tool text measured in bytes and Unicode scalar values before retention.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResultText {
    value: String,
    applied_limit: ToolOutputLimit,
}
impl ToolResultText {
    /// Validates bytes and Unicode scalar count against the supplied output limits.
    ///
    /// # Errors
    /// Rejects text above either ceiling before retaining it.
    pub fn new(value: String, output_limit: ToolOutputLimit) -> Result<Self, RuntimeError> {
        if value.len() > output_limit.bytes_max() || value.len() > TOOL_RESULT_BYTES_MAX.value {
            return Err(limit(TOOL_RESULT_BYTES_MAX.name));
        }
        let chars = value.chars().count();
        if chars > output_limit.model_chars_max() || chars > TOOL_RESULT_MODEL_CHARS_MAX.value {
            return Err(limit(TOOL_RESULT_MODEL_CHARS_MAX.name));
        }
        Ok(Self {
            value,
            applied_limit: output_limit,
        })
    }
    /// Borrows validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }
    /// Returns the exact definition-specific limit applied to this text.
    #[must_use]
    pub const fn applied_limit(&self) -> ToolOutputLimit {
        self.applied_limit
    }
}
impl Serialize for ToolResultText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            value: &'a str,
            applied_limit: ToolOutputLimit,
        }
        Wire {
            value: &self.value,
            applied_limit: self.applied_limit,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ToolResultText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            value: String,
            applied_limit: ToolOutputLimit,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.value, wire.applied_limit).map_err(serde::de::Error::custom)
    }
}

/// Stable conversation-data result of one tool execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutcome {
    /// Tool completed successfully.
    Success {
        /// Bounded model-facing success content.
        content: ToolResultText,
    },
    /// User explicitly denied execution.
    UserDenied {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// Explicit cancellation interrupted execution.
    Interruption {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// Tool execution exceeded its deadline, distinct from provider timeout.
    Timeout {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// Input failed tool validation.
    ValidationFailure {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// Sandbox policy denied execution.
    SandboxDenied {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// An executor could not spawn.
    SpawnFailure {
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
    /// The tool returned its own normalized error code and message.
    ToolDefinedError {
        /// Stable bounded tool-defined code.
        code: ToolOutcomeCode,
        /// Bounded normalized explanation.
        message: ToolOutcomeMessage,
    },
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
            .ok_or_else(|| io::Error::other("tool JSON size overflow"))?;
        if next > self.max {
            return Err(io::Error::other("tool JSON size limit"));
        }
        self.count = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn validate_schema_shape(value: &serde_json::Value) -> Result<(), RuntimeError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("tool input schema must be an object"))?;
    if object
        .get("type")
        .is_some_and(|value| value.as_str() != Some("object"))
    {
        return Err(invalid("tool input schema type must be object"));
    }
    let properties = match object.get("properties") {
        Some(value) => Some(
            value
                .as_object()
                .ok_or_else(|| invalid("tool input schema properties must be an object"))?,
        ),
        None => None,
    };
    if let Some(value) = object.get("required") {
        let required = value
            .as_array()
            .ok_or_else(|| invalid("tool input schema required must be a string array"))?;
        for (index, item) in required.iter().enumerate() {
            let name = item
                .as_str()
                .ok_or_else(|| invalid("tool input schema required must be a string array"))?;
            if required[..index]
                .iter()
                .any(|previous| previous.as_str() == Some(name))
            {
                return Err(invalid("tool input schema required names must be unique"));
            }
            if properties.is_some_and(|properties| !properties.contains_key(name)) {
                return Err(invalid(
                    "tool input schema required name must exist in properties",
                ));
            }
        }
    }
    Ok(())
}

fn measure_json(value: &serde_json::Value, max: usize) -> Result<usize, RuntimeError> {
    let mut counter = BoundedCounter { count: 0, max };
    serde_json::to_writer(&mut counter, value).map_err(|_| limit(TOOL_INPUT_BYTES_MAX.name))?;
    Ok(counter.count)
}

/// Object-safe boundary implemented by Rust, MCP, controller, mod, and channel adapters.
///
/// Callers own schema validation and request construction. Implementations own execution after
/// transfer, inspect input only through its validated accessor, honor cancellation and deadline,
/// and return normalized outcomes. No routing, approval, permission, sandbox, or retry is implied.
pub trait ToolPort: Send + Sync {
    /// Executes one owning validated request.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] for port/adapter contract failures; tool failures are outcomes.
    fn execute(&self, request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome>;
}

#[cfg(test)]
#[path = "tool_tests.rs"]
pub(crate) mod tests;
