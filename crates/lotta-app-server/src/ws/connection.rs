use std::collections::HashMap;

use lotta_domain::bounds::{CONNECTIONS_MAX, RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX};
use lotta_domain::{BoundedVec, NonEmptyString, RuntimeConnection, RuntimeScope};

use super::{envelope::StampedRuntimeEvent, event::RuntimeEvent};

/// Stable connection identifier assigned by the hub.
pub type ConnectionId = u64;

/// One ordered, connection-specific delivery.
#[derive(Debug)]
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
    next_id: ConnectionId,
    next_ordinal: u64,
}

impl Default for RuntimeConnections {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
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
        if self.entries.len() == CONNECTIONS_MAX.value {
            return Err(crate::error::AppServerError::Unavailable);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| crate::error::AppServerError::Unavailable)?;
        let id = self.next_id;
        let ordinal = self.next_ordinal;
        let next_id = id
            .checked_add(1)
            .ok_or(crate::error::AppServerError::Internal)?;
        let next_ordinal = ordinal
            .checked_add(1)
            .ok_or(crate::error::AppServerError::Internal)?;
        let connection = RuntimeConnection {
            id: NonEmptyString::new(format!("connection-{id}"))
                .map_err(|_| crate::error::AppServerError::Internal)?,
            ordinal,
            initialized: false,
            subscriptions: BoundedVec::new(Vec::new())
                .map_err(|_| crate::error::AppServerError::Internal)?,
            event_seq: 0,
        };
        self.entries.insert(id, connection);
        self.next_id = next_id;
        self.next_ordinal = next_ordinal;
        Ok(id)
    }

    /// Marks protocol initialization complete.
    ///
    /// # Errors
    /// Returns an error when the connection is absent.
    pub fn initialize(&mut self, id: ConnectionId) -> Result<(), crate::error::AppServerError> {
        self.entry_mut(id)?.initialized = true;
        Ok(())
    }

    /// Removes a closed connection.
    pub fn close(&mut self, id: ConnectionId) {
        self.entries.remove(&id);
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
