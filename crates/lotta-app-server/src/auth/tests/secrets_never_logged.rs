use super::{AppServerError, AuthPolicy};
use crate::{
    auth::test_support::{authorize, signed_args, temp_file, token},
    config::ServerArgs,
};
use serde_json::json;
use std::{
    fmt::Write as _,
    fs, io,
    sync::{Arc, Mutex},
};
use tracing::Dispatch;
use tracing_subscriber::fmt::MakeWriter;

const CAPABILITY: &str = "capability-original-literal-Task14-O5";
const CAPABILITY_REPLACEMENT: &str = "capability-replacement-literal-Task14-O5";
const SHARED: &[u8; 37] = b"shared-original-literal-Task14-O5-xxx";
const SHARED_REPLACEMENT: &[u8; 40] = b"shared-replacement-literal-Task14-O5-xxx";
const TRACE_LIMIT: usize = 16 * 1024;

type Bytes = Arc<Mutex<Vec<u8>>>;

#[derive(Clone)]
struct BoundedWriter(Bytes);

struct BoundedGuard(Bytes);

impl<'a> MakeWriter<'a> for BoundedWriter {
    type Writer = BoundedGuard;

    fn make_writer(&'a self) -> Self::Writer {
        BoundedGuard(Arc::clone(&self.0))
    }
}

impl io::Write for BoundedGuard {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let mut bytes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let accepted = input.len().min(TRACE_LIMIT.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&input[..accepted]);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capability_args(path: &std::path::Path) -> ServerArgs {
    ServerArgs {
        listen_enabled: true,
        ws_auth: Some("capability-token".into()),
        ws_token_file: Some(path.to_path_buf()),
        ..ServerArgs::default()
    }
}

fn replacement_proof(capability: &AuthPolicy, signed: &AuthPolicy) -> Vec<AppServerError> {
    assert!(authorize(capability, CAPABILITY, 60).is_ok());
    assert!(authorize(capability, CAPABILITY_REPLACEMENT, 60).is_err());
    let original = token(&json!({"exp": 61}), SHARED);
    let replacement = token(&json!({"exp": 61}), SHARED_REPLACEMENT);
    assert!(authorize(signed, &original, 60).is_ok());
    let failures = vec![
        authorize(signed, &replacement, 60).expect_err("replacement must fail"),
        authorize(capability, CAPABILITY_REPLACEMENT, 60).expect_err("replacement must fail"),
    ];
    failures
}

fn trace_diagnostics(policies: &[&AuthPolicy], failures: &[AppServerError]) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(BoundedWriter(Arc::clone(&bytes)))
        .finish();
    let dispatch = Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        for policy in policies {
            tracing::info!(
                auth_mode = policy.mode_name(),
                "app server listener started"
            );
        }
        for error in failures {
            tracing::warn!(code = error.code(), "websocket authentication denied");
        }
    });
    bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn scan_surfaces(policies: &[&AuthPolicy], failures: &[AppServerError], trace: &[u8]) {
    let mut surfaces = String::from_utf8_lossy(trace).into_owned();
    for policy in policies {
        let _ = write!(surfaces, "{policy:?}");
    }
    for error in failures {
        let _ = write!(surfaces, "{error:?}{error}");
    }
    let config = AuthPolicy::prepare(&ServerArgs {
        ws_auth: Some("capability-token".into()),
        ..ServerArgs::default()
    })
    .expect_err("missing capability source");
    let _ = write!(surfaces, "{config:?}{config}");
    let secrets = [
        CAPABILITY.as_bytes(),
        CAPABILITY_REPLACEMENT.as_bytes(),
        SHARED,
        SHARED_REPLACEMENT,
    ];
    for secret in secrets {
        assert!(
            !surfaces
                .as_bytes()
                .windows(secret.len())
                .any(|part| part == secret)
        );
    }
}

#[test]
fn secrets_never_logged() {
    let capability_path = temp_file(CAPABILITY.as_bytes());
    let shared_path = temp_file(SHARED);
    let capability =
        AuthPolicy::prepare(&capability_args(&capability_path)).expect("prepare capability");
    let signed = AuthPolicy::prepare(&signed_args(&shared_path)).expect("prepare signed");
    fs::remove_file(&capability_path).expect("delete capability file");
    fs::remove_file(&shared_path).expect("delete shared file");
    fs::write(&capability_path, CAPABILITY_REPLACEMENT).expect("replace capability file");
    fs::write(&shared_path, SHARED_REPLACEMENT).expect("replace shared file");
    let failures = replacement_proof(&capability, &signed);
    let policies = [&capability, &signed];
    let trace = trace_diagnostics(&policies, &failures);
    scan_surfaces(&policies, &failures, &trace);
    let _ = fs::remove_file(capability_path);
    let _ = fs::remove_file(shared_path);
}
