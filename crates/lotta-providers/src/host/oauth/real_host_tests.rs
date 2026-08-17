use super::*;
use crate::host::client::{HostClient, HostConfig, materialize_host_script};
use lotta_extensions::sidecar::SidecarOwnerIdentity;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Clock(AtomicU64);
impl Clock {
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::SeqCst);
    }
}
impl OAuthClock for Clock {
    fn now_seconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Random(AtomicU8);
impl OAuthRandom for Random {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), OAuthError> {
        let start = self.0.fetch_add(1, Ordering::SeqCst);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = start.wrapping_add(u8::try_from(index).unwrap());
        }
        Ok(())
    }
}

#[derive(Default)]
struct Browser;
impl OAuthBrowser for Browser {
    fn open(&self, _: &Url) -> Result<(), OAuthError> {
        Ok(())
    }
}

#[derive(Default)]
struct Http(Mutex<VecDeque<OAuthHttpResponse>>);
impl Http {
    fn push(&self, status: u16, fields: &[(&str, &str)]) {
        self.0.lock().unwrap().push_back(OAuthHttpResponse {
            status,
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        });
    }
    fn response(&self) -> Result<OAuthHttpResponse, OAuthError> {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(OAuthError::Provider)
    }
}
impl OAuthHttp for Http {
    fn post_form(&self, _: &Url, _: &[(&str, &str)]) -> Result<OAuthHttpResponse, OAuthError> {
        self.response()
    }
    fn post_json(&self, _: &Url, _: &[(&str, &str)]) -> Result<OAuthHttpResponse, OAuthError> {
        self.response()
    }
}

fn source_root() -> PathBuf {
    std::env::var_os("LOTTA_LETTA_CODE_CHECKOUT")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../letta-code"),
            PathBuf::from,
        )
        .canonicalize()
        .unwrap()
}

fn config() -> (HostConfig, PathBuf) {
    let source = source_root();
    let fixture = std::env::temp_dir().join(format!(
        "lotta-oauth-host-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&fixture);
    std::fs::create_dir(&fixture).unwrap();
    let script = fixture.join("pi-ai-host.mjs");
    materialize_host_script(&script).unwrap();
    let scratch = fixture.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    (
        HostConfig {
            bun_executable: PathBuf::from(
                std::env::var_os("LOTTA_BUN_EXECUTABLE")
                    .unwrap_or_else(|| "/Users/stan/.bun/bin/bun".into()),
            )
            .canonicalize()
            .unwrap(),
            host_script: script.canonicalize().unwrap(),
            package_root: source
                .join("node_modules/@earendil-works/pi-ai")
                .canonicalize()
                .unwrap(),
            scratch_cwd: scratch.canonicalize().unwrap(),
            test_mode: false,
        },
        fixture,
    )
}

fn manager() -> (Arc<OAuthManager>, Arc<Clock>, Arc<Http>) {
    let clock = Arc::new(Clock::default());
    let http = Arc::new(Http::default());
    (
        Arc::new(
            OAuthManager::new(
                clock.clone(),
                Arc::new(Random::default()),
                http.clone(),
                Arc::new(Browser),
            )
            .unwrap(),
        ),
        clock,
        http,
    )
}

fn state(flow: &OAuthBegin) -> String {
    Url::parse(&flow.authorization_url)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1
        .into_owned()
}

fn tokens(http: &Http) {
    http.push(
        200,
        &[
            ("access_token", "marker-access"),
            ("refresh_token", "marker-refresh"),
            ("id_token", "marker-id"),
            ("expires_in", "3600"),
        ],
    );
}

#[tokio::test]
async fn real_host_pkce_flow() {
    let (manager, _, http) = manager();
    let (config, root) = config();
    let mut client = HostClient::spawn(
        config,
        SidecarOwnerIdentity::new("oauth", "runtime", "pkce").unwrap(),
    )
    .await
    .unwrap()
    .with_oauth(manager);
    let flow = client
        .oauth_begin(OPENAI_PROVIDER, "session", OPENAI_REDIRECT, false)
        .await
        .unwrap();
    let callback_state = state(&flow);
    tokens(&http);
    let credential = client
        .oauth_callback(
            &flow.flow_id,
            OPENAI_PROVIDER,
            "session",
            "http://localhost:1455",
            OPENAI_REDIRECT,
            &[("code", "marker-code"), ("state", &callback_state)],
        )
        .await
        .unwrap();
    assert_eq!(credential.expires_at, 3600);
    assert!(
        client
            .oauth_callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "http://localhost:1455",
                OPENAI_REDIRECT,
                &[("code", "marker-code"), ("state", &callback_state)]
            )
            .await
            .is_err()
    );
    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn real_host_device_flow() {
    let (manager, clock, http) = manager();
    http.push(
        200,
        &[
            ("device_auth_id", "device-id"),
            ("user_code", "USER-CODE"),
            ("interval", "1"),
        ],
    );
    let (config, root) = config();
    let mut client = HostClient::spawn(
        config,
        SidecarOwnerIdentity::new("oauth", "runtime", "device").unwrap(),
    )
    .await
    .unwrap()
    .with_oauth(manager);
    let flow = client
        .oauth_device_begin(OPENAI_PROVIDER, "session")
        .await
        .unwrap();
    http.push(400, &[("error", "authorization_pending")]);
    assert!(matches!(
        client
            .oauth_device_poll(&flow.flow_id, OPENAI_PROVIDER, "session")
            .await
            .unwrap(),
        OAuthDevicePoll::Pending { .. }
    ));
    clock.set(1);
    http.push(400, &[("error", "slow_down")]);
    assert!(matches!(
        client
            .oauth_device_poll(&flow.flow_id, OPENAI_PROVIDER, "session")
            .await
            .unwrap(),
        OAuthDevicePoll::Pending { next_poll_at: 7 }
    ));
    clock.set(7);
    http.push(
        200,
        &[
            ("authorization_code", "device-code"),
            ("code_verifier", "device-verifier"),
        ],
    );
    tokens(&http);
    assert!(matches!(
        client
            .oauth_device_poll(&flow.flow_id, OPENAI_PROVIDER, "session")
            .await
            .unwrap(),
        OAuthDevicePoll::Complete(_)
    ));
    assert!(
        client
            .oauth_device_poll(&flow.flow_id, OPENAI_PROVIDER, "session")
            .await
            .is_err()
    );
    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
