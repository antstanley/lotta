//! Canonical public provider limits and shared boundary predicates.

/// Maximum configured providers.
pub const PROVIDERS_MAX: usize = 128;
/// Maximum models exposed by one provider.
pub const MODELS_PER_PROVIDER_MAX: usize = 10_000;
/// Maximum normalized provider request bytes.
pub const PROVIDER_REQUEST_BYTES_MAX: usize =
    lotta_runtime::bounds::PROVIDER_REQUEST_BYTES_MAX.value;
/// Maximum one normalized provider response event bytes.
pub const PROVIDER_RESPONSE_EVENT_BYTES_MAX: usize =
    lotta_runtime::bounds::PROVIDER_RESPONSE_EVENT_BYTES_MAX.value;
/// Maximum assembled tool-call argument bytes.
pub const TOOL_ARGUMENT_BYTES_MAX: usize = lotta_runtime::bounds::TOOL_ARGUMENT_BYTES_MAX.value;
/// Default provider operation timeout in milliseconds.
pub const PROVIDER_TIMEOUT_MS_DEFAULT: u64 =
    lotta_runtime::retry::PROVIDER_RETRY_DEADLINE_MS_DEFAULT;
/// Maximum provider retries after the initial attempt.
pub const PROVIDER_RETRIES_MAX: u32 = lotta_runtime::retry::PROVIDER_RETRIES_MAX;
/// Maximum provider retry backoff in milliseconds.
pub const PROVIDER_BACKOFF_MS_MAX: u64 = lotta_runtime::retry::PROVIDER_BACKOFF_MS_MAX;
/// OAuth state lifetime in seconds.
pub const OAUTH_STATE_TTL_SECONDS: u64 = 600;
/// Maximum decoded bytes accepted for each provider image.
pub const IMAGE_BYTES_MAX: usize = lotta_runtime::bounds::PROVIDER_IMAGE_BYTES_MAX.value;

/// Stable provider boundary limit kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LimitError {
    /// The named inclusive ceiling was exceeded.
    #[error("provider limit exceeded: {kind:?}")]
    Exceeded {
        /// Stable machine-readable limit selector.
        kind: LimitKind,
    },
    /// Checked accumulation overflowed the machine integer.
    #[error("provider limit arithmetic overflow: {kind:?}")]
    Arithmetic {
        /// Stable machine-readable limit selector.
        kind: LimitKind,
    },
}

/// Named production provider limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitKind {
    /// Provider count.
    Providers,
    /// Models listed by one provider.
    ModelsPerProvider,
    /// Serialized normalized request bytes.
    RequestBytes,
    /// One raw response event/chunk bytes.
    ResponseEventBytes,
    /// Assembled tool argument bytes.
    ToolArgumentBytes,
    /// Provider timeout milliseconds.
    TimeoutMillis,
    /// Retry attempts after the initial attempt.
    Retries,
    /// Retry backoff milliseconds.
    BackoffMillis,
    /// OAuth state lifetime seconds.
    OAuthStateSeconds,
    /// One decoded image bytes.
    ImageBytes,
    /// Aggregate decoded request image bytes.
    ImageRequestBytes,
}

/// Validates an inclusive `usize` ceiling.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above `maximum`.
pub const fn validate_usize(
    value: usize,
    maximum: usize,
    kind: LimitKind,
) -> Result<(), LimitError> {
    if value <= maximum {
        Ok(())
    } else {
        Err(LimitError::Exceeded { kind })
    }
}

/// Validates an inclusive `u64` ceiling.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above `maximum`.
pub const fn validate_u64(value: u64, maximum: u64, kind: LimitKind) -> Result<(), LimitError> {
    if value <= maximum {
        Ok(())
    } else {
        Err(LimitError::Exceeded { kind })
    }
}

/// Validates an inclusive `u32` ceiling.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above `maximum`.
pub const fn validate_u32(value: u32, maximum: u32, kind: LimitKind) -> Result<(), LimitError> {
    if value <= maximum {
        Ok(())
    } else {
        Err(LimitError::Exceeded { kind })
    }
}

/// Validates configured provider count.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical provider bound.
pub const fn validate_provider_count(value: usize) -> Result<(), LimitError> {
    validate_usize(value, PROVIDERS_MAX, LimitKind::Providers)
}

/// Validates models listed by one provider.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical model bound.
pub const fn validate_models_per_provider(value: usize) -> Result<(), LimitError> {
    validate_usize(value, MODELS_PER_PROVIDER_MAX, LimitKind::ModelsPerProvider)
}

/// Validates exact serialized provider request bytes.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical request bound.
pub const fn validate_provider_request_bytes(value: usize) -> Result<(), LimitError> {
    validate_usize(value, PROVIDER_REQUEST_BYTES_MAX, LimitKind::RequestBytes)
}

