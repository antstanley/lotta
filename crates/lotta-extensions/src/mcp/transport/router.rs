use super::jsonrpc::response_result;
use super::{MCP_MESSAGE_BYTES_MAX, MCP_PENDING_REQUESTS_MAX, McpError, Value};
use std::collections::BTreeMap;
use tokio::sync::oneshot;

pub(super) type ResponseSender = oneshot::Sender<Result<Value, McpError>>;
pub(super) type ResponseReceiver = oneshot::Receiver<Result<Value, McpError>>;
pub(super) type AckSender = oneshot::Sender<Result<(), McpError>>;

pub(super) enum RouterCommand {
    Register {
        id: u64,
        reply: ResponseSender,
        ack: AckSender,
    },
    Resolve {
        id: u64,
        value: Value,
        ack: AckSender,
    },
    Remove(u64),
}

pub(super) struct ResponseRouter {
    pending: BTreeMap<u64, ResponseSender>,
    ready: BTreeMap<u64, Value>,
    ready_bytes: usize,
}

impl ResponseRouter {
    pub(super) fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            ready: BTreeMap::new(),
            ready_bytes: 0,
        }
    }

    pub(super) fn handle(&mut self, command: RouterCommand) {
        match command {
            RouterCommand::Register { id, reply, ack } => self.register(id, reply, ack),
            RouterCommand::Resolve { id, value, ack } => self.resolve(id, value, ack),
            RouterCommand::Remove(id) => {
                self.pending.remove(&id);
                self.remove_ready(id);
            }
        }
    }

    pub(super) fn route(&mut self, value: Value) {
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return;
        };
        if let Some(reply) = self.pending.remove(&id) {
            let _ = reply.send(response_result(value, id));
        } else {
            let _ = self.store_ready(id, value);
        }
    }

    fn register(&mut self, id: u64, reply: ResponseSender, ack: AckSender) {
        if self.pending.contains_key(&id) {
            let _ = reply.send(Err(McpError::Protocol));
            let _ = ack.send(Err(McpError::Protocol));
            return;
        }
        if let Some(value) = self.remove_ready(id) {
            let _ = reply.send(response_result(value, id));
            let _ = ack.send(Ok(()));
            return;
        }
        if self.total() >= MCP_PENDING_REQUESTS_MAX {
            let _ = reply.send(Err(McpError::Limit));
            let _ = ack.send(Err(McpError::Limit));
            return;
        }
        self.pending.insert(id, reply);
        let _ = ack.send(Ok(()));
    }

    fn resolve(&mut self, id: u64, value: Value, ack: AckSender) {
        let result = if value.get("id").and_then(Value::as_u64) != Some(id) {
            Err(McpError::Protocol)
        } else if let Some(reply) = self.pending.remove(&id) {
            let _ = reply.send(response_result(value, id));
            Ok(())
        } else if self.ready.contains_key(&id) {
            Err(McpError::Protocol)
        } else {
            self.store_ready(id, value)
        };
        let _ = ack.send(result);
    }

    fn store_ready(&mut self, id: u64, value: Value) -> Result<(), McpError> {
        if self.total() >= MCP_PENDING_REQUESTS_MAX || self.ready.contains_key(&id) {
            return Err(if self.ready.contains_key(&id) {
                McpError::Protocol
            } else {
                McpError::Limit
            });
        }
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| McpError::Protocol)?
            .len();
        if self.ready_bytes.checked_add(bytes).ok_or(McpError::Limit)? > MCP_MESSAGE_BYTES_MAX {
            return Err(McpError::Limit);
        }
        self.ready_bytes += bytes;
        self.ready.insert(id, value);
        Ok(())
    }

    fn remove_ready(&mut self, id: u64) -> Option<Value> {
        let value = self.ready.remove(&id)?;
        self.ready_bytes = self
            .ready_bytes
            .saturating_sub(serde_json::to_vec(&value).map_or(0, |bytes| bytes.len()));
        Some(value)
    }

    fn total(&self) -> usize {
        self.pending.len() + self.ready.len()
    }

    pub(super) fn drain(self, error: McpError) {
        for (_, reply) in self.pending {
            let _ = reply.send(Err(error));
        }
    }
}
