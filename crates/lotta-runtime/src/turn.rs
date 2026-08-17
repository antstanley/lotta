//! Minimal owner-local provider/tool turn loop.

mod effects;
mod projection;
mod step;
mod tool_calls;
#[path = "turn/loop.rs"]
mod turn_loop;

pub use effects::{ToolResultRecord, TurnEffectPort, TurnEvent};
pub use projection::{ProjectionKind, TurnProjection};
pub use tool_calls::TurnToolCatalog;
pub use turn_loop::{
    ConfiguredFallback, ProviderTurnExecutorPort, TurnPorts, TurnProvider, TurnRunOutcome, run_turn,
};

#[cfg(test)]
#[path = "turn/bounds.rs"]
mod bounds;
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
#[path = "turn/sequential_tools.rs"]
mod sequential_tools;
#[cfg(test)]
#[path = "turn/terminal_once.rs"]
mod terminal_once;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "turn/tool_call_assembly.rs"]
mod tool_call_assembly;
