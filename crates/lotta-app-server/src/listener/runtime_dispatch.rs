use super::{
    AgentsBridge, AppServerError, Arc, Clock, ConversationsBridge, DeviceBridge,
    EventDeliveryBatch, ExternalForwarder, FilesBridge, HashMap, ListenerState, MemoryBridge,
    Message, ModelsBridge, OutboundBatch, PreparedServer, RouterEventSink, SchedulesBridge,
    SharedOutbound, TeleportForwarder, TerminalForwarder, WebSocket, dispatch_event_batch, mpsc,
};

pub(super) fn handle_channel_tool_response(
    state: &ListenerState,
    principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    let (Some(manager), Some(principal)) = (&state.channel_tools, principal) else {
        return false;
    };
    let Some(runtime) = frame
        .value
        .get("runtime")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let (Some(agent), Some(conversation), Some(request_id)) = (
        runtime.get("agent_id").and_then(serde_json::Value::as_str),
        runtime
            .get("conversation_id")
            .and_then(serde_json::Value::as_str),
        frame
            .value
            .get("request_id")
            .and_then(serde_json::Value::as_str),
    ) else {
        return false;
    };
    let key = lotta_tools::external::ChannelRuntimeKey {
        agent_id: agent.to_owned(),
        conversation_id: conversation.to_owned(),
    };
    manager.respond(
        principal.owner(),
        principal.generation(),
        &key,
        lotta_tools::external::ChannelToolResponse {
            request_id: request_id.to_owned(),
            tool_call_id: frame
                .value
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            result: frame.value.get("result").cloned(),
            error: frame
                .value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        },
    ) == lotta_tools::external::ResponseDisposition::Resolved
}

