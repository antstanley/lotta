//! Typed, bounded Runtime WebSocket commands, events, and routing.

/// Agent management command group bridging wire frames to Tasks 23, 28, 29,
/// 30, and 54.
#[path = "ws/groups/agents.rs"]
pub mod agents;
/// Runtime command wire models and decoding.
pub mod command;
/// Connection subscriptions and per-connection sequencing.
pub mod connection;
/// Conversation management command group bridging wire frames to Tasks 23/24,
/// 28, 30, and 58.
#[path = "ws/groups/conversations.rs"]
pub mod conversations;
/// Device command group bridging wire frames to Tasks 18 and 45 plus git and
/// secrets surfaces.
#[path = "ws/groups/device.rs"]
pub mod device;
/// Confined git operations and the local-backend secrets store behind the
/// device command group.
#[path = "ws/groups/device_support.rs"]
pub mod device_support;
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
/// Introspection command group serving authenticated capability discovery.
#[path = "ws/groups/introspection.rs"]
pub mod introspection;
/// Memory command group bridging wire frames to the Task 29 memory repository.
#[path = "ws/groups/memory.rs"]
pub mod memory;
/// Models/providers command group bridging wire frames to Tasks 47, 52, and 32.
#[path = "ws/groups/models.rs"]
pub mod models;
/// §Outbound message groups coverage model over the pinned protocol fixture.
pub mod outbound;
/// Synchronous owner-local routing and deferred input application.
pub mod router;
/// Schedules command group bridging wire frames to Tasks 60 and 61.
#[path = "ws/groups/schedules.rs"]
pub mod schedules;
/// Injectable Runtime command service seam.
pub mod service;
/// Settings command group bridging wire frames to the Task 27 side store.
#[path = "ws/groups/settings.rs"]
pub mod settings;
/// Skills command group bridging wire frames to Task 36 skill selection.
#[path = "ws/groups/skills.rs"]
pub mod skills;
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
