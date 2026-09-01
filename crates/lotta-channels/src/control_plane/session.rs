use super::{
    Arc, BTreeMap, BTreeSet, CHANNEL_STATE_ROWS_MAX, CONTROL_ARRAY_ITEMS_MAX,
    CONTROL_CORRELATIONS_MAX, CONTROL_ID_BYTES_MAX, CONTROL_JSON_DEPTH_MAX,
    CONTROL_MAP_ENTRIES_MAX, CONTROL_PROTOCOL_VERSION, CONTROL_REPLAY_IDS_MAX,
    CONTROL_STRING_BYTES_MAX, CONTROL_TIMEOUT_MS_MAX, ChannelState, ChildFrame,
    ControlCorrelationDirection, ControlError, FrameMetadata, ManagementCapability, ParentFrame,
    RESERVED_TOOL_NAMES, RUNTIME_TOOLS_PER_PUBLICATION_MAX, RuntimeKey, RuntimeTool, SystemTime,
    UNIX_EPOCH, Value, VecDeque,
};

#[derive(Clone, Debug)]
struct InFlightCorrelation {
    direction: ControlCorrelationDirection,
    owner: String,
    generation: u64,
    deadline_ms: u64,
}

/// Stateful owner/correlation validator and command dispatcher.
pub struct ControlPlane {
    owner: String,
    generation: u64,
    replay_order: VecDeque<String>,
    replay: BTreeSet<String>,
    in_flight: BTreeMap<String, InFlightCorrelation>,
    tools: Arc<lotta_tools::external::ChannelExternalToolManager>,
}

impl ControlPlane {
    /// Creates a plane bound to one exact supervised child identity.
    ///
    /// # Errors
    /// Returns when the owner identity is invalid.
    pub fn new(
        owner: String,
        generation: u64,
        tools: Arc<lotta_tools::external::ChannelExternalToolManager>,
    ) -> Result<Self, ControlError> {
        validate_id(&owner)?;
        if generation == 0 {
            return Err(ControlError::Owner);
        }
        Ok(Self {
            owner,
            generation,
            replay_order: VecDeque::new(),
            replay: BTreeSet::new(),
            in_flight: BTreeMap::new(),
            tools,
        })
    }

    /// Validates and applies one child management frame.
    ///
    /// # Errors
    /// Returns a structural, owner, correlation, registry, or bound failure.
    pub fn dispatch(
        &mut self,
        frame: ChildFrame,
        channels: &[ChannelState],
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.dispatch_at(frame, channels, current_millis()?)
    }

