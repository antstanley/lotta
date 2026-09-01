use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, Request},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lotta_domain::Clock;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

mod turn_supervisor;

use crate::{
    auth::origin,
    bounds::{HTTP_BODY_BYTES_MAX, WS_FRAME_BYTES_MAX, WS_PING_INTERVAL_MS},
    config::{PreparedServer, is_loopback_host},
    error::AppServerError,
    heartbeat::Heartbeat,
    ws::{
        EventDeliveryBatch, RandomEventIdGenerator, RouterEventSink, RuntimeCommandService,
        RuntimeRouter, ServiceBackedTurnController, TurnController,
        UnsupportedRuntimeCommandService,
        agents::{AgentsBridge, decode as decode_agents},
        conversations::{ConversationsBridge, decode as decode_conversations},
        device::{DeviceBridge, decode as decode_device},
        external_tools::{
            ExternalForwarder, ExternalToolBridge, ExternalToolsCommand, ExternalToolsMessage,
            ToolsUpdateResponseMessage, decode as decode_external_tools,
        },
        files::{FilesBridge, decode as decode_files},
        introspection::{IntrospectionBridge, decode as decode_introspection},
        lock_router,
        memory::{MemoryBridge, decode as decode_memory},
        models::{ModelsBridge, decode as decode_models},
        route_command,
        schedules::{SchedulesBridge, decode as decode_schedules},
        settings::{SettingsBridge, decode as decode_settings},
        skills::{SkillsBridge, decode as decode_skills},
        teleport::{TeleportBridge, TeleportCommand, TeleportForwarder, decode as decode_teleport},
        terminal::{TerminalBridge, TerminalCommand, TerminalForwarder, decode as decode_terminal},
    },
};

#[cfg(test)]
#[path = "listener/tests/heartbeat.rs"]
mod heartbeat_integration;
#[cfg(test)]
#[path = "listener/tests/observer.rs"]
mod observer_tests;
#[cfg(test)]
#[path = "listener/tests/reconnect_security.rs"]
mod reconnect_security;
#[cfg(test)]
#[path = "listener/tests/transport.rs"]
mod transport;
#[cfg(test)]
#[path = "listener/tests/url_resolution.rs"]
mod url_resolution;
#[cfg(test)]
#[path = "listener/tests/websocket.rs"]
mod websocket;

/// Compatibility re-export of the canonical WebSocket frame ceiling.
pub use crate::bounds::WS_FRAME_BYTES_MAX as FRAME_BYTES_MAX;

mod groups;
mod helpers;
mod http;
mod lifecycle;
mod runtime_dispatch;
mod session;

use groups::{dispatch_event_batch, dispatch_typed_failure, handle_external_frame};
use helpers::{format_bind_address, method_not_allowed, not_found, resolved_urls, send_close};
use http::{build_router, initialize_socket_liveness, reject_stale_channel_socket};
use lifecycle::{ListenerState, OutboundBatch, SharedOutbound};
use runtime_dispatch::{
    StorageBridges, channel_runtime_command_allowed, compose_device_bridges,
    compose_storage_bridges, dispatch_output, dispatch_value, event_sink, external_forwarder,
    handle_channel_tool_response, introspection_forwarder, publish_channel_runtime_tool,
    send_outbound_batch, settings_forwarder, skills_forwarder, teleport_forwarder,
    terminal_forwarder,
};
use session::{
    publish_expired_subscription_updates, publish_shutdown_subscription_updates, serve_socket,
};

#[cfg(test)]
use groups::dispatch_deliveries;
#[cfg(test)]
use helpers::{test_forbidden, test_internal, test_unavailable};
#[cfg(test)]
use lifecycle::{
    SocketLimits, start_listener_for_test, start_listener_with_state_for_test, test_openai_chat,
    test_openai_responses,
};
#[cfg(test)]
use session::websocket_error_close;

#[cfg(test)]
pub(crate) use http::RECONNECT_CLIENT_ID_BYTES_MAX;
pub use http::WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX;
pub use lifecycle::{
    ListenerHandle, SharedGroupBridges, start_listener, start_listener_with_runtime_service,
    start_listener_with_runtime_service_and_controller,
    start_listener_with_runtime_service_and_observer,
    start_listener_with_runtime_service_controller_observer_and_bridges,
};

#[cfg(test)]
pub(crate) use lifecycle::start_listener_with_responses_for_test;
