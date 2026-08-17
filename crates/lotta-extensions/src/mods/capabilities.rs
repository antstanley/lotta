use super::types::{
    Capability, ConversationHandle, Generation, ModError, ModOwner, ModRuntimeScope,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};
use tokio_util::sync::CancellationToken;

/// Future returned by capability runtime ports.
pub type CapabilityFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, ModError>> + Send + 'a>>;

/// Dependency-neutral runtime effects available to declared capabilities.
pub trait CapabilityPort: Send + Sync {
    /// Executes one already-authorized capability operation.
    fn call(
        &self,
        owner: &ModOwner,
        scope: &ModRuntimeScope,
        capability: Capability,
        operation: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> CapabilityFuture<'_>;
}

/// Immutable declared capabilities and one opaque runtime-scoped handle.
#[derive(Clone)]
pub struct CapabilityContext {
    declared: BTreeSet<Capability>,
    owner: ModOwner,
    scope: ModRuntimeScope,
    handle: ConversationHandle,
}
impl CapabilityContext {
    /// Creates a context bound to exactly one owner generation and runtime scope.
    #[must_use]
    pub fn new(
        declared: impl IntoIterator<Item = Capability>,
        owner: ModOwner,
        scope: ModRuntimeScope,
        handle: ConversationHandle,
    ) -> Self {
        Self {
            declared: declared.into_iter().collect(),
            owner,
            scope,
            handle,
        }
    }
    /// Borrows the opaque handle sent to the compatibility child.
    #[must_use]
    pub fn handle(&self) -> &ConversationHandle {
        &self.handle
    }
    /// Borrows declared capabilities only.
    #[must_use]
    pub fn declared(&self) -> &BTreeSet<Capability> {
        &self.declared
    }
    /// Borrows the exact owner.
    #[must_use]
    pub fn owner(&self) -> &ModOwner {
        &self.owner
    }
}

/// One complete capability call authorization request.
pub struct CapabilityCall {
    /// Exact caller owner.
    pub owner: ModOwner,
    /// Opaque scoped handle.
    pub handle: ConversationHandle,
    /// Declared capability.
    pub capability: Capability,
    /// Capability-specific operation name.
    pub operation: String,
    /// Operation parameters.
    pub params: Value,
    /// Explicit cancellation.
    pub cancellation: CancellationToken,
}

/// Authorizes capability operations before reaching the runtime port.
pub struct CapabilityBroker {
    context: CapabilityContext,
    port: Arc<dyn CapabilityPort>,
}
impl CapabilityBroker {
    /// Creates a broker over one immutable capability context.
    #[must_use]
    pub fn new(context: CapabilityContext, port: Arc<dyn CapabilityPort>) -> Self {
        Self { context, port }
    }
    /// Calls a declared capability within the exact owner, generation, handle, and scope.
    pub async fn call(&self, call: CapabilityCall) -> Result<Value, ModError> {
        if !self.context.declared.contains(&call.capability) {
            return Err(ModError::UndeclaredCapability);
        }
        if call.owner != self.context.owner || call.handle != self.context.handle {
            return Err(ModError::InvalidScope);
        }
        self.port
            .call(
                &call.owner,
                &self.context.scope,
                call.capability,
                &call.operation,
                call.params,
                call.cancellation,
            )
            .await
    }
    /// Refuses a stale generation before any runtime effect.
    pub fn validate_generation(&self, generation: Generation) -> Result<(), ModError> {
        if generation != self.context.owner.generation {
            return Err(ModError::InvalidScope);
        }
        Ok(())
    }
}

/// Multi-mod broker table that resolves opaque handles before any capability effect.
pub struct CapabilityBrokerTable {
    contexts: RwLock<BTreeMap<ConversationHandle, Arc<CapabilityBroker>>>,
}
impl CapabilityBrokerTable {
    /// Creates an empty broker table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            contexts: RwLock::new(BTreeMap::new()),
        }
    }
    /// Mints and stores one unforgeable opaque scope binding.
    pub fn mint(
        &self,
        declared: impl IntoIterator<Item = Capability>,
        owner: ModOwner,
        scope: ModRuntimeScope,
        port: Arc<dyn CapabilityPort>,
    ) -> Result<ConversationHandle, ModError> {
        let handle = ConversationHandle::mint()?;
        let context = CapabilityContext::new(declared, owner, scope, handle.clone());
        self.contexts
            .write()
            .map_err(|_| ModError::Unavailable)?
            .insert(
                handle.clone(),
                Arc::new(CapabilityBroker::new(context, port)),
            );
        Ok(handle)
    }
    /// Resolves and authorizes an inbound child request by handle and exact owner generation.
    pub async fn call(&self, call: CapabilityCall) -> Result<Value, ModError> {
        let broker = self
            .contexts
            .read()
            .map_err(|_| ModError::Unavailable)?
            .get(&call.handle)
            .cloned()
            .ok_or(ModError::InvalidScope)?;
        broker.call(call).await
    }
    /// Removes a handle when its generation is no longer active.
    pub fn revoke(&self, handle: &ConversationHandle) -> Result<(), ModError> {
        self.contexts
            .write()
            .map_err(|_| ModError::Unavailable)?
            .remove(handle);
        Ok(())
    }
    /// Number of live handles, which equals the number of currently accepted generations.
    pub fn len(&self) -> Result<usize, ModError> {
        self.contexts
            .read()
            .map(|contexts| contexts.len())
            .map_err(|_| ModError::Unavailable)
    }
    /// Whether no handle currently resolves to a runtime scope.
    pub fn is_empty(&self) -> Result<bool, ModError> {
        Ok(self.len()? == 0)
    }
}
impl Default for CapabilityBrokerTable {
    fn default() -> Self {
        Self::new()
    }
}
