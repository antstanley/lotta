//! Canonical channel-owned external tools over the production Task 32 registry.

use super::{
    ConnectionId, ControllerConnection, ControllerReceiver, ExternalCallRequest,
    ExternalCallResponse, ExternalRegistrationError, ExternalRequestId, ExternalToolGroup,
    ExternalToolManager, ExternalToolMember, GroupRevision, RegistrySelection, ResponseDisposition,
    RuntimeId, ToolCallId,
};
use crate::{RegistrySnapshot, ToolRegistry};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Exact runtime key used by channel management publication.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChannelRuntimeKey {
    /// Agent identifier.
    pub agent_id: String,
    /// Conversation identifier.
    pub conversation_id: String,
}

/// One bounded channel-owned external tool descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelToolDescriptor {
    /// Model-facing tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema parameters.
    pub parameters: serde_json::Value,
}

struct Entry {
    owner: String,
    generation: u64,
    runtime_id: RuntimeId,
    manager: ExternalToolManager,
    connection: Arc<ControllerConnection>,
    revision: GroupRevision,
    receiver: Option<ControllerReceiver>,
    pending: BTreeMap<String, ExternalCallRequest>,
}

/// Correlated channel tool response received on the public Runtime WebSocket.
pub struct ChannelToolResponse {
    /// Server-minted request identity.
    pub request_id: String,
    /// Optional echoed tool-call identity.
    pub tool_call_id: Option<String>,
    /// Successful bounded result payload.
    pub result: Option<serde_json::Value>,
    /// Mutually exclusive scrubbed error.
    pub error: Option<String>,
}

/// Owner-and-generation facade over the exact registry consumed by production turn setup.
pub struct ChannelExternalToolManager {
    registry: Arc<ToolRegistry>,
    entries: Mutex<BTreeMap<ChannelRuntimeKey, Entry>>,
}

impl ChannelExternalToolManager {
    /// Binds channel publication to the canonical production registry instance.
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Atomically publishes one runtime's complete channel-owned tool set.
    ///
    /// # Errors
    /// Rejects stale ownership/generation and all canonical external-tool validation failures.
    pub fn publish(
        &self,
        owner: &str,
        generation: u64,
        runtime: ChannelRuntimeKey,
        tools: Vec<ChannelToolDescriptor>,
    ) -> Result<(), ExternalRegistrationError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        if let Some(entry) = entries.get_mut(&runtime) {
            if entry.owner != owner || entry.generation != generation {
                return Err(ExternalRegistrationError::OwnerDisconnected);
            }
            let groups = groups(tools);
            entry.revision = entry.manager.update(
                &entry.connection,
                &entry.runtime_id,
                entry.revision,
                &groups,
                self.selection()?,
            )?;
            return Ok(());
        }
        let runtime_id =
            RuntimeId::new(format!("{}:{}", runtime.agent_id, runtime.conversation_id))
                .map_err(|_| ExternalRegistrationError::RuntimeMismatch)?;
        let manager =
            ExternalToolManager::production(runtime_id.clone(), Arc::clone(&self.registry));
        let identity = ConnectionId::new(format!("channel:{owner}:{generation}"))
            .map_err(|_| ExternalRegistrationError::InvalidName)?;
        let (connection, receiver) = manager.connect(identity)?;
        let revision = manager.runtime_start(
            &connection,
            &runtime_id,
            GroupRevision::new(0),
            &groups(tools),
            self.selection()?,
        )?;
        entries.insert(
            runtime,
            Entry {
                owner: owner.to_owned(),
                generation,
                runtime_id,
                manager,
                connection,
                revision,
                receiver: Some(receiver),
                pending: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Takes the sole bounded invocation receiver for transport on the owning WebSocket.
    pub fn take_receiver(
        &self,
        owner: &str,
        generation: u64,
        runtime: &ChannelRuntimeKey,
    ) -> Option<ControllerReceiver> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get_mut(runtime)?;
        (entry.owner == owner && entry.generation == generation)
            .then(|| entry.receiver.take())
            .flatten()
    }

    /// Records one emitted call before it is forwarded to the owning WebSocket.
    pub fn record_call(
        &self,
        owner: &str,
        generation: u64,
        runtime: &ChannelRuntimeKey,
        request: ExternalCallRequest,
    ) -> bool {
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        let Some(entry) = entries.get_mut(runtime) else {
            return false;
        };
        if entry.owner != owner || entry.generation != generation || entry.pending.len() >= 256 {
            return false;
        }
        entry
            .pending
            .insert(request.request_id.as_str().to_owned(), request)
            .is_none()
    }

    /// Resolves one exact correlated call result from the owning WebSocket generation.
    pub fn respond(
        &self,
        owner: &str,
        generation: u64,
        runtime: &ChannelRuntimeKey,
        response: ChannelToolResponse,
    ) -> ResponseDisposition {
        let Ok(mut entries) = self.entries.lock() else {
            return ResponseDisposition::UnknownIgnored;
        };
        let Some(entry) = entries.get_mut(runtime) else {
            return ResponseDisposition::UnknownIgnored;
        };
        if entry.owner != owner || entry.generation != generation {
            return ResponseDisposition::OwnerMismatch;
        }
        let Some(request) = entry.pending.remove(&response.request_id) else {
            return ResponseDisposition::UnknownIgnored;
        };
        let Ok(request_id) = ExternalRequestId::new(response.request_id) else {
            return ResponseDisposition::InvalidResponse;
        };
        let tool_call_id = match response.tool_call_id.as_deref() {
            Some(value) => match ToolCallId::new(value.to_owned()) {
                Ok(value) => value,
                Err(_) => return ResponseDisposition::InvalidResponse,
            },
            None => request.tool_call_id.clone(),
        };
        entry
            .connection
            .respond(ExternalCallResponse {
                runtime_id: request.runtime_id,
                request_id,
                tool_call_id,
                internal_name: request.internal_name,
                model_name: request.model_name,
                scope_id: request.scope_id,
                result: response.result,
                error: response.error,
            })
            .unwrap_or(ResponseDisposition::InvalidResponse)
    }

    /// Releases one exact owner/generation/runtime without touching a replacement generation.
    ///
    /// # Errors
    /// Rejects stale release attempts and canonical registry failures.
    pub fn release(
        &self,
        owner: &str,
        generation: u64,
        runtime: &ChannelRuntimeKey,
    ) -> Result<bool, ExternalRegistrationError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        let Some(entry) = entries.get_mut(runtime) else {
            return Ok(false);
        };
        if entry.owner != owner || entry.generation != generation {
            return Err(ExternalRegistrationError::OwnerDisconnected);
        }
        entry.revision = entry.manager.update(
            &entry.connection,
            &entry.runtime_id,
            entry.revision,
            &[],
            self.selection()?,
        )?;
        entry.connection.close();
        entries.remove(runtime);
        Ok(true)
    }