pub(super) fn publish_channel_runtime_tool(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &crate::ws::RuntimeCommand,
    principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> bool {
    let crate::ws::RuntimeCommand::RuntimeStart(start) = command else {
        return true;
    };
    let Some(manager) = &state.channel_tools else {
        return false;
    };
    let Some(principal) = principal else {
        return false;
    };
    let (Some(agent), Some(conversation)) = (&start.agent_id, &start.conversation_id) else {
        return false;
    };
    let runtime = lotta_tools::external::ChannelRuntimeKey {
        agent_id: agent.as_str().to_owned(),
        conversation_id: conversation.as_str().to_owned(),
    };
    let Some(tools) = &start.external_tools else {
        return false;
    };
    let Some(descriptors) = channel_tool_descriptors(tools) else {
        return false;
    };
    if manager
        .publish(
            principal.owner(),
            principal.generation(),
            runtime.clone(),
            descriptors,
        )
        .is_err()
    {
        return false;
    }
    if let Some(receiver) =
        manager.take_receiver(principal.owner(), principal.generation(), &runtime)
    {
        spawn_channel_tool_pump(
            Arc::clone(state),
            connection_id,
            Arc::clone(manager),
            principal.owner().to_owned(),
            principal.generation(),
            runtime,
            receiver,
        );
    }
    true
}

pub(super) fn channel_tool_descriptors(
    tools: &crate::ws::command::RuntimeStartExternalTools,
) -> Option<Vec<lotta_tools::external::ChannelToolDescriptor>> {
    tools
        .0
        .as_slice()
        .iter()
        .map(|tool| {
            let object = tool.as_value().as_object()?;
            Some(lotta_tools::external::ChannelToolDescriptor {
                name: object.get("name")?.as_str()?.to_owned(),
                description: object.get("description")?.as_str()?.to_owned(),
                parameters: object.get("parameters")?.clone(),
            })
        })
        .collect()
}

pub(super) fn spawn_channel_tool_pump(
    state: Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    manager: Arc<lotta_tools::external::ChannelExternalToolManager>,
    owner: String,
    generation: u64,
    runtime: lotta_tools::external::ChannelRuntimeKey,
    mut receiver: lotta_tools::external::ControllerReceiver,
) {
    tokio::spawn(async move {
        while let Some(request) = receiver.recv().await {
            if !manager.record_call(&owner, generation, &runtime, request.clone()) {
                break;
            }
            let frame = serde_json::json!({
                "type": "runtime_external_tool_call_request",
                "request_id": request.request_id.as_str(),
                "runtime": {
                    "agent_id": runtime.agent_id,
                    "conversation_id": runtime.conversation_id
                },
                "tool_call_id": request.tool_call_id.as_str(),
                "tool_name": request.model_name.as_str(),
                "input": request.arguments.as_value()
            });
            if dispatch_value(&state, connection_id, &frame).is_err() {
                break;
            }
        }
    });
}

pub(super) fn channel_runtime_command_allowed(
    state: &ListenerState,
    command: &crate::ws::RuntimeCommand,
    value: &serde_json::Value,
) -> bool {
    match command {
        crate::ws::RuntimeCommand::RuntimeStart(start) => state
            .channel_session
            .as_ref()
            .is_some_and(|authenticator| authenticator.runtime_start_frame_allowed(start, value)),
        crate::ws::RuntimeCommand::Input(input) => state
            .channel_session
            .as_ref()
            .is_some_and(|authenticator| authenticator.runtime_scope_allowed(&input.runtime)),
        _ => false,
    }
}

pub(super) fn event_sink(state: &Arc<ListenerState>) -> Arc<dyn crate::ws::RuntimeEventSink> {
    let observer_state = state.clone();
    Arc::new(RouterEventSink::new(
        state.runtime_router.clone(),
        Arc::new(move |scope, event, deliveries| {
            dispatch_runtime_event_batch(&observer_state, scope, event, &deliveries)
        }),
    ))
}

pub(super) fn dispatch_runtime_event_batch(
    state: &ListenerState,
    scope: lotta_domain::RuntimeScope,
    event: crate::ws::RuntimeEvent,
    deliveries: &EventDeliveryBatch,
) -> Result<(), AppServerError> {
    match dispatch_event_batch(state, scope, event, deliveries) {
        Err(AppServerError::Unavailable) => Ok(()),
        result => result,
    }
}

pub(super) async fn send_outbound_batch(
    socket: &mut WebSocket,
    batch: OutboundBatch,
) -> Result<(), axum::Error> {
    for body in batch {
        socket.send(Message::Text(body.into())).await?;
    }
    Ok(())
}

pub(super) fn dispatch_atomic_output(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    let mut frames = Vec::new();
    for batch in output.event_batches.as_slice() {
        for delivery in batch.deliveries.as_slice() {
            if delivery.connection_id == connection_id {
                frames.push(
                    serde_json::to_string(&delivery.frame).map_err(|_| AppServerError::Internal)?,
                );
            }
        }
    }
    for response in output.responses.as_slice() {
        frames.push(serde_json::to_string(response).map_err(|_| AppServerError::Internal)?);
    }
    let sender = state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?
        .get(&connection_id)
        .cloned()
        .ok_or(AppServerError::Unavailable)?;
    sender
        .try_send(frames)
        .map_err(|_| AppServerError::Unavailable)
}

pub(super) fn dispatch_output(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    if output.response_after_events {
        return dispatch_atomic_output(state, connection_id, output);
    }
    dispatch_responses(state, connection_id, output)?;
    dispatch_batches(state, output)
}

pub(super) fn dispatch_responses(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    for response in output.responses.as_slice() {
        dispatch_value(state, connection_id, response)?;
    }
    Ok(())
}

pub(super) fn dispatch_batches(
    state: &ListenerState,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    for batch in output.event_batches.as_slice() {
        dispatch_event_batch(
            state,
            batch.scope.clone(),
            batch.event.clone(),
            &batch.deliveries,
        )?;
    }
    Ok(())
}

pub(super) fn dispatch_value(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    value: &impl serde::Serialize,
) -> Result<(), AppServerError> {
    send_frame(&state.outbound, connection_id, value)
}

pub(super) fn send_frame(
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>,
    connection_id: crate::ws::ConnectionId,
    value: &impl serde::Serialize,
) -> Result<(), AppServerError> {
    let sender = outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?
        .get(&connection_id)
        .cloned()
        .ok_or(AppServerError::Unavailable)?;
    let body = serde_json::to_string(value).map_err(|_| AppServerError::Internal)?;
    sender
        .try_send(vec![body])
        .map_err(|_| AppServerError::Unavailable)
}

pub(super) fn external_forwarder(outbound: &SharedOutbound) -> ExternalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn teleport_forwarder(outbound: &SharedOutbound) -> TeleportForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn terminal_forwarder(outbound: &SharedOutbound) -> TerminalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn files_forwarder(outbound: &SharedOutbound) -> crate::ws::files::FilesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn memory_forwarder(outbound: &SharedOutbound) -> crate::ws::memory::MemoryForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn models_forwarder(outbound: &SharedOutbound) -> crate::ws::models::ModelsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn schedules_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::schedules::SchedulesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn skills_forwarder(outbound: &SharedOutbound) -> crate::ws::skills::SkillsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn settings_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::settings::SettingsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn device_forwarder(outbound: &SharedOutbound) -> crate::ws::device::DeviceForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn introspection_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::introspection::IntrospectionForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn agents_forwarder(outbound: &SharedOutbound) -> crate::ws::agents::AgentsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

pub(super) fn conversations_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::conversations::ConversationsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

/// Composes the conversation management bridge over the canonical storage root.
///
/// The optional authoritative lease authority comes from host-composed shared
/// bridges, so compaction commands acquire their command leases from the same
/// lifecycle registry the production turn path uses.
pub(super) fn compose_conversations_bridge(
    outbound: &SharedOutbound,
    storage_dir: &std::path::Path,
    clock: &Arc<dyn Clock + Send + Sync>,
    authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
) -> Result<Arc<ConversationsBridge>, AppServerError> {
    Ok(Arc::new(ConversationsBridge::new(
        conversations_forwarder(outbound),
        storage_dir,
        Arc::clone(clock),
        authority,
    )?))
}

/// Storage-root-backed command-group bridges composed once at startup.
pub(super) struct StorageBridges {
    pub(super) files: Arc<FilesBridge>,
    pub(super) memories: Arc<MemoryBridge>,
    pub(super) agents: Arc<AgentsBridge>,
    pub(super) conversations: Arc<ConversationsBridge>,
    pub(super) models: Arc<ModelsBridge>,
    pub(super) schedules: Arc<SchedulesBridge>,
}

/// Composes the storage-root-backed command-group bridges once at startup.
pub(super) fn compose_storage_bridges(
    outbound: &SharedOutbound,
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
    artifacts_dir: &std::path::Path,
    authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
) -> Result<StorageBridges, AppServerError> {
    let files = Arc::new(FilesBridge::new(
        files_forwarder(outbound),
        &prepared.workspace_dir,
        artifacts_dir,
    )?);
    let memories = Arc::new(MemoryBridge::new(
        memory_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let agents = Arc::new(AgentsBridge::new(
        agents_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let conversations =
        compose_conversations_bridge(outbound, &prepared.storage_dir, clock, authority)?;
    let models = Arc::new(ModelsBridge::new(
        models_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let schedules = Arc::new(SchedulesBridge::new(
        schedules_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    Ok(StorageBridges {
        files,
        memories,
        agents,
        conversations,
        models,
        schedules,
    })
}

/// Composes the device bridge over canonical roots with the host-registered
/// queue authority, and returns it alongside its introspection sibling.
///
/// # Errors
/// Returns a stable listener error when the workspace root is not absolute.
pub(super) fn compose_device_bridges(
    outbound: &SharedOutbound,
    prepared: &PreparedServer,
    queue_authority: Option<Arc<dyn crate::ws::device::QueueAuthority>>,
) -> Result<Arc<DeviceBridge>, AppServerError> {
    let devices = Arc::new(DeviceBridge::new(
        device_forwarder(outbound),
        &prepared.workspace_dir,
        &prepared.storage_dir,
    )?);
    if let Some(authority) = queue_authority {
        devices.register_queue_authority(authority);
    }
    Ok(devices)
}
