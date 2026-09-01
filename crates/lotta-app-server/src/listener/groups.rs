use super::{
    AppServerError, Arc, EventDeliveryBatch, ExternalToolsCommand, ExternalToolsMessage, HashMap,
    ListenerState, Ordering, OutboundBatch, TeleportCommand, TerminalCommand,
    ToolsUpdateResponseMessage, decode_agents, decode_conversations, decode_device,
    decode_external_tools, decode_files, decode_introspection, decode_memory, decode_models,
    decode_schedules, decode_settings, decode_skills, decode_teleport, decode_terminal,
    dispatch_value, mpsc,
};

/// Decodes and routes the external-tool command group for one frame.
pub(super) fn handle_external_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_external_tools(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_teleport_frame(state, connection_id, frame),
        Ok(Some(command)) => route_external_command(state, connection_id, &command),
    }
}

/// Decodes and routes the teleport command group for one frame.
pub(super) fn handle_teleport_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_teleport(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_terminal_frame(state, connection_id, frame),
        Ok(Some(command)) => route_teleport_command(state, connection_id, &command),
    }
}

/// Decodes and routes the terminal command group for one frame.
pub(super) fn handle_terminal_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_terminal(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_files_frame(state, connection_id, frame),
        Ok(Some(command)) => route_terminal_command(state, connection_id, &command),
    }
}

/// Decodes and routes the files command group for one frame.
pub(super) fn handle_files_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_files(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_memory_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.files.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the memory command group for one frame. Handlers run in
/// detached tasks; the memory bridge owns no per-connection resources, so
/// connection cleanup needs no memory step.
pub(super) fn handle_memory_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_memory(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_models_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.memories.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the models/providers command group for one frame.
pub(super) fn handle_models_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_models(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_schedules_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.models.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the schedules command group for one frame. Handlers run
/// in detached tasks; the schedules bridge owns no per-connection resources,
/// so connection cleanup needs no schedules step.
pub(super) fn handle_schedules_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_schedules(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_skills_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.schedules.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the skills command group for one frame. Handlers run in
/// detached tasks; the skills bridge owns no per-connection resources, so
/// connection cleanup needs no skills step.
pub(super) fn handle_skills_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_skills(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_settings_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.skills.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the settings command group for one frame. Handlers run
/// in detached tasks; the settings bridge owns no per-connection resources,
/// so connection cleanup needs no settings step.
pub(super) fn handle_settings_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_settings(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_agents_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.settings.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the agent management command group for one frame.
/// Handlers run in detached tasks; the agents bridge owns no per-connection
/// resources, so connection cleanup needs no agents step.
pub(super) fn handle_agents_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_agents(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_conversations_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.agents.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the conversation management command group for one
/// frame. Handlers run in detached tasks; the conversations bridge owns no
/// per-connection resources, so connection cleanup needs no conversations step.
pub(super) fn handle_conversations_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_conversations(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_device_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.conversations.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the device command group for one frame. Handlers run in
/// detached tasks; the device bridge owns no per-connection resources, so
/// connection cleanup needs no device step.
pub(super) fn handle_device_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_device(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_introspection_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.devices.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the introspection group for one frame. This is the
/// final group of the decode chain.
pub(super) fn handle_introspection_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_introspection(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => true,
        Ok(Some(command)) => state.introspection.handle(connection_id, &command),
    }
}

pub(super) fn route_terminal_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &TerminalCommand,
) -> bool {
    match command {
        TerminalCommand::Spawn(spawn) => {
            state.terminals.spawn(connection_id, spawn);
            true
        }
        TerminalCommand::Input(input) => {
            state.terminals.input(connection_id, input);
            true
        }
        TerminalCommand::Resize(resize) => {
            state.terminals.resize(connection_id, resize);
            true
        }
        TerminalCommand::Kill(kill) => {
            state.terminals.kill(connection_id, kill);
            true
        }
    }
}

pub(super) fn route_teleport_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &TeleportCommand,
) -> bool {
    match command {
        TeleportCommand::Probe(probe) => {
            state.teleports.probe(connection_id, probe);
            true
        }
        TeleportCommand::Request(request) => {
            // The compatibility listener owns no runtime registry, so no turn
            // can be processing through it; scopes answer ready immediately.
            state.teleports.request(connection_id, request, false);
            true
        }
        TeleportCommand::Failed(failure) => {
            state.teleports.failed(failure);
            true
        }
    }
}

pub(super) fn route_external_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &ExternalToolsCommand,
) -> bool {
    match command {
        ExternalToolsCommand::ToolsUpdate(update) => {
            let outcome = state.external_tools.apply_update(connection_id, update);
            let message = ExternalToolsMessage::UpdateResponse(ToolsUpdateResponseMessage {
                request_id: update.request_id.as_str().to_owned(),
                success: outcome.is_ok(),
                error: outcome.err(),
            });
            dispatch_value(state, connection_id, &message).is_ok()
        }
        ExternalToolsCommand::CallResponse(response) => {
            let _ = state
                .external_tools
                .handle_response(connection_id, response);
            true
        }
    }
}

pub(super) fn dispatch_event_batch(
    state: &ListenerState,
    scope: lotta_domain::RuntimeScope,
    event: crate::ws::RuntimeEvent,
    deliveries: &EventDeliveryBatch,
) -> Result<(), AppServerError> {
    dispatch_deliveries(&state.outbound, deliveries)?;
    // Observation is post-dispatch and cannot affect routing. Once the ordinal space is exhausted,
    // retain the saturated counter and skip all later observations rather than wrapping it.
    let ordinal = state
        .next_observation
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_add(1)
        })
        .ok();
    let Some(ordinal) = ordinal else {
        return Ok(());
    };
    let value = crate::observer::observation(ordinal, scope, event, deliveries);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.observer.observe(value);
    }));
    Ok(())
}