    /// Releases all tools belonging to one exact terminated child generation.
    pub fn release_generation(&self, owner: &str, generation: u64) -> usize {
        let keys = self.entries.lock().map_or_else(
            |_| Vec::new(),
            |entries| {
                entries
                    .iter()
                    .filter(|(_, entry)| entry.owner == owner && entry.generation == generation)
                    .map(|(key, _)| key.clone())
                    .collect()
            },
        );
        keys.into_iter()
            .filter(|key| self.release(owner, generation, key).unwrap_or(false))
            .count()
    }

    /// Returns the exact runtime-scoped registrations consumed by production turn setup.
    #[must_use]
    pub fn runtime_registrations(
        &self,
        runtime: &ChannelRuntimeKey,
    ) -> Option<Vec<crate::ToolRegistration>> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(runtime)?;
        let snapshot = entry.manager.select(self.selection().ok()?).ok()?;
        Some(
            snapshot
                .complete_registrations()
                .1
                .into_iter()
                .filter(|registration| {
                    registration.definition.execution_owner
                        == lotta_runtime::ports::ToolExecutionOwner::Controller
                })
                .collect(),
        )
    }

    /// Returns the canonical production snapshot currently visible to turn setup.
    #[must_use]
    pub fn canonical_snapshot(&self) -> Option<Arc<RegistrySnapshot>> {
        self.registry.snapshot().ok()
    }

    /// Returns whether an exact runtime is currently owned by a live generation.
    #[must_use]
    pub fn contains(&self, runtime: &ChannelRuntimeKey) -> bool {
        self.entries
            .lock()
            .is_ok_and(|entries| entries.contains_key(runtime))
    }

    fn selection(&self) -> Result<RegistrySelection<'static>, ExternalRegistrationError> {
        let toolset = self
            .registry
            .snapshot()
            .map_err(ExternalRegistrationError::Registry)?
            .complete_registrations()
            .0;
        Ok(RegistrySelection {
            toolset,
            scope_id: None,
            allowlist: None,
        })
    }
}

fn groups(tools: Vec<ChannelToolDescriptor>) -> Vec<ExternalToolGroup> {
    if tools.is_empty() {
        return Vec::new();
    }
    vec![ExternalToolGroup {
        scope_id: None,
        tools: tools
            .into_iter()
            .map(|tool| ExternalToolMember {
                name: tool.name,
                label: None,
                description: tool.description,
                parameters: tool.parameters,
            })
            .collect(),
    }]
}
