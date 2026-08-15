use axum::http::{HeaderMap, HeaderValue, header::ORIGIN};

#[test]
fn origin_bearing_loopback_rejected() {
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, HeaderValue::from_static("http://evil.example"));
    assert!(super::origin::enforce(&headers, &super::AuthPolicy::None).is_err());
}
