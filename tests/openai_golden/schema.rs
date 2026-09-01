use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const BASELINE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
pub const BASELINE_TREE: &str = "30d2e2cebc7761c153f5a6d136b242365092faa0";
pub const FIXTURE_CASES_MAX: usize = 16;
pub const FIXTURE_BYTES_MAX: usize = 1_048_576;
pub const SSE_EVENTS_MAX: usize = 64;
pub const JSON_DEPTH_MAX: usize = 32;
pub const CHILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub const AUTHORIZATION: &str = "Bearer fixture-transport-credential";
pub const TOKEN_SHA256: &str = "1947de502481746d5dc98a64e8fa1d743d6c3da164f5b031cbf9ee9e0fb05ffb";
pub const FORK_SOURCE_FIELD: &str = "openai_fork_source_conversation_id";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureIndex {
    pub schema_version: u8,
    pub baseline_commit: String,
    pub baseline_tree: String,
    pub capture_command: String,
    pub runtime: RuntimeVersion,
    pub sources: BTreeMap<String, SourcePin>,
    pub bounds: FixtureBounds,
    pub cases: Vec<IndexCase>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeVersion {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourcePin {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureBounds {
    pub cases_max: usize,
    pub fixture_bytes_max: usize,
    pub events_per_case_max: usize,
    pub json_depth_max: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexCase {
    pub name: String,
    pub route: String,
    pub mode: String,
    pub dependencies: Vec<String>,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u8,
    pub name: String,
    pub route: String,
    pub mode: String,
    pub dependencies: Vec<String>,
    pub execution: Execution,
    pub request: FixtureRequest,
    pub expected: Value,
    pub observable: Observable,
    pub relationships: RelationshipMap,
    pub cursor: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureRequest {
    pub method: String,
    pub path: String,
    pub mode: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Execution {
    Single {
        #[serde(default)]
        capture_cursor: bool,
        #[serde(default)]
        previous_cursor: Option<String>,
    },
    Repeat {
        count: usize,
    },
    IdempotentLiveJoin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicKind {
    ChatCompletion,
    StoredResponse,
    Response,
    Msg,
    Fc,
    Fco,
    Rs,
    Uuid,
    Conversation,
    Timestamp,
}

impl DynamicKind {
    pub const ALL: [Self; 10] = [
        Self::ChatCompletion,
        Self::StoredResponse,
        Self::Response,
        Self::Msg,
        Self::Fc,
        Self::Fco,
        Self::Rs,
        Self::Uuid,
        Self::Conversation,
        Self::Timestamp,
    ];

    pub const fn token_name(self) -> &'static str {
        match self {
            Self::ChatCompletion => "CHAT_COMPLETION",
            Self::StoredResponse => "STORED_RESPONSE",
            Self::Response => "RESPONSE",
            Self::Msg => "MSG",
            Self::Fc => "FC",
            Self::Fco => "FCO",
            Self::Rs => "RS",
            Self::Uuid => "UUID",
            Self::Conversation => "CONVERSATION",
            Self::Timestamp => "TIMESTAMP",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RelationshipMap(pub BTreeMap<DynamicKind, Vec<String>>);

impl RelationshipMap {
    pub fn validate(&self) -> Result<(), String> {
        let mut all = std::collections::BTreeSet::new();
        for (kind, tokens) in &self.0 {
            for (index, token) in tokens.iter().enumerate() {
                let expected = format!("<{}_ID_{}>", kind.token_name(), index + 1);
                if token != &expected {
                    return Err(format!("noncontiguous {kind:?} token {token}"));
                }
                if !all.insert(token) {
                    return Err(format!("duplicate relationship token {token}"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observable {
    pub provider_calls: Vec<ProviderCall>,
    pub conversations: ConversationDelta,
    pub cleanup: CleanupDelta,
    pub idempotency: IdempotencyDelta,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderCall {
    pub model: String,
    pub inputs: Vec<String>,
    pub stream: bool,
    pub store: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConversationDelta {
    pub created: Vec<ConversationLink>,
    pub deleted: Vec<String>,
    pub retained: Vec<ConversationLink>,
    pub hidden: Vec<ConversationLink>,
    pub forks: Vec<ForkLink>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConversationLink {
    pub id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub hidden: Option<bool>,
    pub source_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ForkLink {
    pub source_id: String,
    pub target_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CleanupDelta {
    pub ephemeral_deleted: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyPhase {
    NotApplicable,
    OwnerActive,
    LiveJoin,
    SettledReplay,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IdempotencyDelta {
    pub allocations: usize,
    pub admissions: usize,
    pub turns: usize,
    pub provider_calls: usize,
    pub live_joins: usize,
    pub phases: Vec<IdempotencyPhase>,
}
