use std::collections::HashMap;

use lotta_domain::bounds::{CONNECTIONS_MAX, RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX};
use std::{
    hash::Hash,
    time::{Duration, Instant},
};

/// Maximum suspended reconnect leases retained by one listener instance.
pub const SUSPENDED_CONNECTIONS_MAX: usize = 1_024;
/// Time after close during which an authenticated client can resume its lease.
pub const SUSPENDED_CONNECTION_TTL: Duration = Duration::from_secs(300);
use lotta_domain::{BoundedVec, NonEmptyString, RuntimeConnection, RuntimeScope};

use super::{envelope::StampedRuntimeEvent, event::RuntimeEvent};

/// Stable connection identifier assigned by the hub.
pub type ConnectionId = u64;

/// Collision-proof authenticated identity for resumable WebSocket state.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReconnectIdentity {
    /// Listener process instance that owns the lease.
    pub listener_instance: String,
    /// Stable verified authentication principal.
    pub principal: String,
    /// Client-generated identity distinguishing devices under one principal.
    pub client_id: String,
}

struct SuspendedConnection {
    connection: RuntimeConnection,
    suspended_at: Instant,
}

/// One ordered, connection-specific delivery.
#[derive(Clone, Debug)]
pub struct EventDelivery {
    /// Target connection identifier.
    pub connection_id: ConnectionId,
    /// Stable connection ordinal.
    pub ordinal: u64,
    /// Independently stamped frame.
    pub frame: StampedRuntimeEvent,
}

/// Owner-local connection registry and per-connection sequencing.
pub struct RuntimeConnections {
    entries: HashMap<ConnectionId, RuntimeConnection>,
    suspended: HashMap<ReconnectIdentity, SuspendedConnection>,
    next_id: ConnectionId,
    next_ordinal: u64,
}

fn connection_name(identity: &ReconnectIdentity) -> String {
    serde_json::to_string(&(
        &identity.listener_instance,
        &identity.principal,
        &identity.client_id,
    ))
    .unwrap_or_default()
}

fn reconnect_identity_from_name(value: &str) -> Option<ReconnectIdentity> {
    let (listener_instance, principal, client_id): (String, String, String) =
        serde_json::from_str(value).ok()?;
    Some(ReconnectIdentity {
        listener_instance,
        principal,
        client_id,
    })
}

impl Default for RuntimeConnections {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            suspended: HashMap::new(),
            next_id: 1,
            next_ordinal: 1,
        }
    }
}

impl RuntimeConnections {
    /// Opens an uninitialized stable connection.
    ///
    /// # Errors
    /// Returns a visible capacity, allocation, identifier, or counter error.
    pub fn open(&mut self) -> Result<ConnectionId, crate::error::AppServerError> {
        self.open_authenticated(None)
    }

    /// Opens or resumes one authenticated reconnect identity.
    ///
    /// A live identity cannot be replaced. Suspended state is consumed exactly
    /// once, so an authenticated reconnect preserves subscriptions and sequence.
    ///
    /// # Errors
    /// Returns a visible capacity, allocation, takeover, identifier, or counter error.
    pub fn open_authenticated(
        &mut self,
        identity: Option<&ReconnectIdentity>,
    ) -> Result<ConnectionId, crate::error::AppServerError> {
        self.validate_open(identity)?;
        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or(crate::error::AppServerError::Internal)?;
        let resumed = identity
            .and_then(|key| self.suspended.remove(key))
            .map(|lease| lease.connection);
        let connection = self.connection_for(id, identity, resumed)?;
        if connection.ordinal == self.next_ordinal {
            self.next_ordinal = self
                .next_ordinal
                .checked_add(1)
                .ok_or(crate::error::AppServerError::Internal)?;
        }
        self.entries.insert(id, connection);
        self.next_id = next_id;
        Ok(id)
    }

