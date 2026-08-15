use super::AuthPolicy;
use crate::auth::test_support::{signed_args, temp_file};
use std::path::PathBuf;

#[test]
fn accepts_299() {
    let p = temp_file(&[b'x'; 32]);
    let mut a = signed_args(&p);
    a.ws_max_clock_skew_seconds = Some(299);
    assert!(AuthPolicy::prepare(&a).is_ok());
}

#[test]
fn accepts_300() {
    let p = temp_file(&[b'x'; 32]);
    let mut a = signed_args(&p);
    a.ws_max_clock_skew_seconds = Some(300);
    assert!(AuthPolicy::prepare(&a).is_ok());
}
#[test]
fn rejects_301_before_bind() {
    let mut a = signed_args(&PathBuf::from("/definitely/missing/secret"));
    a.ws_max_clock_skew_seconds = Some(301);
    let error = AuthPolicy::prepare(&a)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("clock skew"));
}
