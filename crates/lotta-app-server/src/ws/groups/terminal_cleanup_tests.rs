//! `ws::terminal::cleanup` certificate selector: connection drop kills its
//! terminals and leaves the process group empty.

use super::support::{
    CONNECTION_A, bridge, fake_clock, group_live, input_command, spawn_command, wait_spawned,
    wait_until,
};

const TERMINAL: &str = "term";

#[cfg(unix)]
#[tokio::test]
async fn disconnect_kills_sessions_and_process_group() {
    let (bridge, messages) = bridge(fake_clock());

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    let pid = wait_spawned(&bridge, &messages, TERMINAL).await;
    // A background descendant joins the session's process group, proving the
    // group-directed cleanup reaches beyond the direct shell.
    bridge.input(CONNECTION_A, &input_command(TERMINAL, "sleep 3600 &\n"));
    wait_until(|| descendant_joined(pid)).await;
    assert!(group_live(pid), "fixture group must be live before cleanup");

    bridge.disconnect(CONNECTION_A);

    assert_eq!(
        bridge.sessions_len(),
        0,
        "disconnect must drop every owned session record"
    );
    wait_until(|| !group_live(pid)).await;
    assert!(
        !group_live(pid),
        "process group must be empty after connection cleanup"
    );
}

#[cfg(unix)]
fn descendant_joined(group: u32) -> bool {
    // The shell plus at least one background descendant share the group.
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pgid="])
        .output()
        .expect("inspect process groups");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .filter(|row_group| *row_group == group)
        .count()
        >= 2
}

#[cfg(not(unix))]
#[test]
fn unsupported_platform_is_explicit() {
    assert_eq!(
        std::env::consts::FAMILY,
        "unix",
        "terminal cleanup requires Unix"
    );
}
