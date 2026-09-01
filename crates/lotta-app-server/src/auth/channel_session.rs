//! Dynamic, generation-bound authentication for the supervised channel host.

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq as _;
use tokio::sync::watch;

use crate::{auth::bearer_token, error::AppServerError};

/// Maximum lifetime of one child generation's listener capability.
pub const CHANNEL_SESSION_TTL_SECONDS: u64 = 300;

/// The only public Runtime commands admitted on the dedicated channel listener.
pub const CHANNEL_RUNTIME_COMMANDS: &[&str] = &[
    "runtime_start",
    "input",
    "runtime_external_tool_call_response",
];

#[derive(Clone)]
struct ListenerIdentity {
    host: String,
    path: String,
    instance: String,
}

struct Session {
    owner: String,
    generation: u64,
    pid: u32,
    digest: [u8; 32],
    deadline: Instant,
    listener: ListenerIdentity,
    sandbox_root: String,
    runtimes: Vec<(String, String)>,
    revoked: bool,
}

#[derive(Default)]
struct State {
    listener: Option<ListenerIdentity>,
    session: Option<Session>,
}

/// Shared authenticator whose authority is replaced atomically for every respawn.
#[derive(Clone)]
pub struct ChannelSessionAuthenticator {
    state: Arc<Mutex<State>>,
    revision: watch::Sender<u64>,
}

