//! Validated shared scalar values and injected time.
use crate::DomainError;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A string containing at least one byte.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Validates a string without trimming or otherwise rewriting it.
    ///
    /// # Errors
    /// Returns [`DomainError::EmptyString`] when the input has zero bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::EmptyString);
        }
        Ok(Self(value))
    }

    /// Returns the original string bytes.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper without rewriting its value.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// UTC RFC3339 instant serialized in canonical `Z` form.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Captures the current instant exclusively from an injected clock.
    ///
    /// # Panics
    /// Panics if a clock implementation violates the UTC timestamp invariant.
    #[must_use]
    pub fn now(clock: &impl Clock) -> Self {
        let timestamp = clock.now();
        assert_eq!(
            timestamp.0.timezone(),
            Utc,
            "clock timestamp must remain UTC"
        );
        assert!(
            !timestamp.to_string().is_empty(),
            "timestamp encoding must not be empty"
        );
        timestamp
    }

    /// Parses zero-offset RFC3339 text through an injected clock boundary.
    ///
    /// Accepted inputs serialize in canonical UTC `Z` form.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidTimestamp`] unless `value` is valid zero-offset RFC3339.
    pub fn parse(clock: &impl Clock, value: &str) -> Result<Self, DomainError> {
        clock.parse_timestamp(value)
    }

    /// Returns the UTC date-time value.
    #[must_use]
    pub fn as_utc(&self) -> &DateTime<Utc> {
        &self.0
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_rfc3339_utc(&value).map_err(de::Error::custom)
    }
}

/// Source of all domain time creation and parsing.
pub trait Clock {
    /// Returns the clock's current UTC instant.
    fn now(&self) -> Timestamp;

    /// Parses persisted timestamp input under this clock boundary.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidTimestamp`] for invalid input.
    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError>;
}

fn parse_rfc3339_utc(value: &str) -> Result<Timestamp, DomainError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| invalid_timestamp(value))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(invalid_timestamp(value));
    }
    Ok(Timestamp(parsed.with_timezone(&Utc)))
}

fn invalid_timestamp(value: &str) -> DomainError {
    DomainError::InvalidTimestamp {
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }

        fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
            parse_rfc3339_utc(value)
        }
    }

    fn clock() -> FixedClock {
        let timestamp = parse_rfc3339_utc("2026-08-14T12:34:56Z");
        match timestamp {
            Ok(value) => FixedClock(value),
            Err(error) => panic!("fixed test timestamp must be valid: {error}"),
        }
    }

    #[test]
    fn rejects_only_empty_strings() {
        assert_eq!(NonEmptyString::new(""), Err(DomainError::EmptyString));
        assert_eq!(
            NonEmptyString::new(" ").map(NonEmptyString::into_string),
            Ok(" ".into())
        );
        assert_eq!(
            NonEmptyString::new("\n\t").map(NonEmptyString::into_string),
            Ok("\n\t".into())
        );
    }

    #[test]
    fn rfc3339_utc_inputs_normalize_to_z() {
        let zulu = Timestamp::parse(&clock(), "2026-08-14T12:34:56.123Z");
        assert_eq!(
            zulu.map(|timestamp| timestamp.to_string()),
            Ok("2026-08-14T12:34:56.123Z".into())
        );

        let explicit_offset = Timestamp::parse(&clock(), "2026-08-14T12:34:56.123+00:00");
        assert_eq!(
            explicit_offset.map(|timestamp| timestamp.to_string()),
            Ok("2026-08-14T12:34:56.123Z".into())
        );
        assert_eq!(Timestamp::now(&clock()).to_string(), "2026-08-14T12:34:56Z");
    }

    #[test]
    fn rejects_non_utc_and_malformed_timestamps() {
        assert!(Timestamp::parse(&clock(), "2026-08-14T13:34:56+01:00").is_err());
        assert!(Timestamp::parse(&clock(), "not-a-timestamp").is_err());
    }

    proptest! {
        #[test]
        fn accepted_non_empty_strings_preserve_bytes(value in ".{1,256}") {
            let accepted = NonEmptyString::new(value.clone())?.into_string();
            prop_assert_eq!(accepted.as_bytes(), value.as_bytes());
        }
    }
}
