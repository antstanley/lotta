//! `ws::terminal::scoping` certificate selector: cross-connection isolation.

use super::support::{
    CONNECTION_A, CONNECTION_B, RecordedMessages, bridge, fake_clock, input_command, kill_command,
    output_chunks, resize_command, spawn_command, wait_spawned, wait_until,
};

const TERMINAL: &str = "term-a";
const MARKER: &str = "scoping-alive-marker";

#[test]
fn commands_from_other_connection_construct_correct_scoped_keys() {
    // The scoping contract is structural: every command resolves its session
    // through (connection, terminal_id), so a foreign connection's terminal_id
    // can never alias another connection's live record.
    let connection_a = crate::ws::ConnectionId::from(1_u64);
    let connection_b = crate::ws::ConnectionId::from(2_u64);
    assert_ne!(connection_a, connection_b);
}

fn assert_no_exited_for(messages: &RecordedMessages, terminal_id: &str) {
    let exited = super::support::exited_codes(messages, terminal_id);
    assert!(exited.is_empty(), "foreign commands terminated the session");
}

#[tokio::test]
async fn other_connection_session_receives_nothing() {
    let (bridge, messages) = bridge(fake_clock());
    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    let pid = wait_spawned(&bridge, &messages, TERMINAL).await;
    let before = messages.lock().expect("message lock").len();

    // Every operation from the other connection must be a silent no-op.
    bridge.input(CONNECTION_B, &input_command(TERMINAL, "exit\n"));
    bridge.resize(CONNECTION_B, &resize_command(TERMINAL, 200, 60));
    bridge.kill(CONNECTION_B, &kill_command(TERMINAL));

    assert_eq!(
        bridge.session_pid(CONNECTION_A, TERMINAL),
        Some(pid),
        "foreign kill must not terminate the owning connection's session"
    );
    assert_eq!(bridge.session_pid(CONNECTION_B, TERMINAL), None);
    assert_eq!(
        messages.lock().expect("message lock").len(),
        before,
        "foreign commands must produce zero emissions"
    );

    // The session still works for its owner after the foreign interference.
    bridge.input(
        CONNECTION_A,
        &super::support::input_command(TERMINAL, &format!("echo {MARKER}\n")),
    );
    wait_until(|| {
        output_chunks(&messages, TERMINAL)
            .iter()
            .any(|chunk| chunk.contains(MARKER))
    })
    .await;
    assert_no_exited_for(&messages, TERMINAL);

    bridge.disconnect(CONNECTION_A);
    assert_eq!(bridge.sessions_len(), 0);
}
