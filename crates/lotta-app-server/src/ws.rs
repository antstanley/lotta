//! Typed, bounded Runtime WebSocket commands, events, and routing.

/// Runtime command wire models and decoding.
pub mod command;
/// Connection subscriptions and per-connection sequencing.
pub mod connection;
/// Runtime event lifecycle envelopes.
pub mod envelope;
/// Exact runtime broadcast event models.
pub mod event;
/// External-tool command group bridging wire frames to the Task 41 registry.
#[path = "ws/groups/external_tools.rs"]
pub mod external_tools;
/// Files command group bridging wire frames to confined workspace services.
#[path = "ws/groups/files.rs"]
pub mod files;
/// Memory command group bridging wire frames to the Task 29 memory repository.
#[path = "ws/groups/memory.rs"]
pub mod memory;
/// Synchronous owner-local routing and deferred input application.
pub mod router;
/// Injectable Runtime command service seam.
pub mod service;
/// Teleport command group bridging wire frames to pending teleport state.
#[path = "ws/groups/teleport.rs"]
pub mod teleport;
/// Terminal command group bridging wire frames to interactive shell sessions.
#[path = "ws/groups/terminal.rs"]
pub mod terminal;

pub use command::RuntimeCommand;
pub use connection::{ConnectionId, EventDelivery, RuntimeConnections};
pub use envelope::{EventIdGenerator, RandomEventIdGenerator, StampedRuntimeEvent};
pub use event::RuntimeEvent;
pub use router::{
    ConnectionResponse, DeferredInput, EventDeliveryBatch, RouteAdmission, RouteOutput,
    RoutedEventBatch, RoutedEventBatches, RouterEventSink, RuntimeRouter, lock_router,
    route_command,
};
pub use service::{
    RuntimeCommandService, RuntimeEventSink, ServiceBackedTurnController, TurnController,
    UnsupportedRuntimeCommandService, WS_RUNTIME_ROUTE_EVENTS_MAX,
};

#[cfg(test)]
#[path = "ws/ordering_tests.rs"]
mod ordering_invariants;
#[cfg(test)]
#[path = "ws/runtime_group_tests.rs"]
mod runtime_group;
#[cfg(test)]
#[path = "ws/subscriptions_tests.rs"]
mod subscriptions;
#[cfg(test)]
#[path = "ws/test_support.rs"]
mod test_support;
