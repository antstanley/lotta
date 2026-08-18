use lotta_domain::{BoundedJsonValue, NonEmptyString, RunId};
use serde::Serialize;

/// One of the exactly seven runtime broadcast payloads.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum RuntimeEvent {
    /// Approval control request.
    #[serde(rename = "control_request")]
    ControlRequest {
        /// Stable request identifier.
        request_id: NonEmptyString,
        /// Bounded control request body.
        request: BoundedJsonValue,
        /// Optional baseline compatibility agent identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<NonEmptyString>,
        /// Optional baseline compatibility conversation identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<NonEmptyString>,
    },
    /// Controller-owned external tool execution request.
    #[serde(rename = "controller_tool_request")]
    ControllerToolRequest {
        /// Stable provider call identifier.
        call_id: NonEmptyString,
        /// Captured lifecycle lease generation.
        lease_generation: u64,
        /// Bounded non-secret request body.
        request: BoundedJsonValue,
    },
    /// Context compaction request delegated to a production service.
    #[serde(rename = "compaction_request")]
    CompactionRequest {
        /// Captured lifecycle lease generation.
        lease_generation: u64,
        /// Estimated model-visible tokens before compaction.
        tokens_before: u64,
        /// Model-visible messages before compaction.
        messages_before: usize,
        /// Stable compaction reason.
        reason: NonEmptyString,
    },
    /// Authoritative device status snapshot.
    #[serde(rename = "update_device_status")]
    UpdateDeviceStatus {
        /// Bounded device status.
        device_status: BoundedJsonValue,
    },
    /// Authoritative loop status snapshot.
    #[serde(rename = "update_loop_status")]
    UpdateLoopStatus {
        /// Bounded loop status.
        loop_status: BoundedJsonValue,
    },
    /// Authoritative queue snapshot and ordered removals.
    #[serde(rename = "update_queue")]
    UpdateQueue {
        /// Bounded queue snapshot.
        queue: BoundedJsonValue,
        /// Bounded ordered queue-removal transitions.
        removed: BoundedJsonValue,
    },
    /// One bounded stream delta.
    #[serde(rename = "stream_delta")]
    StreamDelta {
        /// Bounded delta body.
        delta: BoundedJsonValue,
        /// Optional originating subagent.
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_id: Option<NonEmptyString>,
    },
    /// Exactly-once terminal turn event.
    #[serde(rename = "turn_finished")]
    TurnFinished {
        /// Turn identifier.
        turn_id: NonEmptyString,
        /// Optional run identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        /// Exact bounded stop-reason discriminant.
        stop_reason: NonEmptyString,
        /// Optional scrubbed error detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<NonEmptyString>,
    },
    /// Authoritative subagent snapshot.
    #[serde(rename = "update_subagent_state")]
    UpdateSubagentState {
        /// Bounded subagent list.
        subagents: BoundedJsonValue,
    },
}

impl RuntimeEvent {
    /// Returns the exact wire discriminant.
    #[must_use]
    pub const fn discriminant(&self) -> &'static str {
        match self {
            Self::ControlRequest { .. } => "control_request",
            Self::ControllerToolRequest { .. } => "controller_tool_request",
            Self::CompactionRequest { .. } => "compaction_request",
            Self::UpdateDeviceStatus { .. } => "update_device_status",
            Self::UpdateLoopStatus { .. } => "update_loop_status",
            Self::UpdateQueue { .. } => "update_queue",
            Self::StreamDelta { .. } => "stream_delta",
            Self::TurnFinished { .. } => "turn_finished",
            Self::UpdateSubagentState { .. } => "update_subagent_state",
        }
    }
}
