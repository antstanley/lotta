use super::AuthPolicy;
use crate::auth::test_support::{FixedClock, headers, temp_file};
use crate::config::ServerArgs;
use sha2::{Digest, Sha256};

fn policy_file(token: &[u8]) -> AuthPolicy {
    let p = temp_file(token);
    AuthPolicy::prepare(&ServerArgs {
        ws_auth: Some("capability-token".into()),
        ws_token_file: Some(p),
        ..ServerArgs::default()
    })
    .unwrap_or_else(|_| panic!("prepare"))
}
fn policy_digest(token: &[u8], upper: bool) -> AuthPolicy {
    let digest = Sha256::digest(token);
    let text = digest
        .iter()
        .map(|b| {
            if upper {
                format!("{b:02X}")
            } else {
                format!("{b:02x}")
            }
        })
        .collect();
    AuthPolicy::prepare(&ServerArgs {
        ws_auth: Some("capability-token".into()),
        ws_token_sha256: Some(text),
        ..ServerArgs::default()
    })
    .unwrap_or_else(|_| panic!("prepare"))
}
fn check(policy: &AuthPolicy, token: &[u8]) -> bool {
    policy
        .authorize(
            &headers(&[b"Bearer ".as_slice(), token].concat()),
            &FixedClock(1),
        )
        .is_ok()
}

#[test]
fn file_source_accepts_valid_and_rejects_wrong() {
    let p = policy_file(b"alpha");
    assert!(check(&p, b"alpha"));
    assert!(!check(&p, b"beta"));
}
#[test]
fn digest_source_accepts_valid_and_rejects_wrong() {
    let p = policy_digest(b"alpha", false);
    assert!(check(&p, b"alpha"));
    assert!(!check(&p, b"beta"));
}
#[test]
fn uppercase_hex_is_accepted() {
    assert!(check(&policy_digest(b"alpha", true), b"alpha"));
}
#[test]
fn file_trims_only_edges() {
    let p = policy_file(b" \nalpha\t ");
    assert!(check(&p, b"alpha"));
    assert!(!check(&p, b"al pha"));
}