    /// Validates and applies one frame at an explicit monotonic test deadline instant.
    ///
    /// Admission happens only after complete structural, owner, capability, and
    /// operation validation. Parent terminals must match a live parent-owned
    /// correlation; child requests remain live until their exact response is written.
    ///
    /// # Errors
    /// Returns a structural, ownership, deadline, correlation, registry, or bound failure.
    pub fn dispatch_at(
        &mut self,
        frame: ChildFrame,
        channels: &[ChannelState],
        now_ms: u64,
    ) -> Result<Option<ParentFrame>, ControlError> {
        let (metadata, owner, request_id) = frame_identity(&frame);
        if metadata.version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::Version);
        }
        if owner != self.owner || metadata.generation != self.generation {
            return Err(ControlError::Owner);
        }
        if metadata.capability != ManagementCapability::ChannelManagement {
            return Err(ControlError::Capability);
        }
        if metadata.timeout_ms == 0 || metadata.timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        validate_frame(&frame, channels)?;
        let id = request_id.ok_or(ControlError::Correlation)?.to_owned();
        if matches!(
            frame,
            ChildFrame::Ready { .. } | ChildFrame::ShutdownComplete { .. }
        ) {
            self.complete_correlation(
                &id,
                ControlCorrelationDirection::ParentRequest,
                owner,
                metadata.generation,
                now_ms,
            )?;
            return self.apply(frame, channels);
        }
        self.admit_request(
            &id,
            ControlCorrelationDirection::ChildRequest,
            metadata.timeout_ms,
            now_ms,
        )?;
        let outcome = self.apply(frame, channels);
        if outcome.is_err() {
            self.in_flight.remove(&id);
        }
        outcome
    }

    fn apply(
        &self,
        frame: ChildFrame,
        channels: &[ChannelState],
    ) -> Result<Option<ParentFrame>, ControlError> {
        match frame {
            ChildFrame::PublishRuntimeTools {
                request_id,
                runtime,
                tools,
                ..
            } => self.publish(request_id, runtime, tools),
            ChildFrame::ReleaseRuntimeTools {
                request_id,
                runtime,
                ..
            } => self.release(request_id, runtime),
            ChildFrame::Channels { request_id, .. } => Ok(Some(ParentFrame::ChannelsResult {
                metadata: FrameMetadata::new(self.generation),
                owner: self.owner.clone(),
                correlation_id: request_id,
                channels: channels.to_vec(),
            })),
            ChildFrame::Ready { .. } | ChildFrame::ShutdownComplete { .. } => Ok(None),
        }
    }

    fn publish(
        &self,
        request_id: String,
        runtime: RuntimeKey,
        tools: Vec<RuntimeTool>,
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.tools
            .publish(
                &self.owner,
                self.generation,
                canonical_runtime(runtime),
                tools.into_iter().map(canonical_tool).collect(),
            )
            .map_err(|_| ControlError::Registry)?;
        Ok(Some(ParentFrame::RuntimeToolsPublished {
            metadata: FrameMetadata::new(self.generation),
            owner: self.owner.clone(),
            correlation_id: request_id,
        }))
    }

    fn release(
        &self,
        request_id: String,
        runtime: RuntimeKey,
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.tools
            .release(&self.owner, self.generation, &canonical_runtime(runtime))
            .map_err(|_| ControlError::Registry)?;
        Ok(Some(ParentFrame::RuntimeToolsReleased {
            metadata: FrameMetadata::new(self.generation),
            owner: self.owner.clone(),
            correlation_id: request_id,
        }))
    }

    /// Returns the exact owner bound to this management plane.
    #[must_use]
    pub(crate) fn owner(&self) -> String {
        self.owner.clone()
    }

    /// Returns the exact generation bound to this management plane.
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns whether the canonical production manager currently owns this runtime.
    #[must_use]
    pub fn contains_runtime(&self, runtime: &RuntimeKey) -> bool {
        self.tools.contains(&canonical_runtime(runtime.clone()))
    }

    /// Releases all registrations for this exact terminated generation.
    #[must_use]
    pub fn release_stale(&self) -> usize {
        self.tools.release_generation(&self.owner, self.generation)
    }

    /// Registers one exact parent request before it is written to the child.
    ///
    /// # Errors
    /// Rejects invalid, replayed, over-capacity, or invalid-timeout requests.
    pub fn register_parent_request(
        &mut self,
        request_id: &str,
        timeout_ms: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        if timeout_ms == 0 || timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        self.admit_request(
            request_id,
            ControlCorrelationDirection::ParentRequest,
            timeout_ms,
            now_ms,
        )
    }

    /// Completes the exact child request after its parent response is successfully written.
    ///
    /// # Errors
    /// Unknown, late, duplicated, wrong-owner, wrong-generation, and wrong-direction
    /// responses are terminal correlation failures.
    pub fn complete_parent_response_at(
        &mut self,
        frame: &ParentFrame,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        let (metadata, owner, correlation) = parent_response_identity(frame)?;
        if metadata.version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::Version);
        }
        if metadata.capability != ManagementCapability::ChannelManagement {
            return Err(ControlError::Capability);
        }
        if metadata.timeout_ms == 0 || metadata.timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        self.complete_correlation(
            correlation,
            ControlCorrelationDirection::ChildRequest,
            owner,
            metadata.generation,
            now_ms,
        )
    }

    /// Returns the current bounded live-correlation count.
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    fn admit_request(
        &mut self,
        request_id: &str,
        direction: ControlCorrelationDirection,
        timeout_ms: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        validate_id(request_id)?;
        if self.replay.contains(request_id) || self.in_flight.len() >= CONTROL_CORRELATIONS_MAX {
            return Err(ControlError::Correlation);
        }
        self.retain_replay(request_id);
        self.in_flight.insert(
            request_id.to_owned(),
            InFlightCorrelation {
                direction,
                owner: self.owner.clone(),
                generation: self.generation,
                deadline_ms: now_ms.saturating_add(timeout_ms),
            },
        );
        Ok(())
    }

    fn complete_correlation(
        &mut self,
        request_id: &str,
        direction: ControlCorrelationDirection,
        owner: &str,
        generation: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        validate_id(request_id)?;
        let pending = self
            .in_flight
            .remove(request_id)
            .ok_or(ControlError::Correlation)?;
        if pending.direction != direction
            || pending.owner != owner
            || pending.generation != generation
            || now_ms > pending.deadline_ms
        {
            return Err(if now_ms > pending.deadline_ms {
                ControlError::Timeout
            } else {
                ControlError::Correlation
            });
        }
        Ok(())
    }

    fn retain_replay(&mut self, request_id: &str) {
        if self.replay.len() >= CONTROL_REPLAY_IDS_MAX
            && let Some(oldest) = self.replay_order.pop_front()
        {
            self.replay.remove(&oldest);
        }
        self.replay.insert(request_id.to_owned());
        self.replay_order.push_back(request_id.to_owned());
    }
}

