use super::{AUTHORIZATION_HEADER_BYTES_MAX, bearer_token};
use crate::auth::test_support::headers;
use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

#[test]
fn bearer_exact_one() {
    assert!(bearer_token(&headers(b"Bearer token")).is_ok_and(|v| v == "token"));
}
#[test]
fn bearer_scheme_is_case_insensitive() {
    assert!(bearer_token(&headers(b"bEaReR token")).is_ok_and(|v| v == "token"));
}
#[test]
fn bearer_trims_outer_whitespace() {
    assert!(bearer_token(&headers(b"Bearer \t token \t")).is_ok_and(|v| v == "token"));
}
#[test]
fn bearer_rejects_duplicate() {
    let mut h = HeaderMap::new();
    h.append(AUTHORIZATION, HeaderValue::from_static("Bearer a"));
    h.append(AUTHORIZATION, HeaderValue::from_static("Bearer a"));
    assert!(bearer_token(&h).is_err());
}
#[test]
fn bearer_rejects_internal_whitespace() {
    assert!(bearer_token(&headers(b"Bearer to ken")).is_err());
}
#[test]
fn bearer_rejects_non_utf8() {
    assert!(bearer_token(&headers(b"Bearer \xff")).is_err());
}
#[test]
fn bearer_rejects_oversize() {
    let value = format!("Bearer {}", "x".repeat(AUTHORIZATION_HEADER_BYTES_MAX));
    assert!(bearer_token(&headers(value.as_bytes())).is_err());
}
#[test]
fn bearer_rejects_missing_or_empty() {
    assert!(bearer_token(&HeaderMap::new()).is_err());
    assert!(bearer_token(&headers(b"Bearer ")).is_err());
}
