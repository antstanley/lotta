//! Minimal owner-local provider/tool turn loop.

mod effects;
mod projection;
/// Production turn setup orchestration and admission boundary.
pub mod setup;
mod setup_run;
mod setup_steps;
mod step;
mod stop_reason;
mod tool_calls;
#[path = "turn/loop.rs"]
mod turn_loop;

pub use effects::{
    CompactionRequest, ControlRequest, ControllerToolRequestRecord, ToolResultRecord,
    TurnEffectPort, TurnEvent,
};
pub use projection::{ProjectionKind, TurnProjection};
pub use setup::{
    AdmissionReceipt, CwdFailure, CwdResolution, ExtensionSnapshot, ReminderClaim,
    ResolvedTurnModel, SetupError, SetupFailure, SetupInput, SetupOrchestrator, SetupOutput,
    SetupPorts, SetupScopeHandle, SetupStage, SetupStatus, SetupStatusSink, SetupToolSource,
    SkillInventory, ToolCandidate,
};
pub use setup_run::{TurnSetup, run_turn_with_setup};
pub use stop_reason::{ProviderFailureDetail, TurnStopReason, TurnStopRecord};
pub use tool_calls::{ToolSnapshotHandle, TurnToolCatalog};
pub use turn_loop::{
    ApprovalPort, ApprovalResolution, CONTEXT_OVERFLOW_COMPACTIONS_MAX, CompactionPort,
    CompactionProgress, ConfiguredFallback, ControllerToolPort, ProviderStartPort,
    ProviderTurnExecutorPort, RequestRefreshPort, TurnPorts, TurnProvider, TurnRunOutcome,
    run_turn,
};

#[cfg(test)]
#[path = "turn/bounds.rs"]
mod bounds;
#[cfg(test)]
#[path = "turn/branches.rs"]
mod branches;
#[cfg(test)]
#[path = "turn/context_pressure.rs"]
mod context_pressure;
#[cfg(test)]
#[path = "turn/fallback_preserves_model.rs"]
mod fallback_preserves_model;
#[cfg(test)]
#[path = "turn/projections.rs"]
mod projections;
#[cfg(test)]
#[path = "turn/protocol_failures.rs"]
mod protocol_failures;
#[cfg(test)]
#[path = "turn/queue_vertical_slice.rs"]
mod queue_vertical_slice;
#[cfg(test)]
#[path = "turn/retryable_error.rs"]
mod retryable_error;
#[cfg(test)]
#[path = "turn/sequential_tools.rs"]
mod sequential_tools;
#[cfg(test)]
#[path = "turn/setup_tests.rs"]
mod setup_tests;
#[cfg(test)]
#[path = "turn/stop_reasons.rs"]
mod stop_reasons;
#[cfg(test)]
#[path = "turn/terminal_once.rs"]
mod terminal_once;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "turn/tool_call_assembly.rs"]
mod tool_call_assembly;
