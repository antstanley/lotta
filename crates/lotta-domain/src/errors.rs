use crate::{IdKind, TurnStateKind};
use thiserror::Error;

/// A domain boundary operation failed validation or a lifecycle invariant.
#[allow(
    missing_docs,
    reason = "variant fields are explained by variant documentation"
)]
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DomainError {
    /// An accepted opaque identifier was empty.
    #[error("{kind} identifier must not be empty")]
    EmptyId { kind: IdKind },
    /// A generated identifier had no suffix after its canonical prefix.
    #[error("generated {kind} identifier must have a suffix after {prefix}")]
    MissingGeneratedIdSuffix { kind: IdKind, prefix: &'static str },
    /// A generated identifier required a UUID v4 suffix.
    #[error("generated {kind} identifier requires a UUID v4 suffix")]
    IdUuidVersion { kind: IdKind },
    /// A sequence-backed identifier used the reserved zero value.
    #[error("generated {kind} sequence must be at least 1")]
    IdSequenceOutOfRange { kind: IdKind },
    /// A required string was empty.
    #[error("non-empty string must contain at least one byte")]
    EmptyString,
    /// A timestamp was not strict RFC3339 UTC.
    #[error("timestamp must be canonical RFC3339 UTC: {value}")]
    InvalidTimestamp { value: String },
    /// A collection exceeded its explicit domain bound.
    #[error("{field} contains {actual} items; maximum is {maximum}")]
    CollectionTooLarge {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    /// An unknown field collided with a canonical field.
    #[error("unknown field collides with canonical field: {field}")]
    ExtraFieldCollision { field: String },
    /// An IANA timezone identifier was invalid.
    #[error("invalid IANA timezone identifier: {value}")]
    InvalidTimezone { value: String },
    /// A stop reason was empty.
    #[error("stop reason must be non-empty")]
    EmptyStopReason,
    /// A requested turn-state transition is not a lifecycle edge.
    #[error("illegal turn transition from {from:?} to {to:?}")]
    TurnTransition {
        from: TurnStateKind,
        to: TurnStateKind,
    },
    /// A lease did not match the current owner's complete token.
    #[error("turn lease is not current for this lifecycle owner")]
    StaleTurnLease,
    /// A lifecycle owner exhausted its lease generation space.
    #[error("turn lease generation exhausted")]
    TurnLeaseGenerationExhausted,
    /// A turn identifier was empty.
    #[error("turn identifier must be non-empty")]
    EmptyTurnId,
}

impl DomainError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyId { .. } => "domain.id.empty",
            Self::MissingGeneratedIdSuffix { .. } => "domain.id.generated_suffix_missing",
            Self::IdUuidVersion { .. } => "domain.id.uuid_version",
            Self::IdSequenceOutOfRange { .. } => "domain.id.sequence_out_of_range",
            Self::EmptyString => "domain.scalar.empty_string",
            Self::InvalidTimestamp { .. } => "domain.scalar.invalid_timestamp",
            Self::CollectionTooLarge { .. } => "domain.entity.collection_too_large",
            Self::ExtraFieldCollision { .. } => "domain.entity.extra_field_collision",
            Self::InvalidTimezone { .. } => "domain.entity.invalid_timezone",
            Self::EmptyStopReason => "domain.runtime.empty_stop_reason",
            Self::TurnTransition { .. } => "domain.runtime.turn_transition",
            Self::StaleTurnLease => "domain.runtime.stale_turn_lease",
            Self::TurnLeaseGenerationExhausted => "domain.runtime.lease_generation_exhausted",
            Self::EmptyTurnId => "domain.runtime.empty_turn_id",
        }
    }

    pub(crate) const fn collection(field: &'static str, actual: usize, maximum: usize) -> Self {
        Self::CollectionTooLarge {
            field,
            actual,
            maximum,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn assert_stable_codes() {
        let errors = [
            DomainError::EmptyId {
                kind: IdKind::Agent,
            },
            DomainError::MissingGeneratedIdSuffix {
                kind: IdKind::Agent,
                prefix: "x",
            },
            DomainError::IdUuidVersion {
                kind: IdKind::Agent,
            },
            DomainError::IdSequenceOutOfRange {
                kind: IdKind::Agent,
            },
            DomainError::EmptyString,
            DomainError::InvalidTimestamp { value: "x".into() },
            DomainError::collection("x", 2, 1),
            DomainError::ExtraFieldCollision { field: "x".into() },
            DomainError::InvalidTimezone { value: "x".into() },
            DomainError::EmptyStopReason,
            DomainError::TurnTransition {
                from: TurnStateKind::Idle,
                to: TurnStateKind::Active,
            },
            DomainError::StaleTurnLease,
            DomainError::TurnLeaseGenerationExhausted,
            DomainError::EmptyTurnId,
        ];
        let mut codes = std::collections::BTreeSet::new();
        for error in &errors {
            assert!(codes.insert(error.code()));
        }
        assert_eq!(codes.len(), errors.len());
    }
}

#[cfg(test)]
#[test]
fn stable_codes() {
    tests::assert_stable_codes();
}