impl Default for ChannelSessionAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelSessionAuthenticator {
    /// Creates an empty authenticator. No upgrade is admitted before installation.
    #[must_use]
    pub fn new() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            state: Arc::new(Mutex::new(State::default())),
            revision,
        }
    }

    /// Binds this authority to the exact listener instance, host, and path once.
    ///
    /// # Errors
    /// Rejects rebinding to a different listener or a non-loopback host/path.
    pub fn bind_listener(
        &self,
        host: &str,
        path: &str,
        instance: &str,
    ) -> Result<(), AppServerError> {
        if !matches!(host, "127.0.0.1" | "::1" | "localhost")
            || !path.starts_with('/')
            || path.contains("//")
            || instance.is_empty()
        {
            return Err(AppServerError::Config("invalid channel listener identity"));
        }
        let candidate = ListenerIdentity {
            host: host.to_owned(),
            path: path.to_owned(),
            instance: instance.to_owned(),
        };
        let mut state = self.state.lock().map_err(|_| AppServerError::Internal)?;
        if let Some(current) = &state.listener
            && (current.host != candidate.host
                || current.path != candidate.path
                || current.instance != candidate.instance)
        {
            return Err(AppServerError::Config("channel listener already bound"));
        }
        state.listener = Some(candidate);
        Ok(())
    }

    /// Mints and atomically installs a fresh capability after the child PID exists.
    ///
    /// # Errors
    /// Fails closed before listener binding, for invalid identity/generation/PID,
    /// or entropy failure.
    pub fn install(
        &self,
        owner: &str,
        generation: u64,
        pid: u32,
        sandbox_root: &str,
    ) -> Result<ChannelSessionCapability, AppServerError> {
        self.install_scoped(owner, generation, pid, sandbox_root, &[])
    }

    /// Mints a capability bound to exact canonical routed Runtime scopes.
    ///
    /// # Errors
    /// Fails closed for invalid scope, listener, identity, or entropy.
    pub fn install_scoped(
        &self,
        owner: &str,
        generation: u64,
        pid: u32,
        sandbox_root: &str,
        runtimes: &[(String, String)],
    ) -> Result<ChannelSessionCapability, AppServerError> {
        if owner.is_empty()
            || owner.len() > 128
            || generation == 0
            || pid == 0
            || !sandbox_root.starts_with('/')
            || runtimes.len() > 256
            || runtimes
                .iter()
                .any(|(agent, conversation)| agent.is_empty() || conversation.is_empty())
        {
            return Err(AppServerError::Config("invalid channel session identity"));
        }
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|_| AppServerError::Config("channel capability entropy unavailable"))?;
        let token = hex(&secret);
        secret.fill(0);
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut state = self.state.lock().map_err(|_| AppServerError::Internal)?;
        let listener = state
            .listener
            .clone()
            .ok_or(AppServerError::Config("channel listener is not bound"))?;
        if let Some(old) = &mut state.session {
            old.revoked = true;
            old.digest.fill(0);
        }
        state.session = Some(Session {
            owner: owner.to_owned(),
            generation,
            pid,
            digest,
            deadline: Instant::now() + Duration::from_secs(CHANNEL_SESSION_TTL_SECONDS),
            listener,
            sandbox_root: sandbox_root.to_owned(),
            runtimes: runtimes.to_vec(),
            revoked: false,
        });
        self.bump();
        Ok(ChannelSessionCapability { token })
    }

    /// Authenticates an upgrade and returns its generation-specific principal.
    ///
    /// # Errors
    /// Returns one fixed unauthorized failure for absent, stale, expired, or wrong credentials.
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<ChannelPrincipal, AppServerError> {
        let token = bearer_token(headers)?;
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let state = self
            .state
            .lock()
            .map_err(|_| AppServerError::Unauthorized)?;
        let session = state.session.as_ref().ok_or(AppServerError::Unauthorized)?;
        if session.revoked
            || Instant::now() > session.deadline
            || candidate.ct_eq(&session.digest).unwrap_u8() != 1
        {
            return Err(AppServerError::Unauthorized);
        }
        Ok(ChannelPrincipal {
            owner: session.owner.clone(),
            generation: session.generation,
            pid: session.pid,
            listener_instance: session.listener.instance.clone(),
        })
    }

    /// Checks that a live principal still names the current non-expired generation.
    #[must_use]
    pub fn admits(&self, principal: &ChannelPrincipal) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.session.as_ref().is_some_and(|session| {
                !session.revoked
                    && Instant::now() <= session.deadline
                    && session.owner == principal.owner
                    && session.generation == principal.generation
                    && session.pid == principal.pid
                    && session.listener.instance == principal.listener_instance
            })
        })
    }

    /// Atomically revokes only the named generation and wakes every live socket.
    #[must_use]
    pub fn revoke(&self, owner: &str, generation: u64, pid: u32) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some(session) = &mut state.session else {
            return false;
        };
        if session.owner != owner || session.generation != generation || session.pid != pid {
            return false;
        }
        session.revoked = true;
        session.digest.fill(0);
        drop(state);
        self.bump();
        true
    }

    /// Subscribes to installation and revocation changes for live-socket closure.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// Returns whether a frame type belongs to the fixed channel Runtime command set.
    #[must_use]
    pub fn command_allowed(kind: &str) -> bool {
        CHANNEL_RUNTIME_COMMANDS.contains(&kind)
    }

    /// Checks one Runtime scope against the exact canonical routed scope set.
    #[must_use]
    pub fn runtime_scope_allowed(&self, runtime: &lotta_domain::RuntimeScope) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.session.as_ref().is_some_and(|session| {
                session.runtimes.iter().any(|(agent, conversation)| {
                    agent == runtime.agent_id.as_str()
                        && conversation == runtime.conversation_id.as_str()
                })
            })
        })
    }

    /// Validates the fixed sandbox, strict policy, and exact `MessageChannel` declaration.
    #[must_use]
    pub fn runtime_start_allowed(&self, command: &crate::ws::command::RuntimeStartCommand) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        let Some(session) = &state.session else {
            return false;
        };
        let Some(sandbox) = &command.workspace_sandbox else {
            return false;
        };
        let (Some(agent), Some(conversation)) = (&command.agent_id, &command.conversation_id)
        else {
            return false;
        };
        if sandbox.root.as_str() != session.sandbox_root
            || sandbox.isolation_root.as_str() != session.sandbox_root
            || command.cwd.as_deref() != Some(session.sandbox_root.as_str())
            || !matches!(command.mode, Some(crate::ws::command::RuntimeMode::Strict))
            || command.create_agent.is_some()
            || command.create_conversation.is_some()
            || command.conversation_source_tags.is_some()
            || command.skill_sources.is_some()
            || command.preserve_skill_sources.is_some()
            || command.force_device_status.is_some()
            || command.wait_for_replay.is_some()
            || !session
                .runtimes
                .iter()
                .any(|(allowed_agent, allowed_conversation)| {
                    allowed_agent == agent.as_str() && allowed_conversation == conversation.as_str()
                })
        {
            return false;
        }
        let Some(client) = &command.client_info else {
            return false;
        };
        if client.name.as_str() != "lotta-channel-host"
            || client.title.is_some()
            || client.version.is_some()
        {
            return false;
        }
        let Some(tools) = &command.external_tools else {
            return false;
        };
        let [tool] = tools.0.as_slice() else {
            return false;
        };
        tool.as_value() == &message_channel_descriptor()
    }

    /// Validates the complete dedicated-listener `runtime_start` object.
    #[must_use]
    pub fn runtime_start_frame_allowed(
        &self,
        command: &crate::ws::command::RuntimeStartCommand,
        value: &serde_json::Value,
    ) -> bool {
        const FIELDS: &[&str] = &[
            "type",
            "request_id",
            "agent_id",
            "conversation_id",
            "cwd",
            "mode",
            "workspace_sandbox",
            "external_tools",
            "client_info",
        ];
        let Some(object) = value.as_object() else {
            return false;
        };
        object.len() == FIELDS.len()
            && FIELDS.iter().all(|field| object.contains_key(*field))
            && self.runtime_start_allowed(command)
    }

    fn bump(&self) {
        let next = self.revision.borrow().wrapping_add(1);
        self.revision.send_replace(next);
    }
}

