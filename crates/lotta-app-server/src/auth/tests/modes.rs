use std::path::PathBuf;

use super::{AppServerError, AuthPolicy};
use crate::auth::test_support::{signed_args, temp_file};
use crate::config::{ServerArgs, parse_cli};

fn capability() -> ServerArgs {
    ServerArgs {
        listen_enabled: true,
        ws_auth: Some("capability-token".into()),
        ws_token_sha256: Some("00".repeat(32)),
        ..ServerArgs::default()
    }
}

#[test]
fn accepts_capability_token() {
    assert!(AuthPolicy::prepare(&capability()).is_ok_and(|p| p.mode_name() == "capability-token"));
}
#[test]
fn accepts_signed_bearer_token() {
    let p = temp_file(&[b'x'; 32]);
    assert!(
        AuthPolicy::prepare(&signed_args(&p)).is_ok_and(|p| p.mode_name() == "signed-bearer-token")
    );
}
#[test]
fn rejects_unknown_mode() {
    let mut a = capability();
    a.ws_auth = Some("other".into());
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_token_file_and_digest_together() {
    let mut a = capability();
    a.ws_token_file = Some(temp_file(b"x"));
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_auth_flags_without_mode() {
    let a = ServerArgs {
        ws_token_sha256: Some("00".repeat(32)),
        ..ServerArgs::default()
    };
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_neither_capability_source() {
    let mut a = capability();
    a.ws_token_sha256 = None;
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_cross_mode_flags() {
    let mut a = capability();
    a.ws_issuer = Some("x".into());
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn requires_signed_shared_secret() {
    let a = ServerArgs {
        ws_auth: Some("signed-bearer-token".into()),
        ..ServerArgs::default()
    };
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_relative_files() {
    let mut a = signed_args(&PathBuf::from("relative"));
    assert!(AuthPolicy::prepare(&a).is_err());
    a = capability();
    a.ws_token_sha256 = None;
    a.ws_token_file = Some(PathBuf::from("relative"));
    assert!(AuthPolicy::prepare(&a).is_err());
}
#[test]
fn rejects_duplicate_cli_value() {
    assert!(parse_cli(["--listen", "--ws-auth", "x", "--ws-auth", "y"]).is_err());
}
#[test]
fn rejects_duplicate_cli_boolean() {
    assert!(parse_cli(["--listen", "--openai-api", "--openai-api"]).is_err());
}
#[test]
fn rejects_duplicate_listen() {
    assert!(parse_cli(["--listen", "--listen"]).is_err());
}
#[test]
fn rejects_unknown_arg() {
    assert!(parse_cli(["--listen", "--wat"]).is_err());
}
#[test]
fn omitted_port_prepares() {
    assert!(
        parse_cli(["--listen", "ws://127.0.0.1/ws"])
            .and_then(ServerArgs::prepare)
            .is_ok()
    );
}
#[test]
fn rejects_secret_oversize() {
    let p = temp_file(&vec![b'x'; 65_537]);
    assert!(AuthPolicy::prepare(&signed_args(&p)).is_err());
}
#[test]
fn config_errors_are_stable() {
    let result = AuthPolicy::prepare(&ServerArgs {
        ws_auth: Some("bad".into()),
        ..ServerArgs::default()
    });
    assert!(matches!(result, Err(AppServerError::Config(_))));
}
