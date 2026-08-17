use lotta_runtime::boundary::{ProviderEventText, ProviderName};
use lotta_runtime::ports::{ProviderError, ProviderErrorContext};
use lotta_runtime::retry::RetryAfter;
use reqwest::{StatusCode, header::HeaderMap};
use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn model_id(handle: &str) -> Result<&str, ()> {
    handle
        .parse::<crate::model::ModelHandle>()
        .map_err(|_| ())?;
    handle.split_once('/').map(|(_, model)| model).ok_or(())
}

pub(crate) const ERROR_BODY_BYTES_MAX: usize = 1024 * 1024;

pub(crate) fn endpoint_allowed(endpoint: &reqwest::Url, has_credential: bool) -> bool {
    endpoint.scheme() == "https"
        || (endpoint.scheme() == "http"
            && endpoint
                .host_str()
                .is_some_and(|host| local_or_lan(host) && (!has_credential || is_loopback(host))))
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn local_or_lan(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(value) => value.is_loopback() || value.is_private() || value.is_link_local(),
        IpAddr::V6(value) => {
            value.is_loopback() || value.is_unique_local() || value.is_unicast_link_local()
        }
    })
}

pub(crate) fn endpoint(base: &reqwest::Url, path: &str) -> reqwest::Url {
    let mut value = base.clone();
    value.set_query(None);
    value.set_fragment(None);
    let base_path = value.path().trim_end_matches('/');
    let suffix = path.trim_start_matches('/');
    value.set_path(&format!("{base_path}/{suffix}"));
    value
}

pub(crate) fn context(code: &str, message: &str, headers: &HeaderMap) -> ProviderErrorContext {
    let code = ProviderName::new(normalize_code(code)).unwrap_or_else(|_| {
        ProviderName::new("provider_error".to_owned()).expect("constant valid")
    });
    let text = ProviderEventText::new(sanitize(message)).unwrap_or_else(|_| {
        ProviderEventText::new("provider error".to_owned()).expect("constant valid")
    });
    let value = ProviderErrorContext::new(code, text);
    retry_after(headers).map_or(value.clone(), |retry| value.with_retry_after(retry))
}

fn normalize_code(code: &str) -> String {
    let normalized = code
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(128)
        .collect::<String>();
    if normalized.is_empty() {
        "provider_error".to_owned()
    } else {
        normalized
    }
}

pub(crate) fn retry_after(headers: &HeaderMap) -> Option<RetryAfter> {
    if let Some(milliseconds) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_milliseconds)
    {
        return Some(RetryAfter::Milliseconds(milliseconds));
    }
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    parse_seconds(value)
        .or_else(|| parse_http_date(value))
        .map(RetryAfter::Milliseconds)
}

fn parse_milliseconds(value: &str) -> Option<u64> {
    parse_decimal(value.trim(), 1)
}

fn parse_seconds(value: &str) -> Option<u64> {
    parse_decimal(value.trim(), 1000)
}

fn parse_decimal(value: &str, scale: u64) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').map_or((value, ""), |parts| parts);
    let whole = whole.parse::<u64>().ok()?.checked_mul(scale)?;
    if fraction.is_empty() {
        return Some(whole);
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let denominator = 10_u64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let numerator = fraction.parse::<u64>().ok()?.checked_mul(scale)?;
    let rounded = numerator.checked_add(denominator - 1)? / denominator;
    whole.checked_add(rounded)
}

fn parse_http_date(value: &str) -> Option<u64> {
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let deadline = u64::try_from(date.timestamp_millis()).ok()?;
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()?;
    Some(deadline.saturating_sub(now))
}

pub(crate) fn sanitize(message: &str) -> String {
    let mut output = message.chars().take(512).collect::<String>();
    for prefix in ["sk-", "Bearer ", "bearer "] {
        while let Some(position) = output.find(prefix) {
            let end = output[position..]
                .find(char::is_whitespace)
                .map_or(output.len(), |length| position + length);
            output.replace_range(position..end, "[REDACTED]");
        }
    }
    output
}

