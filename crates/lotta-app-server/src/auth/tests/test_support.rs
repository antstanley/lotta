use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use lotta_domain::{Clock, DomainError, Timestamp};
use serde_json::{Value, json};
use sha2::Sha256;

use super::{AppServerError, AuthPolicy};
use crate::config::ServerArgs;

pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        let text = format!("1970-01-01T00:{:02}:{:02}Z", self.0 / 60, self.0 % 60);
        Timestamp::parse_persisted_rfc3339(&text).unwrap_or_else(|_| panic!("valid fixed clock"))
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

pub fn temp_file(contents: &[u8]) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "lotta-auth-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, contents).unwrap_or_else(|_| panic!("write test secret"));
    path
}

pub fn signed_args(secret: &Path) -> ServerArgs {
    ServerArgs {
        listen_enabled: true,
        ws_auth: Some("signed-bearer-token".into()),
        ws_shared_secret_file: Some(secret.to_path_buf()),
        ..ServerArgs::default()
    }
}

pub fn signed_policy(secret: &[u8]) -> AuthPolicy {
    let path = temp_file(secret);
    let policy = AuthPolicy::prepare(&signed_args(&path)).unwrap_or_else(|_| panic!("prepare"));
    let _ = fs::remove_file(path);
    policy
}

pub fn token(claims: &Value, secret: &[u8]) -> String {
    token_with_header(&json!({"alg":"HS256","typ":"JWT"}), claims, secret)
}

pub fn token_with_header(header: &Value, claims: &Value, secret: &[u8]) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap_or_default());
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let message = format!("{header}.{claims}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap_or_else(|_| panic!("hmac key"));
    mac.update(message.as_bytes());
    format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

pub fn headers(value: &[u8]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_bytes(value).unwrap_or_else(|_| panic!("header bytes"));
    headers.insert(AUTHORIZATION, value);
    headers
}

pub fn authorize(policy: &AuthPolicy, token: &str, now: i64) -> Result<(), AppServerError> {
    policy.authorize(
        &headers(format!("Bearer {token}").as_bytes()),
        &FixedClock(now),
    )
}