impl fmt::Debug for ChannelSessionAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelSessionAuthenticator([REDACTED])")
    }
}

/// Verified principal for one exact child process generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelPrincipal {
    owner: String,
    generation: u64,
    pid: u32,
    listener_instance: String,
}

impl ChannelPrincipal {
    /// Returns the exact child owner identity.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the exact child generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns a stable generation-specific reconnect principal.
    #[must_use]
    pub fn reconnect_principal(&self) -> String {
        format!("{}:{}:{}", self.owner, self.generation, self.pid)
    }
}

/// Plaintext bootstrap capability. Debug and Display never expose its contents.
pub struct ChannelSessionCapability {
    token: String,
}

impl ChannelSessionCapability {
    /// Borrows the secret only for inherited-stdin bootstrap.
    #[must_use]
    pub fn expose_for_pipe(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for ChannelSessionCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelSessionCapability([REDACTED])")
    }
}

fn message_channel_descriptor() -> serde_json::Value {
    serde_json::json!({
        "name": "MessageChannel",
        "description": "Send a visible reply through the originating channel",
        "parameters": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
            "additionalProperties": false
        }
    })
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    #[test]
    fn generation_replacement_and_exact_revocation_are_atomic() {
        let auth = ChannelSessionAuthenticator::new();
        auth.bind_listener("127.0.0.1", "/channel-runtime", "listener-1")
            .unwrap();
        let first = auth.install("owner-1", 1, 101, "/channels").unwrap();
        let first_headers = headers(first.expose_for_pipe());
        let first_principal = auth.authenticate(&first_headers).unwrap();
        assert!(auth.admits(&first_principal));

        let second = auth.install("owner-2", 2, 202, "/channels").unwrap();
        assert!(auth.authenticate(&first_headers).is_err());
        assert!(!auth.admits(&first_principal));
        let second_headers = headers(second.expose_for_pipe());
        let second_principal = auth.authenticate(&second_headers).unwrap();
        assert!(!auth.revoke("owner-1", 1, 101));
        assert!(auth.admits(&second_principal));
        assert!(auth.revoke("owner-2", 2, 202));
        assert!(auth.authenticate(&second_headers).is_err());
    }

    #[test]
    fn runtime_start_requires_exact_sandbox_policy_and_message_channel() {
        let auth = ChannelSessionAuthenticator::new();
        auth.bind_listener("127.0.0.1", "/channel-runtime", "listener-1")
            .unwrap();
        let _capability = auth
            .install_scoped(
                "owner",
                1,
                101,
                "/channels",
                &[("agent".into(), "conversation".into())],
            )
            .unwrap();
        let command: crate::ws::command::RuntimeStartCommand =
            serde_json::from_value(serde_json::json!({
                "request_id": "start",
                "agent_id": "agent",
                "conversation_id": "conversation",
                "cwd": "/channels",
                "mode": "strict",
                "workspace_sandbox": {
                    "root": "/channels",
                    "isolation_root": "/channels"
                },
                "external_tools": [message_channel_descriptor()],
                "client_info": {"name": "lotta-channel-host"}
            }))
            .unwrap();
        assert!(auth.runtime_start_allowed(&command));

        let arbitrary: crate::ws::command::RuntimeStartCommand =
            serde_json::from_value(serde_json::json!({
                "request_id": "bad",
                "agent_id": "agent",
                "conversation_id": "conversation",
                "mode": "strict",
                "workspace_sandbox": {
                    "root": "/channels",
                    "isolation_root": "/channels"
                },
                "external_tools": [{
                    "name": "Bash",
                    "description": "arbitrary",
                    "parameters": {"type": "object"}
                }]
            }))
            .unwrap();
        assert!(!auth.runtime_start_allowed(&arbitrary));
    }
}