/// Validates assembled tool-call argument bytes.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical argument bound.
pub const fn validate_tool_argument_bytes(value: usize) -> Result<(), LimitError> {
    if value <= lotta_runtime::bounds::TOOL_ARGUMENT_BYTES_MAX.value {
        Ok(())
    } else {
        Err(LimitError::Exceeded {
            kind: LimitKind::ToolArgumentBytes,
        })
    }
}

/// Returns the canonical provider timeout.
#[must_use]
pub const fn provider_timeout_default() -> std::time::Duration {
    std::time::Duration::from_millis(PROVIDER_TIMEOUT_MS_DEFAULT)
}

/// Validates configured provider retries.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical retry bound.
pub fn validate_provider_retries(value: u32) -> Result<(), LimitError> {
    match lotta_runtime::retry::validate_provider_retries(value) {
        Ok(()) => Ok(()),
        Err(_) => Err(LimitError::Exceeded {
            kind: LimitKind::Retries,
        }),
    }
}

/// Caps a provider retry delay at the canonical backoff maximum.
#[must_use]
pub const fn cap_backoff(value: u64) -> u64 {
    lotta_runtime::retry::cap_backoff(value)
}

/// Returns the canonical OAuth state lifetime.
#[must_use]
pub const fn oauth_state_ttl() -> std::time::Duration {
    std::time::Duration::from_secs(OAUTH_STATE_TTL_SECONDS)
}

/// Validates one raw inbound event/chunk before parsing.
///
/// # Errors
/// Returns [`LimitError::Exceeded`] above the canonical event bound.
pub const fn validate_response_event_bytes(value: usize) -> Result<(), LimitError> {
    validate_usize(
        value,
        PROVIDER_RESPONSE_EVENT_BYTES_MAX,
        LimitKind::ResponseEventBytes,
    )
}

/// Validates one decoded image and checked request image aggregate.
///
/// # Errors
/// Returns a typed overflow or exceeded image/request aggregate error.
pub fn validate_image_bytes(value: usize, aggregate: &mut usize) -> Result<(), LimitError> {
    validate_usize(value, IMAGE_BYTES_MAX, LimitKind::ImageBytes)?;
    *aggregate = aggregate.checked_add(value).ok_or(LimitError::Arithmetic {
        kind: LimitKind::ImageRequestBytes,
    })?;
    validate_usize(
        *aggregate,
        PROVIDER_REQUEST_BYTES_MAX,
        LimitKind::ImageRequestBytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! boundary3 {
        ($name:ident, $limit:expr, $validator:path) => {
            #[test]
            fn $name() {
                assert!($validator($limit - 1).is_ok());
                assert!($validator($limit).is_ok());
                assert!($validator($limit + 1).is_err());
            }
        };
    }

    boundary3!(
        provider_count_named_boundary,
        PROVIDERS_MAX,
        validate_provider_count
    );
    boundary3!(
        models_named_boundary,
        MODELS_PER_PROVIDER_MAX,
        validate_models_per_provider
    );
    boundary3!(
        request_named_boundary,
        PROVIDER_REQUEST_BYTES_MAX,
        validate_provider_request_bytes
    );
    boundary3!(
        event_named_boundary,
        PROVIDER_RESPONSE_EVENT_BYTES_MAX,
        validate_response_event_bytes
    );
    boundary3!(
        tool_named_boundary,
        TOOL_ARGUMENT_BYTES_MAX,
        validate_tool_argument_bytes
    );
    boundary3!(
        retries_named_boundary,
        PROVIDER_RETRIES_MAX,
        validate_provider_retries
    );

    #[test]
    fn timeout_named_default_is_exact() {
        assert_eq!(
            provider_timeout_default().as_millis(),
            u128::from(PROVIDER_TIMEOUT_MS_DEFAULT)
        );
    }

    #[test]
    fn backoff_named_cap_is_exact() {
        assert_eq!(
            cap_backoff(PROVIDER_BACKOFF_MS_MAX - 1),
            PROVIDER_BACKOFF_MS_MAX - 1
        );
        assert_eq!(
            cap_backoff(PROVIDER_BACKOFF_MS_MAX),
            PROVIDER_BACKOFF_MS_MAX
        );
        assert_eq!(
            cap_backoff(PROVIDER_BACKOFF_MS_MAX + 1),
            PROVIDER_BACKOFF_MS_MAX
        );
    }

    #[test]
    fn oauth_named_ttl_is_exact() {
        assert_eq!(oauth_state_ttl().as_secs(), OAUTH_STATE_TTL_SECONDS);
    }

    #[test]
    fn image_named_boundary_is_exact() {
        for (value, valid) in [
            (IMAGE_BYTES_MAX - 1, true),
            (IMAGE_BYTES_MAX, true),
            (IMAGE_BYTES_MAX + 1, false),
        ] {
            let mut aggregate = 0;
            assert_eq!(validate_image_bytes(value, &mut aggregate).is_ok(), valid);
        }
    }
}
