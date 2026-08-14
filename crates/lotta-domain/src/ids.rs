//! Opaque identifiers and deterministic generation forms.
use crate::DomainError;

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::{Uuid, Version};

const AGENT_PREFIX: &str = "agent-local-";
const CONVERSATION_PREFIX: &str = "local-conv-";
const PROJECTED_MESSAGE_PREFIX: &str = "letta-msg-";
const LOCAL_MESSAGE_PREFIX: &str = "ui-msg-";
const RUN_PREFIX: &str = "local-run-";
const RESPONSE_PREFIX: &str = "resp_letta_";
const SEQUENCE_MIN: u64 = 1;

/// Entity categories used by typed identifier errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdKind {
    /// Agent identifier.
    Agent,
    /// Conversation identifier.
    Conversation,
    /// Message identifier.
    Message,
    /// Run identifier.
    Run,
    /// Stored `OpenAI` response identifier.
    Response,
}

impl fmt::Display for IdKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Agent => "agent",
            Self::Conversation => "conversation",
            Self::Message => "message",
            Self::Run => "run",
            Self::Response => "response",
        };
        formatter.write_str(name)
    }
}

macro_rules! opaque_id {
    ($name:ident, $kind:expr) => {
        #[doc = concat!("Opaque ", stringify!($name), " value serialized as a string.")]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Accepts an opaque non-empty identifier without rewriting it.
            ///
            /// # Errors
            /// Returns [`DomainError::EmptyId`] when the input is empty.
            pub fn accept(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                validate_accepted(&value, $kind)?;
                Ok(Self(value))
            }

            /// Returns the identifier exactly as accepted or generated.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the wrapper without changing the identifier bytes.
            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }
    };
}

opaque_id!(AgentId, IdKind::Agent);
opaque_id!(ConversationId, IdKind::Conversation);
opaque_id!(MessageId, IdKind::Message);
opaque_id!(RunId, IdKind::Run);

impl AgentId {
    /// Generates the canonical local-agent form from an injected UUID v4 value.
    ///
    /// # Errors
    /// Returns [`DomainError::IdUuidVersion`] unless `uuid` is version 4.
    pub fn generate(uuid: Uuid) -> Result<Self, DomainError> {
        if uuid.get_version() != Some(Version::Random) {
            return Err(DomainError::IdUuidVersion {
                kind: IdKind::Agent,
            });
        }
        generate_with_suffix(AGENT_PREFIX, &uuid.to_string(), IdKind::Agent).map(Self)
    }
}

impl ConversationId {
    /// Generates `local-conv-<sequence>` without reading process-global state.
    ///
    /// # Errors
    /// Returns an error when `sequence` is zero.
    pub fn generate(sequence: u64) -> Result<Self, DomainError> {
        generate_sequence(CONVERSATION_PREFIX, sequence, IdKind::Conversation).map(Self)
    }

    /// Accepts the agent-scoped virtual default conversation value.
    #[must_use]
    pub fn default_for_agent() -> Self {
        Self(String::from("default"))
    }

    /// Returns whether this is the agent-scoped virtual default value.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.0 == "default"
    }
}

impl MessageId {
    /// Generates the API-projection `letta-msg-<sequence>` form.
    ///
    /// # Errors
    /// Returns an error when `sequence` is zero.
    pub fn generate_projection(sequence: u64) -> Result<Self, DomainError> {
        generate_sequence(PROJECTED_MESSAGE_PREFIX, sequence, IdKind::Message).map(Self)
    }

    /// Generates the transcript/provider `ui-msg-<sequence>` form.
    ///
    /// # Errors
    /// Returns an error when `sequence` is zero.
    pub fn generate_local(sequence: u64) -> Result<Self, DomainError> {
        generate_sequence(LOCAL_MESSAGE_PREFIX, sequence, IdKind::Message).map(Self)
    }
}

impl RunId {
    /// Generates a canonical local-run form from an injected UUID value.
    ///
    /// Run UUID suffixes remain version-agnostic because the domain specification allows any UUID.
    ///
    /// # Errors
    /// Returns an error if the canonical generated form has no suffix.
    pub fn generate_uuid(uuid: Uuid) -> Result<Self, DomainError> {
        generate_with_suffix(RUN_PREFIX, &uuid.to_string(), IdKind::Run).map(Self)
    }

    /// Generates a deterministic canonical local-run sequence form.
    ///
    /// # Errors
    /// Returns an error when `sequence` is zero.
    pub fn generate_sequence(sequence: u64) -> Result<Self, DomainError> {
        generate_sequence(RUN_PREFIX, sequence, IdKind::Run).map(Self)
    }
}

/// Stored `OpenAI` response identifier, kept separate from entity IDs.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ResponseId(String);

