//! Bounded Axum JSON extraction performed before deserialization.

use axum::{body::to_bytes, extract::FromRequest, http::Request};
use serde::de::DeserializeOwned;

use crate::{bounds::HTTP_BODY_BYTES_MAX, errors::AppServerError};

/// JSON value extracted only after enforcing the canonical HTTP byte ceiling.
pub struct BoundedJson<T>(pub T);

impl<S, T> FromRequest<S> for BoundedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppServerError;

    async fn from_request(
        request: Request<axum::body::Body>,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(request.into_body(), HTTP_BODY_BYTES_MAX)
            .await
            .map_err(|_| AppServerError::PayloadTooLarge)?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|_| AppServerError::Malformed)
    }
}

#[cfg(test)]
pub(crate) fn decode_bytes_with_probe<T: DeserializeOwned>(
    bytes: &[u8],
    probe: &std::sync::atomic::AtomicUsize,
) -> Result<T, AppServerError> {
    crate::bounds::accept_capacity(bytes.len(), HTTP_BODY_BYTES_MAX)?;
    probe.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    serde_json::from_slice(bytes).map_err(|_| AppServerError::Malformed)
}

#[cfg(test)]
#[path = "http_body/tests.rs"]
mod route_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn body_below_cap_reaches_parser() {
        let probe = AtomicUsize::new(0);
        let bytes = vec![b' '; HTTP_BODY_BYTES_MAX - 1];
        assert!(decode_bytes_with_probe::<Value>(&bytes, &probe).is_err());
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn body_at_cap_reaches_parser() {
        let probe = AtomicUsize::new(0);
        let bytes = vec![b' '; HTTP_BODY_BYTES_MAX];
        assert!(decode_bytes_with_probe::<Value>(&bytes, &probe).is_err());
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn body_above_cap_skips_parser() {
        let probe = AtomicUsize::new(0);
        let bytes = vec![b' '; HTTP_BODY_BYTES_MAX + 1];
        assert!(decode_bytes_with_probe::<Value>(&bytes, &probe).is_err());
        assert_eq!(probe.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn malformed_below_cap_maps_bad_request() {
        let probe = AtomicUsize::new(0);
        let error = decode_bytes_with_probe::<Value>(b"{", &probe).expect_err("malformed");
        assert!(matches!(error, AppServerError::Malformed));
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }
}