    fn validate_open(
        &mut self,
        identity: Option<&ReconnectIdentity>,
    ) -> Result<(), crate::error::AppServerError> {
        self.purge_expired();
        if self.entries.len() == CONNECTIONS_MAX.value {
            return Err(crate::error::AppServerError::Unavailable);
        }
        if identity.is_some_and(|key| {
            self.entries
                .values()
                .any(|entry| entry.id.as_str() == connection_name(key))
        }) {
            return Err(crate::error::AppServerError::Forbidden);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| crate::error::AppServerError::Unavailable)
    }

    fn connection_for(
        &self,
        id: ConnectionId,
        identity: Option<&ReconnectIdentity>,
        resumed: Option<RuntimeConnection>,
    ) -> Result<RuntimeConnection, crate::error::AppServerError> {
        if let Some(mut connection) = resumed {
            connection.initialized = false;
            return Ok(connection);
        }
        Ok(RuntimeConnection {
            id: NonEmptyString::new(
                identity.map_or_else(|| format!("connection-{id}"), connection_name),
            )
            .map_err(|_| crate::error::AppServerError::Internal)?,
            ordinal: self.next_ordinal,
            initialized: false,
            subscriptions: BoundedVec::new(Vec::new())
                .map_err(|_| crate::error::AppServerError::Internal)?,
            event_seq: 0,
        })
    }

    /// Marks protocol initialization complete.
    ///
    /// # Errors
    /// Returns an error when the connection is absent.
    pub fn initialize(&mut self, id: ConnectionId) -> Result<(), crate::error::AppServerError> {
        self.entry_mut(id)?.initialized = true;
        Ok(())
    }

    /// Removes a closed connection without retaining reconnect state.
    pub fn close(&mut self, id: ConnectionId) {
        self.entries.remove(&id);
    }

    /// Suspends an authenticated connection for a later same-identity reconnect.
    pub fn suspend(&mut self, id: ConnectionId) {
        let Some(connection) = self.entries.remove(&id) else {
            return;
        };
        self.purge_expired();
        if self.suspended.len() >= SUSPENDED_CONNECTIONS_MAX {
            self.evict_oldest_suspended();
        }
        let Some(identity) = reconnect_identity_from_name(connection.id.as_str()) else {
            return;
        };
        self.suspended.insert(
            identity,
            SuspendedConnection {
                connection,
                suspended_at: Instant::now(),
            },
        );
    }

    fn purge_expired(&mut self) {
        self.suspended
            .retain(|_, lease| lease.suspended_at.elapsed() < SUSPENDED_CONNECTION_TTL);
    }

    fn evict_oldest_suspended(&mut self) {
        let oldest = self
            .suspended
            .iter()
            .min_by_key(|(_, lease)| lease.suspended_at)
            .map(|(identity, _)| identity.clone());
        if let Some(identity) = oldest {
            self.suspended.remove(&identity);
        }
    }

    /// Adds one idempotent exact-scope subscription before mutation.
    ///
    /// # Errors
    /// Returns a visible capacity or allocation error without changing subscriptions.
    pub fn subscribe(
        &mut self,
        id: ConnectionId,
        scope: RuntimeScope,
    ) -> Result<(), crate::error::AppServerError> {
        let connection = self.entry_mut(id)?;
        if connection.subscriptions.as_slice().contains(&scope) {
            return Ok(());
        }
        if connection.subscriptions.len() == RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX.value {
            return Err(crate::error::AppServerError::PayloadTooLarge);
        }
        let next_len = connection
            .subscriptions
            .len()
            .checked_add(1)
            .ok_or(crate::error::AppServerError::PayloadTooLarge)?;
        let mut subscriptions = Vec::new();
        subscriptions
            .try_reserve_exact(next_len)
            .map_err(|_| crate::error::AppServerError::Unavailable)?;
        subscriptions.extend_from_slice(connection.subscriptions.as_slice());
        subscriptions.push(scope);
        connection.subscriptions = BoundedVec::new(subscriptions)
            .map_err(|_| crate::error::AppServerError::PayloadTooLarge)?;
        Ok(())
    }

    /// Returns the current distinct subscription count.
    #[must_use]
    pub fn subscription_count(&self, id: ConnectionId) -> Option<usize> {
        self.entries.get(&id).map(|entry| entry.subscriptions.len())
    }