pub(super) fn dispatch_deliveries(
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>,
    deliveries: &EventDeliveryBatch,
) -> Result<(), AppServerError> {
    let senders = {
        let outbound = outbound.lock().map_err(|_| AppServerError::Internal)?;
        deliveries
            .as_slice()
            .iter()
            .map(|delivery| {
                outbound
                    .get(&delivery.connection_id)
                    .cloned()
                    .ok_or(AppServerError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    for (delivery, sender) in deliveries.as_slice().iter().zip(senders) {
        let body = serde_json::to_string(&delivery.frame).map_err(|_| AppServerError::Internal)?;
        sender
            .try_send(vec![body])
            .map_err(|_| AppServerError::Unavailable)?;
    }
    Ok(())
}

pub(super) fn dispatch_typed_failure(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> Result<(), AppServerError> {
    let Some(request_id) = frame.request_id.clone() else {
        return Ok(());
    };
    let runtime = frame
        .value
        .get("runtime")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let response = match frame.value.get("type").and_then(serde_json::Value::as_str) {
        Some("runtime_start") => crate::ws::ConnectionResponse::RuntimeStart {
            request_id,
            success: false,
            runtime: None,
            agent: None,
            conversation: None,
            created: crate::ws::router::CreatedFlags {
                agent: false,
                conversation: false,
            },
            error: Some("runtime service unavailable".into()),
        },
        Some("sync") => crate::ws::ConnectionResponse::SyncResponse {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            success: false,
            error: Some("runtime service unavailable".into()),
        },
        Some("abort_message") => crate::ws::ConnectionResponse::AbortMessage {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            aborted: false,
            success: false,
            error: Some("runtime service unavailable".into()),
        },
        Some("input") => crate::ws::ConnectionResponse::InputAccepted {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            accepted: false,
            disposition: None,
            error: Some("runtime service unavailable".into()),
        },
        _ => return Ok(()),
    };
    dispatch_value(state, connection_id, &response)
}
