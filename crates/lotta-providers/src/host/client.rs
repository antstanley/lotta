//! Isolated real-process client for the pinned provider host.

#![allow(clippy::missing_errors_doc)]

use super::catalog::HostCatalog;
use super::oauth::{
    OAuthBegin, OAuthCredential, OAuthDeviceBegin, OAuthDevicePoll, OAuthManager, OAuthMetadata,
};
use super::pin::{PI_AI_VERSION, validate_package_root};
use super::protocol::{
    HOST_EVENT_BYTES_MAX, HOST_PENDING_REQUESTS_MAX, HOST_PROTOCOL_VERSION, HOST_STREAM_EVENTS_MAX,
    HostHello, HostInferenceStart, HostOAuthCommand, HostRequest, HostResponse, HostStreamPayload,
};
use lotta_extensions::sidecar::framing::{read_frame, write_frame};
use lotta_extensions::sidecar::{
    SIDECAR_PROTOCOL_VERSION, SidecarCapability, SidecarEnvelope, SidecarEnvelopeKind,
    SidecarFrameLimit, SidecarOwnerIdentity,
};
use serde_json::{Value, json};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

const HOST_SCRIPT: &str = include_str!("../../assets/pi-ai-host.mjs");
const HOST_STDERR_BYTES_MAX: usize = 64 * 1024;
const HOST_REQUEST_TIMEOUT_MAX: Duration = Duration::from_mins(5);
const HOST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_NONCE_BYTES: usize = 32;
const HOST_CAPABILITIES: [&str; 5] = [
    "catalog",
    "provider_registration",
    "inference",
    "oauth",
    "lifecycle",
];

/// Explicit production launch configuration with no ambient runtime or package resolution.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Canonical regular Bun executable.
    pub bun_executable: PathBuf,
    /// Canonical regular host script path.
    pub host_script: PathBuf,
    /// Canonical explicit pi-ai package root.
    pub package_root: PathBuf,
    /// Empty dedicated child working directory.
    pub scratch_cwd: PathBuf,
    /// Enables the otherwise unavailable deterministic fixture injection capability.
    pub test_mode: bool,
}

/// Stable compatibility-host failure.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Launch configuration failed strict validation.
    #[error("invalid provider host configuration")]
    Configuration,
    /// Child launch or lifecycle failed.
    #[error("provider host unavailable")]
    Unavailable,
    /// Framing, handshake, correlation, or payload validation failed.
    #[error("provider host protocol rejected")]
    Protocol,
    /// Host rejected one command.
    #[error("provider host command rejected")]
    Rejected,
}

/// One owned, bounded provider-host process session.
pub struct HostClient {
    child: Child,
    reader: ChildStdout,
    writer: Option<ChildStdin>,
    stderr_task: Option<JoinHandle<Vec<u8>>>,
    owner: SidecarOwnerIdentity,
    next_id: u64,
    usable: bool,
    request_timeout: Duration,
    oauth: Option<std::sync::Arc<OAuthManager>>,
}

impl HostClient {
    /// Launches the real host, accepts Task 42 hello, and verifies pin, nonce, and capabilities.
    ///
    /// # Errors
    /// Returns a typed configuration, launch, or protocol failure.
    pub async fn spawn(config: HostConfig, owner: SidecarOwnerIdentity) -> Result<Self, HostError> {
        validate_config(&config)?;
        let nonce = nonce();
        let mut command = Command::new(&config.bun_executable);
        command
            .arg(&config.host_script)
            .arg(&config.package_root)
            .current_dir(&config.scratch_cwd)
            .env_clear()
            .env("NO_COLOR", "1")
            .env(
                "LOTTA_HOST_TEST_MODE",
                if config.test_mode { "1" } else { "0" },
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|_| HostError::Unavailable)?;
        let writer = child.stdin.take().ok_or(HostError::Unavailable)?;
        let reader = child.stdout.take().ok_or(HostError::Unavailable)?;
        let stderr = child.stderr.take().ok_or(HostError::Unavailable)?;
        let stderr_task = tokio::spawn(read_stderr(stderr));
        let mut client = Self {
            child,
            reader,
            writer: Some(writer),
            stderr_task: Some(stderr_task),
            owner,
            next_id: 1,
            usable: false,
            request_timeout: HOST_REQUEST_TIMEOUT_MAX,
            oauth: None,
        };
        let initialize = envelope(
            &client.owner,
            SidecarEnvelopeKind::Request,
            "initialize",
            json!({"command":"host.initialize","params":{"nonce":nonce}}),
        );
        write_frame(
            client.writer.as_mut().ok_or(HostError::Unavailable)?,
            SidecarFrameLimit::provider_host(),
            &initialize,
        )
        .await
        .map_err(|_| HostError::Unavailable)?;
        verify_hello(&mut client.reader, &client.owner, &nonce).await?;
        client.usable = true;
        Ok(client)
    }