/// Reads exactly one bounded newline-delimited JSON object.
///
/// # Errors
/// Returns a byte, structure, UTF-8, JSON, discriminant, or pipe failure.
fn validate_structure(value: &Value) -> Result<(), ControlError> {
    let bounds = lotta_extensions::sidecar::framing::JsonStructureBounds {
        depth_max: CONTROL_JSON_DEPTH_MAX,
        string_bytes_max: CONTROL_STRING_BYTES_MAX,
        array_items_max: CONTROL_ARRAY_ITEMS_MAX,
        map_entries_max: CONTROL_MAP_ENTRIES_MAX,
    };
    lotta_extensions::sidecar::framing::validate_json_structure(value, bounds)
        .map_err(|_| ControlError::Bound)
}

fn validate_frame(frame: &ChildFrame, channels: &[ChannelState]) -> Result<(), ControlError> {
    match frame {
        ChildFrame::PublishRuntimeTools { runtime, tools, .. } => {
            validate_runtime(runtime)?;
            validate_tools(tools)
        }
        ChildFrame::ReleaseRuntimeTools { runtime, .. } => validate_runtime(runtime),
        ChildFrame::Channels { .. } => (channels.len() <= CHANNEL_STATE_ROWS_MAX)
            .then_some(())
            .ok_or(ControlError::Bound),
        ChildFrame::Ready { pid, .. } => (*pid != 0).then_some(()).ok_or(ControlError::Correlation),
        ChildFrame::ShutdownComplete { .. } => Ok(()),
    }
}

fn validate_tools(tools: &[RuntimeTool]) -> Result<(), ControlError> {
    if tools.is_empty() || tools.len() > RUNTIME_TOOLS_PER_PUBLICATION_MAX {
        return Err(ControlError::Registry);
    }
    let mut names = BTreeSet::new();
    for tool in tools {
        validate_id(&tool.name)?;
        if tool.description.is_empty()
            || tool.description.len() > CONTROL_STRING_BYTES_MAX
            || RESERVED_TOOL_NAMES.contains(&tool.name.as_str())
            || !names.insert(&tool.name)
        {
            return Err(ControlError::Registry);
        }
        validate_structure(&tool.parameters)?;
    }
    Ok(())
}

fn canonical_runtime(runtime: RuntimeKey) -> lotta_tools::external::ChannelRuntimeKey {
    lotta_tools::external::ChannelRuntimeKey {
        agent_id: runtime.agent_id,
        conversation_id: runtime.conversation_id,
    }
}

fn canonical_tool(tool: RuntimeTool) -> lotta_tools::external::ChannelToolDescriptor {
    lotta_tools::external::ChannelToolDescriptor {
        name: tool.name,
        description: tool.description,
        parameters: tool.parameters,
    }
}

fn validate_runtime(runtime: &RuntimeKey) -> Result<(), ControlError> {
    validate_id(&runtime.agent_id)?;
    validate_id(&runtime.conversation_id)
}

fn validate_id(value: &str) -> Result<(), ControlError> {
    if value.is_empty()
        || value.len() > CONTROL_ID_BYTES_MAX
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        Err(ControlError::Correlation)
    } else {
        Ok(())
    }
}

fn current_millis() -> Result<u64, ControlError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ControlError::Timeout)?
        .as_millis();
    u64::try_from(millis).map_err(|_| ControlError::Timeout)
}

fn parent_response_identity(
    frame: &ParentFrame,
) -> Result<(FrameMetadata, &str, &str), ControlError> {
    match frame {
        ParentFrame::ChannelsResult {
            metadata,
            owner,
            correlation_id,
            ..
        }
        | ParentFrame::RuntimeToolsPublished {
            metadata,
            owner,
            correlation_id,
        }
        | ParentFrame::RuntimeToolsReleased {
            metadata,
            owner,
            correlation_id,
        } => Ok((*metadata, owner, correlation_id)),
        ParentFrame::Bootstrap { .. } | ParentFrame::Shutdown { .. } => {
            Err(ControlError::Correlation)
        }
    }
}

fn frame_identity(frame: &ChildFrame) -> (FrameMetadata, &str, Option<&str>) {
    match frame {
        ChildFrame::Ready {
            metadata,
            owner,
            correlation_id,
            ..
        }
        | ChildFrame::ShutdownComplete {
            metadata,
            owner,
            correlation_id,
        } => (*metadata, owner, Some(correlation_id)),
        ChildFrame::PublishRuntimeTools {
            metadata,
            owner,
            request_id,
            ..
        }
        | ChildFrame::ReleaseRuntimeTools {
            metadata,
            owner,
            request_id,
            ..
        }
        | ChildFrame::Channels {
            metadata,
            owner,
            request_id,
        } => (*metadata, owner, Some(request_id)),
    }
}
