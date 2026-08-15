use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use lotta_domain::{BoundedJsonValue, Clock, Secret};
use serde_json::Value;
use sha2::Sha256;

use crate::error::AppServerError;

/// Largest accepted encoded signed bearer token.
pub const SIGNED_BEARER_TOKEN_BYTES_MAX: usize = 16_384;
/// Largest decoded header or claims component.
pub const SIGNED_BEARER_COMPONENT_BYTES_MAX: usize = 8_192;
/// Largest accepted audience array.
pub const SIGNED_BEARER_AUDIENCES_MAX: usize = 64;

type HmacSha256 = Hmac<Sha256>;

struct Claims<'a> {
    exp: i64,
    nbf: Option<i64>,
    issuer: Option<&'a str>,
    audience: Option<&'a Value>,
}

/// Verifies canonical HS256 JWT structure, signature, and bounded claims.
///
/// # Errors
/// Returns a fixed unauthorized error for every malformed or invalid token.
pub fn verify_token(
    token: &str,
    shared_secret: &Secret<Vec<u8>>,
    issuer: Option<&str>,
    audience: Option<&str>,
    skew_seconds: u32,
    clock: &dyn Clock,
) -> Result<(), AppServerError> {
    if token.len() > SIGNED_BEARER_TOKEN_BYTES_MAX || token.contains('=') {
        return Err(AppServerError::Unauthorized);
    }
    let mut components = token.split('.');
    let header_component = components.next().ok_or(AppServerError::Unauthorized)?;
    let claim_component = components.next().ok_or(AppServerError::Unauthorized)?;
    let signature_component = components.next().ok_or(AppServerError::Unauthorized)?;
    if components.next().is_some()
        || header_component.is_empty()
        || claim_component.is_empty()
        || signature_component.is_empty()
    {
        return Err(AppServerError::Unauthorized);
    }
    let header_bytes = decode_component(header_component)?;
    let claim_bytes = decode_component(claim_component)?;
    let signature = decode_component(signature_component)?;
    let bounded_header: BoundedJsonValue =
        serde_json::from_slice(&header_bytes).map_err(|_| AppServerError::Unauthorized)?;
    validate_header(bounded_header.as_value())?;
    verify_signature(header_component, claim_component, &signature, shared_secret)?;
    let bounded: BoundedJsonValue =
        serde_json::from_slice(&claim_bytes).map_err(|_| AppServerError::Unauthorized)?;
    let claims = parse_claims(bounded.as_value())?;
    validate_claims(&claims, issuer, audience, skew_seconds, clock)
}

fn validate_header(value: &Value) -> Result<(), AppServerError> {
    let object = value.as_object().ok_or(AppServerError::Unauthorized)?;
    if object.get("alg").and_then(Value::as_str) != Some("HS256") {
        return Err(AppServerError::Unauthorized);
    }
    Ok(())
}

fn decode_component(value: &str) -> Result<Vec<u8>, AppServerError> {
    if value.len() > SIGNED_BEARER_COMPONENT_BYTES_MAX
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(AppServerError::Unauthorized);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppServerError::Unauthorized)?;
    if decoded.len() > SIGNED_BEARER_COMPONENT_BYTES_MAX
        || URL_SAFE_NO_PAD.encode(&decoded) != value
    {
        return Err(AppServerError::Unauthorized);
    }
    Ok(decoded)
}

fn verify_signature(
    header: &str,
    claims: &str,
    signature: &[u8],
    secret: &Secret<Vec<u8>>,
) -> Result<(), AppServerError> {
    secret.expose_secret(|key| {
        let mut mac = HmacSha256::new_from_slice(key).map_err(|_| AppServerError::Unauthorized)?;
        mac.update(header.as_bytes());
        mac.update(b".");
        mac.update(claims.as_bytes());
        mac.verify_slice(signature)
            .map_err(|_| AppServerError::Unauthorized)
    })
}

fn parse_claims(value: &Value) -> Result<Claims<'_>, AppServerError> {
    let object = value.as_object().ok_or(AppServerError::Unauthorized)?;
    let exp = integral_claim(object.get("exp").ok_or(AppServerError::Unauthorized)?)?;
    let nbf = object.get("nbf").map(integral_claim).transpose()?;
    let issuer = object.get("iss").map(Value::as_str).transpose_option()?;
    let audience = object.get("aud");
    Ok(Claims {
        exp,
        nbf,
        issuer,
        audience,
    })
}

fn integral_claim(value: &Value) -> Result<i64, AppServerError> {
    const JS_INTEGER_MAX: i64 = 9_007_199_254_740_991;
    let number = value.as_i64().ok_or(AppServerError::Unauthorized)?;
    if !(-JS_INTEGER_MAX..=JS_INTEGER_MAX).contains(&number) {
        return Err(AppServerError::Unauthorized);
    }
    Ok(number)
}

trait TransposeOption<T> {
    fn transpose_option(self) -> Result<Option<T>, AppServerError>;
}

impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Result<Option<T>, AppServerError> {
        match self {
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(AppServerError::Unauthorized),
            None => Ok(None),
        }
    }
}

fn validate_claims(
    claims: &Claims<'_>,
    issuer: Option<&str>,
    audience: Option<&str>,
    skew_seconds: u32,
    clock: &dyn Clock,
) -> Result<(), AppServerError> {
    let now = clock.now().as_utc().timestamp();
    let skew = i64::from(skew_seconds);
    let latest = claims
        .exp
        .checked_add(skew)
        .ok_or(AppServerError::Unauthorized)?;
    if now > latest {
        return Err(AppServerError::Unauthorized);
    }
    if let Some(nbf) = claims.nbf {
        let earliest = nbf.checked_sub(skew).ok_or(AppServerError::Unauthorized)?;
        if now < earliest {
            return Err(AppServerError::Unauthorized);
        }
    }
    if issuer.is_some_and(|expected| claims.issuer != Some(expected)) {
        return Err(AppServerError::Unauthorized);
    }
    validate_audience(claims.audience)?;
    if let Some(expected) = audience
        && !audience_matches(claims.audience, expected)
    {
        return Err(AppServerError::Unauthorized);
    }
    Ok(())
}

fn validate_audience(value: Option<&Value>) -> Result<(), AppServerError> {
    match value {
        None | Some(Value::String(_)) => Ok(()),
        Some(Value::Array(values)) if values.len() <= SIGNED_BEARER_AUDIENCES_MAX => {
            if values.iter().all(Value::is_string) {
                Ok(())
            } else {
                Err(AppServerError::Unauthorized)
            }
        }
        Some(_) => Err(AppServerError::Unauthorized),
    }
}

fn audience_matches(value: Option<&Value>, expected: &str) -> bool {
    match value {
        Some(Value::String(actual)) => actual == expected,
        Some(Value::Array(actual)) => actual.iter().any(|item| item.as_str() == Some(expected)),
        None | Some(_) => false,
    }
}
