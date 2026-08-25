//! §Outbound message groups coverage model.
//!
//! The six §Outbound message groups rows name their member message types in
//! prose; this module resolves each row against the checked-in pinned protocol
//! fixture so coverage tests assert emissions against the exact fixture
//! message list instead of hard-coded strings. Every row must resolve to at
//! least one live fixture discriminant — if the pinned union renames or drops
//! a member, resolution fails loudly rather than passing vacuously.

use serde_json::Value;

/// Members named by the §Outbound message groups Control row.
pub const CONTROL_ROW: [&str; 2] = ["control_request", "external_tool_call_request"];
/// Members named by the §Outbound message groups Admission row.
pub const ADMISSION_ROW: [&str; 4] = [
    "input_accepted",
    "abort_message_response",
    "sync_response",
    "runtime_start_response",
];
/// Members named by the §Outbound message groups State row.
pub const STATE_ROW: [&str; 12] = [
    "update_device_status",
    "update_loop_status",
    "update_queue",
    "update_subagent_state",
    "memory_updated",
    "skills_updated",
    "crons_updated",
    "channels_updated",
    "channel_accounts_updated",
    "channel_pairings_updated",
    "channel_routes_updated",
    "channel_targets_updated",
];
/// Members named by the §Outbound message groups Stream row.
pub const STREAM_ROW: [&str; 1] = ["stream_delta"];
/// Members named by the §Outbound message groups Terminal row.
pub const TERMINAL_ROW: [&str; 4] = [
    "turn_finished",
    "terminal_spawned",
    "terminal_output",
    "terminal_exited",
];
/// Members named by the §Outbound message groups Management row.
pub const MANAGEMENT_ROW: [&str; 14] = [
    "app_server_info_response",
    "agent_list_response",
    "agent_retrieve_response",
    "conversation_list_response",
    "list_models_response",
    "connect_provider_response",
    "disconnect_provider_response",
    "list_memory_response",
    "cron_list_response",
    "channels_list_response",
    "get_tree_response",
    "read_file_response",
    "search_branches_response",
    "secret_list_response",
];

/// Loads the pinned fixture's full outbound message discriminant list.
///
/// # Panics
/// Panics if the checked-in fixture fails to parse or lacks its messages
/// list, which would mean the pinned-protocol contract drifted.
#[must_use]
pub fn fixture_message_discriminants() -> Vec<String> {
    let raw = include_str!("../../../../fixtures/protocol/discriminants.json");
    let fixture: Value = serde_json::from_str(raw).expect("checked-in fixture parses");
    fixture["messages"]["discriminants"]
        .as_array()
        .expect("fixture carries a messages list")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

/// Intersects one row's named members with the fixture message list.
///
/// # Panics
/// Panics when no row member survives the intersection, which means the spec
/// row and the pinned fixture have drifted apart.
#[must_use]
pub fn members_in_fixture(row: &[&str]) -> Vec<String> {
    let fixture = fixture_message_discriminants();
    let resolved: Vec<String> = row
        .iter()
        .filter(|member| fixture.iter().any(|entry| entry == *member))
        .map(|member| (*member).to_owned())
        .collect();
    assert!(
        !resolved.is_empty(),
        "§Outbound message groups row {row:?} must resolve against the pinned fixture"
    );
    resolved
}

#[cfg(test)]
#[path = "outbound_group_coverage_tests.rs"]
mod group_coverage;
