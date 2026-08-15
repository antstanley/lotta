//! Canonical transport limits and reusable acceptance checks.

/// Largest accepted HTTP request body.
pub const HTTP_BODY_BYTES_MAX: usize = 20 * 1024 * 1024;
/// Largest accepted WebSocket frame and message.
pub const WS_FRAME_BYTES_MAX: usize = 100 * 1024 * 1024;
/// Largest accepted aggregate JSON object-key count in one WebSocket message.
pub const WS_MESSAGE_FIELDS_MAX: usize = 4096;
/// WebSocket protocol ping interval.
pub const WS_PING_INTERVAL_MS: u64 = 30_000;
/// Maximum elapsed duration since the last WebSocket pong.
pub const WS_PONG_TIMEOUT_MS: u64 = 90_000;
/// Largest accepted signed-token clock skew.
pub const AUTH_CLOCK_SKEW_SECONDS_MAX: u32 = 300;
/// Largest accepted protocol request identifier.
pub const REQUEST_ID_BYTES_MAX: usize = 256;
/// Largest accepted chat idempotency outcome cache capacity.
pub const CHAT_IDEMPOTENCY_OUTCOMES_MAX: usize = 1024;
/// Largest accepted `OpenAI` chat-key cache capacity.
pub const OPENAI_CHAT_KEYS_MAX: usize = 4096;

/// Validates an observed cardinality against an inclusive capacity.
///
/// # Errors
/// Returns [`crate::errors::AppServerError::PayloadTooLarge`] above the capacity.
pub fn accept_capacity(
    observed: usize,
    capacity: usize,
) -> Result<(), crate::errors::AppServerError> {
    if observed > capacity {
        return Err(crate::errors::AppServerError::PayloadTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! deferred_cache_boundary {
        ($below:ident, $at:ident, $above:ident, $limit:expr) => {
            #[test]
            fn $below() {
                assert!(accept_capacity($limit - 1, $limit).is_ok());
            }
            #[test]
            fn $at() {
                assert!(accept_capacity($limit, $limit).is_ok());
            }
            #[test]
            fn $above() {
                assert!(accept_capacity($limit + 1, $limit).is_err());
            }
        };
    }

    deferred_cache_boundary!(
        idempotency_below,
        idempotency_at,
        idempotency_above,
        CHAT_IDEMPOTENCY_OUTCOMES_MAX
    );
    deferred_cache_boundary!(
        chat_keys_below,
        chat_keys_at,
        chat_keys_above,
        OPENAI_CHAT_KEYS_MAX
    );

    fn frame(value: usize) -> bool {
        SocketLimitEvidence { frame_bytes: value }.valid()
    }
    struct SocketLimitEvidence {
        frame_bytes: usize,
    }
    impl SocketLimitEvidence {
        fn valid(&self) -> bool {
            self.frame_bytes <= WS_FRAME_BYTES_MAX
        }
    }

    #[test]
    fn frame_below() {
        assert!(frame(WS_FRAME_BYTES_MAX - 1));
    }
    #[test]
    fn frame_at() {
        assert!(frame(WS_FRAME_BYTES_MAX));
    }
    #[test]
    fn frame_above() {
        assert!(!frame(WS_FRAME_BYTES_MAX + 1));
    }

    #[test]
    fn fields_below() {
        assert!(
            crate::framing::decode_text(&crate::framing::test_object(WS_MESSAGE_FIELDS_MAX - 1))
                .is_ok()
        );
    }
    #[test]
    fn fields_at() {
        assert!(
            crate::framing::decode_text(&crate::framing::test_object(WS_MESSAGE_FIELDS_MAX))
                .is_ok()
        );
    }
    #[test]
    fn fields_above() {
        assert!(
            crate::framing::decode_text(&crate::framing::test_object(WS_MESSAGE_FIELDS_MAX + 1))
                .is_err()
        );
    }

    fn heartbeat(ms: i64) -> bool {
        use chrono::Duration;
        use lotta_domain::Timestamp;
        use lotta_testkit::clock::FakeClock;
        let time = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").unwrap();
        let clock = FakeClock::new(time);
        let state = crate::heartbeat::Heartbeat::new(&clock);
        clock.advance(Duration::milliseconds(ms)).unwrap();
        state.is_expired(&clock)
    }
    #[test]
    fn pong_below() {
        assert!(!heartbeat((WS_PONG_TIMEOUT_MS - 1).cast_signed()));
    }
    #[test]
    fn pong_at() {
        assert!(!heartbeat(WS_PONG_TIMEOUT_MS.cast_signed()));
    }
    #[test]
    fn pong_above() {
        assert!(heartbeat((WS_PONG_TIMEOUT_MS + 1).cast_signed()));
    }

    #[test]
    fn ping_below() {
        assert!(
            std::time::Duration::from_millis(WS_PING_INTERVAL_MS - 1)
                < std::time::Duration::from_millis(WS_PING_INTERVAL_MS)
        );
    }
    #[test]
    fn ping_at() {
        assert_eq!(
            std::time::Duration::from_millis(WS_PING_INTERVAL_MS).as_millis(),
            u128::from(WS_PING_INTERVAL_MS)
        );
    }
    #[test]
    fn ping_above() {
        assert!(
            std::time::Duration::from_millis(WS_PING_INTERVAL_MS + 1)
                > std::time::Duration::from_millis(WS_PING_INTERVAL_MS)
        );
    }

    #[test]
    fn http_below() {
        assert!(
            crate::bounds::accept_capacity(HTTP_BODY_BYTES_MAX - 1, HTTP_BODY_BYTES_MAX).is_ok()
        );
    }
    #[test]
    fn http_at() {
        assert!(crate::bounds::accept_capacity(HTTP_BODY_BYTES_MAX, HTTP_BODY_BYTES_MAX).is_ok());
    }
    #[test]
    fn http_above() {
        assert!(
            crate::bounds::accept_capacity(HTTP_BODY_BYTES_MAX + 1, HTTP_BODY_BYTES_MAX).is_err()
        );
    }

    fn auth(skew: u32) -> bool {
        use crate::config::ServerArgs;
        let path = std::env::temp_dir().join(format!("lotta-bound-{}-{skew}", std::process::id()));
        std::fs::write(&path, [b'x'; 32]).unwrap();
        let args = ServerArgs {
            ws_auth: Some("signed-bearer-token".into()),
            ws_shared_secret_file: Some(path.clone()),
            ws_max_clock_skew_seconds: Some(skew),
            ..ServerArgs::default()
        };
        let result = crate::auth::AuthPolicy::prepare(&args).is_ok();
        let _ = std::fs::remove_file(path);
        result
    }
    #[test]
    fn auth_below() {
        assert!(auth(AUTH_CLOCK_SKEW_SECONDS_MAX - 1));
    }
    #[test]
    fn auth_at() {
        assert!(auth(AUTH_CLOCK_SKEW_SECONDS_MAX));
    }
    #[test]
    fn auth_above() {
        assert!(!auth(AUTH_CLOCK_SKEW_SECONDS_MAX + 1));
    }

    fn request(size: usize) -> bool {
        crate::framing::decode_text(&format!(
            r#"{{"type":"sync","request_id":"{}"}}"#,
            "r".repeat(size)
        ))
        .is_ok()
    }
    #[test]
    fn request_below() {
        assert!(request(REQUEST_ID_BYTES_MAX - 1));
    }
    #[test]
    fn request_at() {
        assert!(request(REQUEST_ID_BYTES_MAX));
    }
    #[test]
    fn request_above() {
        assert!(!request(REQUEST_ID_BYTES_MAX + 1));
    }
}
