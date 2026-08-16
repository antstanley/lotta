use lotta_domain::Timestamp;
use serde::{Deserialize, Serialize};

/// TypeScript-compatible durable compiled-system-prompt record.
///
/// Unknown fields are rejected so model, tool, skill, and rendered-hash inputs cannot silently
/// become durable authority. Optional fields are omitted when absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompiledPromptRecord {
    /// Complete base system prompt delivered at a provider request boundary.
    pub content: String,
    /// Freshly rendered committed memory and runtime metadata.
    pub core_memory: String,
    /// One-shot delivery payload; cache persistence clears this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_conversation_system_prompt: Option<String>,
    /// Injected compilation instant.
    #[serde(with = "milliseconds_timestamp")]
    pub compiled_at: Timestamp,
    /// Lowercase SHA-256 of the raw managed/custom system text.
    pub raw_system_hash: String,
    /// Exact committed `MemFS` revision used by compilation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memfs_revision: Option<String>,
}

impl CompiledPromptRecord {
    pub(crate) fn stable(mut self) -> Self {
        self.mid_conversation_system_prompt = None;
        self
    }
}

mod milliseconds_timestamp {
    use lotta_domain::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &Timestamp, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            &value
                .as_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Timestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Timestamp::parse_persisted_rfc3339(&value).map_err(serde::de::Error::custom)
    }
}