    /// Sets the absolute remaining deadline used by every subsequent host operation.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout.min(HOST_REQUEST_TIMEOUT_MAX);
        self
    }

    /// Attaches the production OAuth manager to this exact host lifetime.
    #[must_use]
    pub fn with_oauth(mut self, oauth: std::sync::Arc<OAuthManager>) -> Self {
        self.oauth = Some(oauth);
        self
    }

    /// Returns exact validated provider OAuth metadata through the host protocol.
    pub async fn oauth_metadata(&mut self) -> Result<OAuthMetadata, HostError> {
        self.oauth_manager()?;
        self.oauth_public(HostOAuthCommand::Metadata, Value::Null)
            .await
    }

    /// Starts a browser PKCE flow without ambient browser access.
    pub async fn oauth_begin(
        &mut self,
        provider: &str,
        session: &str,
        redirect: &str,
        open_browser: bool,
    ) -> Result<OAuthBegin, HostError> {
        let manager = self.oauth_manager()?;
        let begin = manager
            .begin(provider, session, &self.owner_key(), redirect, open_browser)
            .map_err(|_| HostError::Rejected)?;
        let routed: OAuthBegin = self
            .oauth_public(
                HostOAuthCommand::Begin,
                json!({
                    "provider": provider,
                    "session": session,
                    "flow_id": begin.flow_id,
                    "authorization_url": begin.authorization_url,
                    "expires_at": begin.expires_at
                }),
            )
            .await?;
        if routed.flow_id != begin.flow_id
            || routed.authorization_url != begin.authorization_url
            || routed.expires_at != begin.expires_at
        {
            manager.host_died();
            return Err(HostError::Protocol);
        }
        Ok(begin)
    }

    /// Handles one duplicate-preserving callback query and delivers credentials once.
    pub async fn oauth_callback(
        &mut self,
        flow_id: &str,
        provider: &str,
        session: &str,
        origin: &str,
        redirect: &str,
        query: &[(&str, &str)],
    ) -> Result<OAuthCredential, HostError> {
        self.oauth_manager()?;
        let code = callback_field(query, "code")?;
        let state = callback_field(query, "state")?;
        let routed: Value = self
            .oauth_public(
                HostOAuthCommand::Exchange,
                json!({
                    "provider": provider,
                    "session": session,
                    "flow_id": flow_id,
                    "origin": origin,
                    "redirect_uri": redirect,
                    "code": code,
                    "state": state
                }),
            )
            .await?;
        if routed.get("accepted").and_then(Value::as_bool) != Some(true) {
            return Err(HostError::Protocol);
        }
        self.oauth_manager()?
            .callback(
                flow_id,
                provider,
                session,
                &self.owner_key(),
                super::oauth::OAuthCallback {
                    origin,
                    redirect_uri: redirect,
                    query,
                },
            )
            .map_err(|_| HostError::Rejected)
    }

    /// Starts a pinned `OpenAI` device-code flow.
    pub async fn oauth_device_begin(
        &mut self,
        provider: &str,
        session: &str,
    ) -> Result<OAuthDeviceBegin, HostError> {
        let manager = self.oauth_manager()?;
        let begin = manager
            .begin_device(provider, session, &self.owner_key())
            .map_err(|_| HostError::Rejected)?;
        let routed: OAuthDeviceBegin = self
            .oauth_public(
                HostOAuthCommand::DeviceBegin,
                json!({
                    "provider": provider,
                    "session": session,
                    "flow_id": begin.flow_id,
                    "user_code": begin.user_code,
                    "verification_uri": begin.verification_uri,
                    "interval_seconds": begin.interval_seconds,
                    "expires_at": begin.expires_at
                }),
            )
            .await?;
        if routed.flow_id != begin.flow_id
            || routed.user_code != begin.user_code
            || routed.verification_uri != begin.verification_uri
            || routed.interval_seconds != begin.interval_seconds
            || routed.expires_at != begin.expires_at
        {
            manager.host_died();
            return Err(HostError::Protocol);
        }
        Ok(begin)
    }

    /// Polls one device-code flow attempt.
    pub async fn oauth_device_poll(
        &mut self,
        flow_id: &str,
        provider: &str,
        session: &str,
    ) -> Result<OAuthDevicePoll, HostError> {
        self.oauth_manager()?;
        let routed: Value = self
            .oauth_public(
                HostOAuthCommand::DevicePoll,
                json!({"flow_id": flow_id, "provider": provider, "session": session}),
            )
            .await?;
        if routed.get("accepted").and_then(Value::as_bool) != Some(true) {
            return Err(HostError::Protocol);
        }
        self.oauth_manager()?
            .poll_device(flow_id, provider, session, &self.owner_key())
            .map_err(|_| HostError::Rejected)
    }

    /// Cancels one OAuth flow for this exact host owner.
    pub async fn oauth_cancel(
        &mut self,
        flow_id: &str,
        provider: &str,
        session: &str,
    ) -> Result<(), HostError> {
        self.oauth_manager()?;
        let routed: Value = self
            .oauth_public(
                HostOAuthCommand::Cancel,
                json!({"flow_id": flow_id, "provider": provider, "session": session}),
            )
            .await?;
        if routed.get("cancelled").and_then(Value::as_bool) != Some(true) {
            return Err(HostError::Protocol);
        }
        self.oauth_manager()?
            .cancel(flow_id, provider, session, &self.owner_key())
            .map_err(|_| HostError::Rejected)
    }

    fn oauth_manager(&self) -> Result<std::sync::Arc<OAuthManager>, HostError> {
        self.oauth.clone().ok_or(HostError::Configuration)
    }

    async fn oauth_public<T: serde::de::DeserializeOwned>(
        &mut self,
        command: HostOAuthCommand,
        params: Value,
    ) -> Result<T, HostError> {
        let value = self.request(command.name(), params).await?;
        serde_json::from_value(value).map_err(|_| HostError::Protocol)
    }

    fn owner_key(&self) -> String {
        serde_json::to_string(&self.owner).unwrap_or_default()
    }

    /// Requests and validates the complete credential-free catalog.
    ///
    /// # Errors
    /// Returns a typed transport, rejection, or catalog-validation failure.
    pub async fn catalog(&mut self) -> Result<HostCatalog, HostError> {
        let value = self.request("catalog.list", Value::Null).await?;
        let catalog: HostCatalog =
            serde_json::from_value(value).map_err(|_| HostError::Protocol)?;
        catalog.validate().map_err(|_| HostError::Protocol)?;
        Ok(catalog)
    }

    /// Atomically registers one mod-owned descriptor and returns the new revision.
    ///
    /// # Errors
    /// Returns a typed transport or ownership/conflict rejection.
    pub async fn register(&mut self, descriptor: Value) -> Result<u64, HostError> {
        revision(&self.request("provider.register", descriptor).await?)
    }

    /// Unregisters a descriptor only for its exact owner and returns the new revision.
    ///
    /// # Errors
    /// Returns a typed transport or ownership rejection.
    pub async fn unregister(&mut self, id: &str, owner: &str) -> Result<u64, HostError> {
        revision(
            &self
                .request("provider.unregister", json!({"id": id, "owner": owner}))
                .await?,
        )
    }

    /// Starts exactly one inference attempt and returns its correlated stream identifier.
    ///
    /// # Errors
    /// Returns a typed transport or payload rejection.
    pub async fn inference_start(
        &mut self,
        params: HostInferenceStart,
    ) -> Result<String, HostError> {
        let value = serde_json::to_value(params).map_err(|_| HostError::Protocol)?;
        let response = self.request("inference.start", value).await?;
        response["stream_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or(HostError::Protocol)
    }

    /// Reads one backpressured event for a correlated inference stream.
    ///
    /// # Errors
    /// Returns a typed transport, correlation, sequence, or event-bound failure.
    pub async fn inference_event(
        &mut self,
        stream_id: &str,
        sequence: u64,
    ) -> Result<HostStreamPayload, HostError> {
        let value = self
            .request(
                "inference.event",
                json!({"stream_id": stream_id, "sequence": sequence}),
            )
            .await?;
        if serde_json::to_vec(&value)
            .map_err(|_| HostError::Protocol)?
            .len()
            > HOST_EVENT_BYTES_MAX
        {
            return Err(HostError::Protocol);
        }
        let event: HostStreamPayload =
            serde_json::from_value(value).map_err(|_| HostError::Protocol)?;
        if event.stream_id != stream_id
            || event.sequence != sequence
            || sequence >= HOST_STREAM_EVENTS_MAX as u64
        {
            return Err(HostError::Protocol);
        }
        Ok(event)
    }

    /// Cancels one active correlated inference stream.
    ///
    /// # Errors
    /// Returns a typed transport or correlation rejection.
    pub async fn inference_cancel(&mut self, stream_id: &str) -> Result<(), HostError> {
        self.request("inference.cancel", json!({"stream_id": stream_id}))
            .await
            .map(|_| ())
    }

    /// Gracefully shuts down and reaps the exact child.
    ///
    /// # Errors
    /// Returns unavailable if the child cannot be reaped within the fixed deadline.
    pub async fn shutdown(mut self) -> Result<(), HostError> {
        let _ = self.request("shutdown", Value::Null).await;
        self.writer.take();
        tokio::time::timeout(HOST_CHILD_REAP_TIMEOUT, self.child.wait())
            .await
            .map_err(|_| HostError::Unavailable)?
            .map_err(|_| HostError::Unavailable)?;
        if let Some(task) = self.stderr_task.take() {
            let _ = task.await;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn isolation_snapshot(&mut self) -> Result<Value, HostError> {
        self.request("debug.isolation", Value::Null).await
    }

    async fn request(&mut self, command: &str, params: Value) -> Result<Value, HostError> {
        if !self.usable {
            return Err(HostError::Unavailable);
        }
        if self.next_id > HOST_PENDING_REQUESTS_MAX as u64 {
            self.next_id = 1;
        }
        let id = format!("request-{}", self.next_id);
        self.next_id += 1;
        let envelope = envelope(
            &self.owner,
            SidecarEnvelopeKind::Request,
            &id,
            serde_json::to_value(HostRequest {
                command: command.into(),
                params,
            })
            .map_err(|_| HostError::Protocol)?,
        );
        if write_frame(
            self.writer.as_mut().ok_or(HostError::Unavailable)?,
            SidecarFrameLimit::provider_host(),
            &envelope,
        )
        .await
        .is_err()
        {
            return self.terminate(HostError::Protocol).await;
        }
        let response: SidecarEnvelope = match tokio::time::timeout(
            self.request_timeout,
            read_frame(&mut self.reader, SidecarFrameLimit::provider_host()),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => return self.terminate(HostError::Protocol).await,
            Err(_) => return self.terminate(HostError::Unavailable).await,
        };
        if validate_response(&response, &self.owner, &id).is_err() {
            return self.terminate(HostError::Protocol).await;
        }
        let payload: HostResponse = match serde_json::from_value(response.payload) {
            Ok(value) => value,
            Err(_) => return self.terminate(HostError::Protocol).await,
        };
        if payload.error.is_some() {
            return Err(HostError::Rejected);
        }
        match payload.ok {
            Some(value) => Ok(value),
            None => self.terminate(HostError::Protocol).await,
        }
    }

    async fn terminate<T>(&mut self, error: HostError) -> Result<T, HostError> {
        self.usable = false;
        if let Some(oauth) = &self.oauth {
            oauth.host_died();
        }
        self.writer.take();
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(HOST_CHILD_REAP_TIMEOUT, self.child.wait()).await;
        Err(error)
    }
}

impl Drop for HostClient {
    fn drop(&mut self) {
        if let Some(oauth) = &self.oauth {
            oauth.host_died();
        }
        let _ = self.child.start_kill();
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn validate_config(config: &HostConfig) -> Result<(), HostError> {
    regular_canonical(&config.bun_executable)?;
    regular_canonical(&config.host_script)?;
    validate_package_root(&config.package_root).map_err(|_| HostError::Configuration)?;
    if !config.scratch_cwd.is_absolute()
        || config.scratch_cwd.canonicalize().ok().as_ref() != Some(&config.scratch_cwd)
        || !config.scratch_cwd.is_dir()
        || std::fs::read_dir(&config.scratch_cwd)
            .map_err(|_| HostError::Configuration)?
            .next()
            .is_some()
    {
        return Err(HostError::Configuration);
    }
    Ok(())
}

fn regular_canonical(path: &Path) -> Result<(), HostError> {
    if !path.is_absolute()
        || path
            .symlink_metadata()
            .map_err(|_| HostError::Configuration)?
            .file_type()
            .is_symlink()
        || !path.is_file()
        || path.canonicalize().ok().as_ref() != Some(&path.to_path_buf())
    {
        return Err(HostError::Configuration);
    }
    Ok(())
}

async fn verify_hello(
    reader: &mut ChildStdout,
    owner: &SidecarOwnerIdentity,
    nonce: &str,
) -> Result<(), HostError> {
    let envelope: SidecarEnvelope = tokio::time::timeout(
        HOST_HANDSHAKE_TIMEOUT,
        read_frame(reader, SidecarFrameLimit::provider_host()),
    )
    .await
    .map_err(|_| HostError::Unavailable)?
    .map_err(|_| HostError::Protocol)?;
    if envelope.kind != SidecarEnvelopeKind::Hello
        || envelope.owner != *owner
        || envelope.capability != SidecarCapability::Provider
        || envelope.version != SIDECAR_PROTOCOL_VERSION
    {
        return Err(HostError::Protocol);
    }
    let hello: HostHello =
        serde_json::from_value(envelope.payload).map_err(|_| HostError::Protocol)?;
    if hello.pi_ai_version != PI_AI_VERSION
        || hello.protocol_version != HOST_PROTOCOL_VERSION
        || hello.nonce != nonce
        || hello.capabilities != HOST_CAPABILITIES
    {
        return Err(HostError::Protocol);
    }
    Ok(())
}

fn validate_response(
    value: &SidecarEnvelope,
    owner: &SidecarOwnerIdentity,
    id: &str,
) -> Result<(), HostError> {
    if value.kind != SidecarEnvelopeKind::Response
        || value.owner != *owner
        || value.capability != SidecarCapability::Provider
        || value.version != SIDECAR_PROTOCOL_VERSION
        || value.request_id != id
    {
        return Err(HostError::Protocol);
    }
    Ok(())
}

fn envelope(
    owner: &SidecarOwnerIdentity,
    kind: SidecarEnvelopeKind,
    id: &str,
    payload: Value,
) -> SidecarEnvelope {
    SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: owner.clone(),
        capability: SidecarCapability::Provider,
        timeout_ms: u64::try_from(HOST_REQUEST_TIMEOUT_MAX.as_millis()).unwrap_or(u64::MAX),
        request_id: id.into(),
        correlation_id: None,
        kind,
        payload,
    }
}

fn nonce() -> String {
    use std::fmt::Write as _;
    let mut bytes = [0_u8; HOST_NONCE_BYTES];
    getrandom::fill(&mut bytes).expect("operating system CSPRNG must be available");
    bytes.iter().fold(
        String::with_capacity(HOST_NONCE_BYTES * 2),
        |mut value, byte| {
            write!(value, "{byte:02x}").expect("writing to a string cannot fail");
            value
        },
    )
}

fn callback_field<'a>(query: &'a [(&str, &'a str)], name: &str) -> Result<&'a str, HostError> {
    let mut found = None;
    for (key, value) in query {
        if *key == name && found.replace(*value).is_some() {
            return Err(HostError::Rejected);
        }
    }
    found.ok_or(HostError::Rejected)
}

fn revision(value: &Value) -> Result<u64, HostError> {
    value["revision"].as_u64().ok_or(HostError::Protocol)
}

async fn read_stderr(stderr: tokio::process::ChildStderr) -> Vec<u8> {
    let mut reader = BufReader::new(stderr);
    let mut retained = Vec::with_capacity(HOST_STDERR_BYTES_MAX);
    let mut buffer = [0_u8; 8 * 1024];
    while let Ok(read) = reader.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let remaining = HOST_STDERR_BYTES_MAX.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    retained
}

/// Materializes the embedded production host script at an explicit caller-owned path.
///
/// # Errors
/// Returns configuration if the path is non-absolute, differs from an existing pinned script,
/// or cannot be created without replacement.
pub fn materialize_host_script(path: &Path) -> Result<(), HostError> {
    use std::io::Write as _;
    if !path.is_absolute() {
        return Err(HostError::Configuration);
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() != HOST_SCRIPT.len() as u64 {
                return Err(HostError::Configuration);
            }
            return std::fs::read_to_string(path)
                .is_ok_and(|source| source == HOST_SCRIPT)
                .then_some(())
                .ok_or(HostError::Configuration);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(HostError::Configuration),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| HostError::Configuration)?;
    file.write_all(HOST_SCRIPT.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| HostError::Configuration)
}