    /// Returns the exact runtime scopes one connection subscribes to.
    #[must_use]
    pub fn subscriptions_of(&self, id: ConnectionId) -> Vec<RuntimeScope> {
        self.entries
            .get(&id)
            .map(|entry| entry.subscriptions.as_slice().to_vec())
            .unwrap_or_default()
    }

    /// Stamps one logical event for exactly one connection.
    ///
    /// # Errors
    /// Returns before UUID/time generation if its sequence would overflow.
    pub fn deliver_to(
        &mut self,
        id: ConnectionId,
        scope: &RuntimeScope,
        event: &RuntimeEvent,
        clock: &(dyn lotta_domain::Clock + Send + Sync),
        ids: &dyn super::envelope::EventIdGenerator,
    ) -> Result<EventDelivery, crate::error::AppServerError> {
        let connection = self.entry_mut(id)?;
        let sequence = connection
            .event_seq
            .checked_add(1)
            .ok_or(crate::error::AppServerError::Internal)?;
        let ordinal = connection.ordinal;
        let frame = super::envelope::stamp(event.clone(), scope.clone(), sequence, clock, ids)?;
        self.entry_mut(id)?.event_seq = sequence;
        Ok(EventDelivery {
            connection_id: id,
            ordinal,
            frame,
        })
    }

    /// Stamps one logical event independently for subscribers in ordinal order.
    ///
    /// # Errors
    /// Returns before UUID/time generation if any target sequence would overflow.
    pub fn broadcast(
        &mut self,
        scope: &RuntimeScope,
        event: &RuntimeEvent,
        clock: &(dyn lotta_domain::Clock + Send + Sync),
        ids: &dyn super::envelope::EventIdGenerator,
    ) -> Result<Vec<EventDelivery>, crate::error::AppServerError> {
        let mut targets = self.targets(scope)?;
        targets.sort_unstable_by_key(|(_, ordinal)| *ordinal);
        for (id, _) in &targets {
            self.entry_mut(*id)?
                .event_seq
                .checked_add(1)
                .ok_or(crate::error::AppServerError::Internal)?;
        }
        let mut deliveries = Vec::new();
        deliveries
            .try_reserve_exact(targets.len())
            .map_err(|_| crate::error::AppServerError::Unavailable)?;
        for (id, ordinal) in &targets {
            let sequence = self.entry_mut(*id)?.event_seq + 1;
            let frame = super::envelope::stamp(event.clone(), scope.clone(), sequence, clock, ids)?;
            deliveries.push(EventDelivery {
                connection_id: *id,
                ordinal: *ordinal,
                frame,
            });
        }
        for (id, _) in targets {
            self.entry_mut(id)?.event_seq += 1;
        }
        Ok(deliveries)
    }

    fn targets(
        &self,
        scope: &RuntimeScope,
    ) -> Result<Vec<(ConnectionId, u64)>, crate::error::AppServerError> {
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(self.entries.len())
            .map_err(|_| crate::error::AppServerError::Unavailable)?;
        targets.extend(self.entries.iter().filter_map(|(id, connection)| {
            (connection.initialized && connection.subscriptions.as_slice().contains(scope))
                .then_some((*id, connection.ordinal))
        }));
        Ok(targets)
    }

    fn entry_mut(
        &mut self,
        id: ConnectionId,
    ) -> Result<&mut RuntimeConnection, crate::error::AppServerError> {
        self.entries
            .get_mut(&id)
            .ok_or(crate::error::AppServerError::Internal)
    }

    #[cfg(test)]
    pub(crate) fn set_event_seq(&mut self, id: ConnectionId, value: u64) {
        if let Some(connection) = self.entries.get_mut(&id) {
            connection.event_seq = value;
        }
    }

    #[cfg(test)]
    pub(crate) fn event_seq(&self, id: ConnectionId) -> Option<u64> {
        self.entries.get(&id).map(|connection| connection.event_seq)
    }
}
