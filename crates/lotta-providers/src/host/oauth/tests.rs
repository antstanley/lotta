use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

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
impl OAuthWallClock for Clock {
    fn unix_epoch_millis(&self) -> Result<u64, OAuthError> {
        self.now_seconds()
            .checked_mul(1_000)
            .ok_or(OAuthError::Provider)
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
struct Browser(Mutex<Vec<String>>);
impl OAuthBrowser for Browser {
    fn open(&self, url: &Url) -> Result<(), OAuthError> {
        self.0.lock().unwrap().push(url.to_string());
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
                .map(|(k, v)| ((*k).into(), (*v).into()))
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

fn setup() -> (OAuthManager, Arc<Clock>, Arc<Http>) {
    let clock = Arc::new(Clock::default());
    let http = Arc::new(Http::default());
    (
        OAuthManager::new(
            clock.clone(),
            clock.clone(),
            Arc::new(Random::default()),
            http.clone(),
            Arc::new(Browser::default()),
        )
        .unwrap(),
        clock,
        http,
    )
}

fn begin(manager: &OAuthManager) -> OAuthBegin {
    manager
        .begin(OPENAI_PROVIDER, "session", "owner", OPENAI_REDIRECT, false)
        .unwrap()
}

fn state(value: &OAuthBegin) -> String {
    Url::parse(&value.authorization_url)
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

#[test]
fn pkce_rfc7636_vector() {
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn rejects_expired_state() {
    let (manager, clock, _) = setup();
    let flow = begin(&manager);
    let value = state(&flow);
    clock.set(OAUTH_STATE_TTL_SECONDS);
    assert_eq!(
        manager
            .callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "owner",
                OAuthCallback {
                    origin: "http://localhost:1455",
                    redirect_uri: OPENAI_REDIRECT,
                    query: &[("code", "marker-code"), ("state", &value)]
                }
            )
            .unwrap_err(),
        OAuthError::Expired
    );
}

#[test]
fn rejects_mismatched_state() {
    let (manager, _, _) = setup();
    let flow = begin(&manager);
    assert_eq!(
        manager
            .callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "owner",
                OAuthCallback {
                    origin: "http://localhost:1455",
                    redirect_uri: OPENAI_REDIRECT,
                    query: &[("code", "marker-code"), ("state", "wrong")]
                }
            )
            .unwrap_err(),
        OAuthError::StateMismatch
    );
}

#[test]
fn callback_success_is_one_use_and_at_ttl_expires() {
    let (manager, clock, http) = setup();
    let flow = begin(&manager);
    let value = state(&flow);
    tokens(&http);
    clock.set(OAUTH_STATE_TTL_SECONDS - 1);
    let credential = manager
        .callback(
            &flow.flow_id,
            OPENAI_PROVIDER,
            "session",
            "owner",
            OAuthCallback {
                origin: "http://localhost:1455",
                redirect_uri: OPENAI_REDIRECT,
                query: &[("code", "marker-code"), ("state", &value)],
            },
        )
        .unwrap();
    assert_eq!(
        credential.expires_at,
        (OAUTH_STATE_TTL_SECONDS - 1 + 3600) * 1_000
    );
    assert_eq!(
        manager
            .callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "owner",
                OAuthCallback {
                    origin: "http://localhost:1455",
                    redirect_uri: OPENAI_REDIRECT,
                    query: &[("code", "marker-code"), ("state", &value)]
                }
            )
            .unwrap_err(),
        OAuthError::Unavailable
    );
}

#[test]
fn callback_matrix_rejects_missing_duplicate_error_and_location() {
    for query in [
        vec![("code", "c")],
        vec![("state", "s")],
        vec![("code", "c"), ("code", "d"), ("state", "s")],
        vec![("code", "c"), ("error", "denied"), ("state", "s")],
    ] {
        let (manager, _, _) = setup();
        let flow = begin(&manager);
        assert_eq!(
            manager
                .callback(
                    &flow.flow_id,
                    OPENAI_PROVIDER,
                    "session",
                    "owner",
                    OAuthCallback {
                        origin: "http://localhost:1455",
                        redirect_uri: OPENAI_REDIRECT,
                        query: &query
                    }
                )
                .unwrap_err(),
            OAuthError::InvalidInput
        );
    }
    let (manager, _, _) = setup();
    let flow = begin(&manager);
    let value = state(&flow);
    assert_eq!(
        manager
            .callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "owner",
                OAuthCallback {
                    origin: "http://127.0.0.1:1455",
                    redirect_uri: OPENAI_REDIRECT,
                    query: &[("code", "c"), ("state", &value)]
                }
            )
            .unwrap_err(),
        OAuthError::InvalidInput
    );
}

#[test]
fn error_callback_is_terminal_and_denied() {
    let (manager, _, _) = setup();
    let flow = begin(&manager);
    let value = state(&flow);
    assert_eq!(
        manager
            .callback(
                &flow.flow_id,
                OPENAI_PROVIDER,
                "session",
                "owner",
                OAuthCallback {
                    origin: "http://localhost:1455",
                    redirect_uri: OPENAI_REDIRECT,
                    query: &[("error", "access_denied"), ("state", &value)]
                }
            )
            .unwrap_err(),
        OAuthError::Denied
    );
    assert_eq!(
        manager
            .cancel(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap_err(),
        OAuthError::Unavailable
    );
}

#[test]
fn binding_cancel_and_host_death_clear_pending() {
    let (manager, _, _) = setup();
    let one = begin(&manager);
    assert_eq!(
        manager
            .cancel(&one.flow_id, OPENAI_PROVIDER, "other", "owner")
            .unwrap_err(),
        OAuthError::Unavailable
    );
    manager
        .cancel(&one.flow_id, OPENAI_PROVIDER, "session", "owner")
        .unwrap();
    let two = begin(&manager);
    manager.host_died();
    assert_eq!(
        manager
            .cancel(&two.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap_err(),
        OAuthError::Unavailable
    );
}

#[test]
fn device_matrix_pending_slow_down_complete_replay() {
    let (manager, clock, http) = setup();
    http.push(
        200,
        &[
            ("device_auth_id", "device-id"),
            ("user_code", "USER-CODE"),
            ("interval", "1"),
        ],
    );
    let flow = manager
        .begin_device(OPENAI_PROVIDER, "session", "owner")
        .unwrap();
    http.push(400, &[("error", "authorization_pending")]);
    assert!(matches!(
        manager
            .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap(),
        OAuthDevicePoll::Pending { .. }
    ));
    assert_eq!(
        manager
            .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap_err(),
        OAuthError::TooSoon
    );
    clock.set(1);
    http.push(400, &[("error", "slow_down")]);
    let OAuthDevicePoll::Pending { next_poll_at } = manager
        .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(next_poll_at, 7);
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
        manager
            .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap(),
        OAuthDevicePoll::Complete(_)
    ));
    assert_eq!(
        manager
            .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap_err(),
        OAuthError::Unavailable
    );
}

#[test]
fn device_denied_expired_and_cancelled_are_terminal() {
    for error in ["access_denied", "expired_token"] {
        let (manager, _, http) = setup();
        http.push(200, &[("device_auth_id", "id"), ("user_code", "code")]);
        let flow = manager
            .begin_device(OPENAI_PROVIDER, "session", "owner")
            .unwrap();
        http.push(400, &[("error", error)]);
        let expected = if error == "access_denied" {
            OAuthError::Denied
        } else {
            OAuthError::Expired
        };
        assert_eq!(
            manager
                .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
                .unwrap_err(),
            expected
        );
    }
    let (manager, clock, http) = setup();
    http.push(200, &[("device_auth_id", "id"), ("user_code", "code")]);
    let flow = manager
        .begin_device(OPENAI_PROVIDER, "session", "owner")
        .unwrap();
    clock.set(OAUTH_DEVICE_TTL_SECONDS);
    assert_eq!(
        manager
            .poll_device(&flow.flow_id, OPENAI_PROVIDER, "session", "owner")
            .unwrap_err(),
        OAuthError::Expired
    );
}

#[test]
fn metadata_and_authorization_url_are_exact_allowlisted() {
    let (manager, _, _) = setup();
    let metadata = manager.metadata();
    assert_eq!(metadata, OAuthMetadata::openai_codex());
    let flow = begin(&manager);
    let url = Url::parse(&flow.authorization_url).unwrap();
    assert_eq!(url.origin().ascii_serialization(), OPENAI_ISSUER);
    let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["client_id"], OPENAI_CLIENT_ID);
    assert_eq!(pairs["redirect_uri"], OPENAI_REDIRECT);
    assert_eq!(pairs["scope"], OPENAI_SCOPE);
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert!(
        manager
            .begin(
                OPENAI_PROVIDER,
                "session",
                "owner",
                "http://127.0.0.1:1455/auth/callback",
                false
            )
            .is_err()
    );
}

#[test]
fn production_ports_are_usable_and_endpoints_are_exactly_allowlisted() {
    let clock = MonotonicOAuthClock::default();
    assert!(clock.now_seconds() <= 1);
    let http = ReqwestOAuthHttp::new().unwrap();
    for accepted in [OPENAI_TOKEN, OPENAI_DEVICE, OPENAI_DEVICE_TOKEN] {
        assert!(validate_production_endpoint(&Url::parse(accepted).unwrap()).is_ok());
    }
    for rejected in [
        "http://auth.openai.com/oauth/token",
        "https://evil.example/oauth/token",
        "https://auth.openai.com/oauth/token?next=evil",
        "https://auth.openai.com/oauth/authorize",
        "https://auth.openai.com:444/oauth/token",
    ] {
        assert_eq!(
            validate_production_endpoint(&Url::parse(rejected).unwrap()).unwrap_err(),
            OAuthError::InvalidInput
        );
    }
    let captured = format!("{http:?}");
    assert!(!captured.contains("token"));
}

#[test]
fn pending_flow_cap_is_exact_and_prunes_at_boundary() {
    let (manager, clock, _) = setup();
    let mut flows = Vec::new();
    for _ in 0..OAUTH_PENDING_FLOWS_MAX {
        flows.push(begin(&manager));
    }
    assert_eq!(
        manager
            .begin(OPENAI_PROVIDER, "session", "owner", OPENAI_REDIRECT, false)
            .unwrap_err(),
        OAuthError::Capacity
    );
    clock.set(OAUTH_STATE_TTL_SECONDS);
    assert!(
        manager
            .begin(OPENAI_PROVIDER, "session", "owner", OPENAI_REDIRECT, false)
            .is_ok()
    );
}

#[test]
fn never_logs_code_or_token() {
    let (manager, _, http) = setup();
    let flow = begin(&manager);
    let value = state(&flow);
    tokens(&http);
    let credential = manager
        .callback(
            &flow.flow_id,
            OPENAI_PROVIDER,
            "session",
            "owner",
            OAuthCallback {
                origin: "http://localhost:1455",
                redirect_uri: OPENAI_REDIRECT,
                query: &[("code", "marker-code"), ("state", &value)],
            },
        )
        .unwrap();
    let captured = format!(
        "{manager:?} {credential:?} {:?} {:?}",
        OAuthCredentialEnvelope(&credential),
        OAuthHttpResponse {
            status: 400,
            fields: [("error".into(), "marker-token".into())].into()
        }
    );
    for marker in [
        "marker-code",
        "marker-access",
        "marker-refresh",
        "marker-id",
        "marker-token",
        &value,
    ] {
        assert!(!captured.contains(marker));
    }
}