pub(crate) fn map_error(
    status: StatusCode,
    error_type: &str,
    code: &str,
    message: &str,
    headers: &HeaderMap,
) -> ProviderError {
    let context = context(code, message, headers);
    let kind = error_type.to_ascii_lowercase();
    let code = code.to_ascii_lowercase();
    if matches!(kind.as_str(), "cancelled" | "canceled")
        || matches!(code.as_str(), "cancelled" | "canceled")
    {
        return ProviderError::Cancelled(context);
    }
    map_non_cancelled(status, &kind, &code, context)
}

/// A vendor-named kind outranks the transport status: providers routinely report a specific
/// failure (`authorization`, `protocol`, `quota`) under a generic `500`.
fn map_non_cancelled(
    status: StatusCode,
    kind: &str,
    code: &str,
    context: ProviderErrorContext,
) -> ProviderError {
    let text = format!("{kind} {code}");
    map_named_error(&text, &context).unwrap_or_else(|| map_status_error(status, &text, context))
}

fn map_status_error(
    status: StatusCode,
    text: &str,
    context: ProviderErrorContext,
) -> ProviderError {
    match status.as_u16() {
        401 => ProviderError::Authentication(context),
        403 => ProviderError::Authorization(context),
        402 => ProviderError::Quota(context),
        408 | 504 => ProviderError::Timeout(context),
        409 | 529 => ProviderError::Overloaded(context),
        429 => ProviderError::RateLimit(context),
        413 => ProviderError::ContextOverflow(context),
        400 | 404 | 422 => map_client_error(text, context),
        500..=599 => ProviderError::Unavailable(context),
        _ => ProviderError::Unknown(context),
    }
}

fn map_client_error(text: &str, context: ProviderErrorContext) -> ProviderError {
    if is_context_overflow(text) {
        ProviderError::ContextOverflow(context)
    } else {
        ProviderError::InvalidRequest(context)
    }
}

fn map_named_error(text: &str, context: &ProviderErrorContext) -> Option<ProviderError> {
    let context = context.clone();
    Some(
        if text.contains("authenticat") || text.contains("invalid_api_key") {
            ProviderError::Authentication(context)
        } else if text.contains("permission") || text.contains("authoriz") {
            ProviderError::Authorization(context)
        } else if text.contains("quota") || text.contains("credit") {
            ProviderError::Quota(context)
        } else if text.contains("rate_limit") {
            ProviderError::RateLimit(context)
        } else if text.contains("overload") || text.contains("busy") {
            ProviderError::Overloaded(context)
        } else if text.contains("timeout") {
            ProviderError::Timeout(context)
        } else if is_context_overflow(text) {
            ProviderError::ContextOverflow(context)
        } else if text.contains("protocol")
            || text.contains("schema")
            || text.contains("invalid_response")
        {
            ProviderError::Protocol(context)
        } else if text.contains("invalid_request") || text.contains("unsupported") {
            ProviderError::InvalidRequest(context)
        } else {
            return None;
        },
    )
}

fn is_context_overflow(text: &str) -> bool {
    [
        "context_overflow",
        "context_length",
        "context window",
        "request_too_large",
        "too many tokens",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{map_error, retry_after, sanitize};
    use lotta_runtime::ports::ProviderError;
    use lotta_runtime::retry::RetryAfter;
    use reqwest::{StatusCode, header::HeaderMap};

    #[test]
    fn cancellation_precedes_status_and_all_server_errors_are_unavailable() {
        let headers = HeaderMap::new();
        assert!(matches!(
            map_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cancelled",
                "x",
                "x",
                &headers
            ),
            ProviderError::Cancelled(_)
        ));
        assert!(matches!(
            map_error(
                StatusCode::from_u16(529).unwrap(),
                "error",
                "x",
                "x",
                &headers
            ),
            ProviderError::Overloaded(_)
        ));
        assert!(matches!(
            map_error(StatusCode::NOT_IMPLEMENTED, "error", "x", "x", &headers),
            ProviderError::Unavailable(_)
        ));
    }

    #[test]
    fn retry_after_supports_milliseconds_and_fractional_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "0.25".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(RetryAfter::Milliseconds(250)));
        headers.insert("retry-after-ms", "12.5".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(RetryAfter::Milliseconds(13)));
    }

    #[test]
    fn scrubbing_preserves_non_secret_token_diagnostics() {
        assert_eq!(sanitize("max_tokens must be 10"), "max_tokens must be 10");
        assert_eq!(sanitize("key sk-secret leaked"), "key [REDACTED] leaked");
    }
}
