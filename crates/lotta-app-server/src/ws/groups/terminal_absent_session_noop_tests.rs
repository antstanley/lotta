//! `ws::terminal::absent_session_noop` certificate selector: input and resize
//! against an unknown `terminal_id` are silent no-ops.

use super::support::{CONNECTION_A, bridge, count_kind, fake_clock, input_command, resize_command};

const UNKNOWN: &str = "never-spawned";

#[tokio::test]
async fn input_for_absent_session_emits_nothing() {
    let (bridge, messages) = bridge(fake_clock());

    bridge.input(CONNECTION_A, &input_command(UNKNOWN, "echo ignored\n"));

    assert_eq!(bridge.sessions_len(), 0);
    assert_eq!(count_kind(&messages, "terminal_output"), 0);
    assert_eq!(count_kind(&messages, "terminal_spawned"), 0);
    assert_eq!(count_kind(&messages, "terminal_exited"), 0);
}

#[tokio::test]
async fn resize_for_absent_session_emits_nothing() {
    let (bridge, messages) = bridge(fake_clock());

    bridge.resize(CONNECTION_A, &resize_command(UNKNOWN, 120, 40));

    assert_eq!(bridge.sessions_len(), 0);
    let guard = messages.lock().expect("message lock");
    assert!(
        guard.is_empty(),
        "absent-session resize must not respond with any frame, including errors"
    );
}