impl ResponseId {
    /// Generates `resp_letta_<cursor>` from an injected, pre-encoded cursor.
    ///
    /// # Errors
    /// Returns an error when `cursor` is empty.
    pub fn generate(cursor: impl Into<String>) -> Result<Self, DomainError> {
        let cursor = cursor.into();
        generate_with_suffix(RESPONSE_PREFIX, &cursor, IdKind::Response).map(Self)
    }

    /// Returns the generated response identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_accepted(value: &str, kind: IdKind) -> Result<(), DomainError> {
    if value.is_empty() {
        return Err(DomainError::EmptyId { kind });
    }
    Ok(())
}

fn generate_sequence(
    prefix: &'static str,
    sequence: u64,
    kind: IdKind,
) -> Result<String, DomainError> {
    if sequence < SEQUENCE_MIN {
        return Err(DomainError::IdSequenceOutOfRange { kind });
    }
    generate_with_suffix(prefix, &sequence.to_string(), kind)
}

fn generate_with_suffix(
    prefix: &'static str,
    suffix: &str,
    kind: IdKind,
) -> Result<String, DomainError> {
    if suffix.is_empty() {
        return Err(DomainError::MissingGeneratedIdSuffix { kind, prefix });
    }
    let generated = format!("{prefix}{suffix}");
    assert!(
        generated.starts_with(prefix),
        "generator must retain its canonical prefix"
    );
    assert!(
        generated.len() > prefix.len(),
        "generator must append a non-empty suffix"
    );
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn generate_matches_prefix() {
        assert_eq!(
            ConversationId::generate(1).map(ConversationId::into_string),
            Ok("local-conv-1".into())
        );
        assert_eq!(
            MessageId::generate_projection(2).map(MessageId::into_string),
            Ok("letta-msg-2".into())
        );
        assert_eq!(
            MessageId::generate_local(3).map(MessageId::into_string),
            Ok("ui-msg-3".into())
        );
        assert_eq!(
            RunId::generate_sequence(4).map(RunId::into_string),
            Ok("local-run-4".into())
        );
        assert_eq!(
            ResponseId::generate("cursor").map(|id| id.0),
            Ok("resp_letta_cursor".into())
        );
        let agent_uuid = Uuid::from_u128(0x123e_4567_e89b_42d3_a456_4266_1417_4000);
        assert_eq!(
            AgentId::generate(agent_uuid).map(AgentId::into_string),
            Ok(format!("agent-local-{agent_uuid}"))
        );
    }

    #[test]
    fn agent_generation_requires_uuid_v4() {
        assert_eq!(
            AgentId::generate(Uuid::nil()),
            Err(DomainError::IdUuidVersion {
                kind: IdKind::Agent
            })
        );
        assert!(RunId::generate_uuid(Uuid::nil()).is_ok());
    }

    #[test]
    fn accepts_arbitrary_baseline_id() {
        let original = "conv/../x\0with spaces";
        let accepted = ConversationId::accept(original);
        assert_eq!(
            accepted.map(ConversationId::into_string),
            Ok(original.into())
        );
    }

    #[test]
    fn rejects_empty_id() {
        assert_eq!(
            ConversationId::accept(""),
            Err(DomainError::EmptyId {
                kind: IdKind::Conversation
            })
        );
    }

    #[test]
    fn sequence_boundaries_are_explicit() {
        assert!(ConversationId::generate(0).is_err());
        assert_eq!(
            ConversationId::generate(1).map(ConversationId::into_string),
            Ok("local-conv-1".into())
        );
        assert_eq!(
            ConversationId::generate(u64::MAX).map(ConversationId::into_string),
            Ok(format!("local-conv-{}", u64::MAX))
        );
    }

    proptest! {
        #[test]
        fn generated_sequence_forms_are_valid(sequence in 1_u64..=u64::MAX) {
            let conversation = ConversationId::generate(sequence)?;
            prop_assert!(conversation.as_str().starts_with(CONVERSATION_PREFIX));

            let projected_message = MessageId::generate_projection(sequence)?;
            prop_assert!(
                projected_message
                    .as_str()
                    .starts_with(PROJECTED_MESSAGE_PREFIX)
            );

            let local_message = MessageId::generate_local(sequence)?;
            prop_assert!(local_message.as_str().starts_with(LOCAL_MESSAGE_PREFIX));

            let run = RunId::generate_sequence(sequence)?;
            prop_assert!(run.as_str().starts_with(RUN_PREFIX));
        }

        #[test]
        fn arbitrary_non_empty_conversations_round_trip(value in ".{1,256}") {
            let accepted = ConversationId::accept(value.clone())?.into_string();
            prop_assert_eq!(accepted.as_bytes(), value.as_bytes());
        }
    }
}
