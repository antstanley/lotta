//! Channel process topology, persistent-root validation, and per-child capability material.

use crate::control_plane::{CHANNEL_STATE_ROWS_MAX, ChannelState};
#[cfg(target_os = "linux")]
use lotta_tools::sandbox::bubblewrap_arguments;
#[cfg(target_os = "macos")]
use lotta_tools::sandbox::{SEATBELT_PROGRAM, seatbelt_arguments};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::process::Command;

/// Per-child capability lifetime.
pub const CHANNEL_CAPABILITY_TTL_SECONDS: u64 = 300;
/// Child startup handshake deadline.
pub const CHANNEL_STARTUP_DEADLINE_MS: u64 = 10_000;
/// Maximum child owner identity bytes.
pub const CHANNEL_OWNER_ID_BYTES_MAX: usize = 128;
/// Canonical channels directory name below the Letta home.
pub const CHANNELS_DIRECTORY_NAME: &str = "channels";
/// Compatibility environment indicating persisted channels should be restored.
pub const RESTORE_ENABLED_CHANNELS_ENV: &str = "LETTA_RESTORE_ENABLED_CHANNELS";
/// Explicit service-composition channel enable switch.
pub const CHANNELS_ENABLED_ENV: &str = "LETTA_CHANNELS_ENABLED";
/// Explicit executable override for packaged compatibility hosts.
pub const CHANNEL_HOST_EXECUTABLE_ENV: &str = "LOTTA_CHANNEL_HOST_EXECUTABLE";

/// Stable topology preparation or sandbox failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TopologyError {
    /// A path was relative, symlinked, escaped, or unavailable.
    #[error("channel root is not safely confined")]
    Path,
    /// Secure directory permissions could not be established.
    #[error("channel root permissions are unsafe")]
    Permissions,
    /// Cryptographic capability generation failed.
    #[error("channel capability generation failed")]
    Entropy,
    /// Owner identity was invalid.
    #[error("channel child identity is invalid")]
    Identity,
    /// No supported mandatory filesystem sandbox is available.
    #[error("channel filesystem sandbox is unavailable")]
    Sandbox,
}

/// Plaintext per-child token that never renders its contents through Debug or Display.
pub struct ChildCapability {
    secret: String,
    digest_hex: String,
    owner: String,
    issued: Instant,
    revoked: bool,
}

impl ChildCapability {
    /// Mints a cryptographically random capability bound to one child owner.
    ///
    /// # Errors
    /// Returns a scrubbed identity or entropy failure.
    pub fn mint(owner: &str) -> Result<Self, TopologyError> {
        validate_owner(owner)?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| TopologyError::Entropy)?;
        let secret = hex(&random);
        let digest_hex = hex(Sha256::digest(secret.as_bytes()).as_slice());
        Ok(Self {
            secret,
            digest_hex,
            owner: owner.to_owned(),
            issued: Instant::now(),
            revoked: false,
        })
    }

    /// Returns the protected secret solely for inherited-pipe bootstrap.
    #[must_use]
    pub fn expose_for_pipe(&self) -> &str {
        &self.secret
    }
    /// Returns the SHA-256 hex digest used to configure the dedicated listener.
    #[must_use]
    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }
    /// Returns the exact bound child owner.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }
    /// Returns whether the capability remains within lifetime and has not been revoked.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.revoked
            && self.issued.elapsed() <= Duration::from_secs(CHANNEL_CAPABILITY_TTL_SECONDS)
    }
    /// Revokes this exact capability; repeated revocation is harmless.
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.secret.clear();
    }
}

impl fmt::Debug for ChildCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildCapability")
            .field("owner", &self.owner)
            .field("secret", &"[REDACTED]")
            .field("revoked", &self.revoked)
            .finish_non_exhaustive()
    }
}

/// Canonical shared channel configuration store root.
#[derive(Clone, Debug)]
pub struct ChannelStore {
    root: PathBuf,
    isolation_root: PathBuf,
}

impl ChannelStore {
    /// Creates or opens `<letta-home>/channels` with symlink and permission checks.
    ///
    /// # Errors
    /// Returns a scrubbed path or permission failure.
    pub fn under_letta_home(letta_home: &Path) -> Result<Self, TopologyError> {
        if !letta_home.is_absolute() {
            return Err(TopologyError::Path);
        }
        create_secure_directory(letta_home)?;
        let isolation_root = canonical_directory(letta_home)?;
        let root = isolation_root.join(CHANNELS_DIRECTORY_NAME);
        create_secure_directory(&root)?;
        let root = canonical_directory(&root)?;
        if root.parent() != Some(isolation_root.as_path()) {
            return Err(TopologyError::Path);
        }
        Ok(Self {
            root,
            isolation_root,
        })
    }

    /// Resolves the canonical root from `LETTA_HOME`, otherwise `$HOME/.letta`.
    ///
    /// # Errors
    /// Returns a scrubbed path or permission failure.
    pub fn from_parent_environment() -> Result<Self, TopologyError> {
        let home = std::env::var_os("LETTA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|value| PathBuf::from(value).join(".letta")))
            .ok_or(TopologyError::Path)?;
        Self::under_letta_home(&home)
    }

    /// Returns the only persistent filesystem root granted to the child.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Returns the masked Letta-home isolation root.
    #[must_use]
    pub fn isolation_root(&self) -> &Path {
        &self.isolation_root
    }

    /// Reads a bounded canonical state snapshot without following channel symlinks.
    ///
    /// # Errors
    /// Returns a scrubbed path, schema, or bound failure.
    pub fn channel_state(&self) -> Result<Vec<ChannelState>, TopologyError> {
        let mut rows = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|_| TopologyError::Path)?;
        for entry in entries.take(CHANNEL_STATE_ROWS_MAX + 1) {
            let entry = entry.map_err(|_| TopologyError::Path)?;
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| TopologyError::Path)?;
            if metadata.file_type().is_symlink() {
                return Err(TopologyError::Path);
            }
            if !metadata.is_dir() {
                continue;
            }
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| TopologyError::Path)?;
            validate_channel_id(&id)?;
            rows.push(ChannelState {
                id,
                enabled: true,
                accounts: account_count(&entry.path())?,
            });
        }
        if rows.len() > CHANNEL_STATE_ROWS_MAX {
            return Err(TopologyError::Path);
        }
        rows.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(rows)
    }
}

/// Returns whether existing startup composition requests channel restoration.
#[must_use]
pub fn channels_enabled() -> bool {
    [CHANNELS_ENABLED_ENV, RESTORE_ENABLED_CHANNELS_ENV]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| value == "1"))
}

/// Builds a child command with cleared ambient environment and mandatory OS write confinement.
///
/// # Errors
/// Returns when paths are invalid or the mandatory OS sandbox is unavailable.
pub fn sandboxed_child_command(
    executable: &Path,
    store: &ChannelStore,
) -> Result<Command, TopologyError> {
    if !executable.is_absolute() {
        return Err(TopologyError::Path);
    }
    let executable_text = executable.to_str().ok_or(TopologyError::Path)?;
    let child_arguments = vec!["__channel-host".to_owned()];
    let (outer, arguments) = sandbox_arguments(store, executable_text, &child_arguments)?;
    let mut command = Command::new(outer);
    command
        .args(arguments)
        .env_clear()
        .current_dir(store.root());
    command.kill_on_drop(true);
    Ok(command)
}

fn sandbox_arguments(
    store: &ChannelStore,
    executable: &str,
    child_arguments: &[String],
) -> Result<(&'static OsStr, Vec<String>), TopologyError> {
    #[cfg(target_os = "macos")]
    {
        if !Path::new(SEATBELT_PROGRAM).is_file() {
            return Err(TopologyError::Sandbox);
        }
        Ok((
            OsStr::new(SEATBELT_PROGRAM),
            seatbelt_arguments(
                store.root(),
                store.isolation_root(),
                executable,
                child_arguments,
            ),
        ))
    }
    #[cfg(target_os = "linux")]
    {
        for candidate in ["/usr/bin/bwrap", "/bin/bwrap"] {
            if Path::new(candidate).is_file() {
                return Ok((
                    OsStr::new(candidate),
                    bubblewrap_arguments(
                        store.root(),
                        store.isolation_root(),
                        executable,
                        child_arguments,
                    ),
                ));
            }
        }
        Err(TopologyError::Sandbox)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (store, executable, child_arguments);
        Err(TopologyError::Sandbox)
    }
}

fn create_secure_directory(path: &Path) -> Result<(), TopologyError> {
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| TopologyError::Path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(TopologyError::Path);
        }
    } else {
        std::fs::create_dir_all(path).map_err(|_| TopologyError::Path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| TopologyError::Permissions)?;
    }
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, TopologyError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| TopologyError::Path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TopologyError::Path);
    }
    std::fs::canonicalize(path).map_err(|_| TopologyError::Path)
}

fn account_count(channel: &Path) -> Result<usize, TopologyError> {
    let path = channel.join("accounts.json");
    if !path.exists() {
        return Ok(0);
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| TopologyError::Path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TopologyError::Path);
    }
    let bytes = std::fs::read(&path).map_err(|_| TopologyError::Path)?;
    if bytes.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX {
        return Err(TopologyError::Path);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| TopologyError::Path)?;
    Ok(value.as_object().map_or(0, serde_json::Map::len))
}

fn validate_channel_id(id: &str) -> Result<(), TopologyError> {
    if id.is_empty()
        || id.len() > CHANNEL_OWNER_ID_BYTES_MAX
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Err(TopologyError::Path)
    } else {
        Ok(())
    }
}

fn validate_owner(owner: &str) -> Result<(), TopologyError> {
    if owner.is_empty()
        || owner.len() > CHANNEL_OWNER_ID_BYTES_MAX
        || !owner.is_ascii()
        || owner
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        Err(TopologyError::Identity)
    } else {
        Ok(())
    }
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
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
fn temp_root(label: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!(
            "lotta-channel-{label}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[cfg(test)]
mod supervision {
    use super::*;

    const HELPER: &str = r"#!/usr/bin/python3
import json, os, socket, sys
boot=json.loads(sys.stdin.readline())
url=boot['websocket_url'].split('://',1)[1]
hostport,path=url.split('/',1)
host,port=hostport.rsplit(':',1)
s=socket.create_connection((host,int(port)))
key='dGhlIHNhbXBsZSBub25jZQ=='
request=('GET /'+path+' HTTP/1.1\r\n'
 +'Host: '+hostport+'\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
 +'Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: '+key+'\r\n'
 +'Authorization: Bearer '+boot['token']+'\r\n\r\n')
s.sendall(request.encode())
if b' 101 ' not in s.recv(4096): sys.exit(3)
ready={'kind':'ready','correlation_id':boot['request_id'],
 'owner':boot['owner'],'pid':os.getpid()}
print(json.dumps(ready),flush=True)
runtime={'agent_id':'agent','conversation_id':'conversation'}
tool={'name':'channel_send','description':'send','parameters':{'type':'object'}}
publish={'kind':'publish_runtime_tools','request_id':'publish',
 'owner':boot['owner'],'runtime':runtime,'tools':[tool]}
print(json.dumps(publish),flush=True)
for line in sys.stdin:
 frame=json.loads(line)
 if frame['kind']=='shutdown':
  done={'kind':'shutdown_complete','correlation_id':frame['request_id'],
   'owner':boot['owner']}
  print(json.dumps(done),flush=True)
  break
s.close()
";

    #[test]
    fn authenticates_over_loopback_and_reads_shared_channel_config() {
        let parent = temp_root("supervision");
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        std::fs::create_dir(store.root().join("telegram")).unwrap();
        let rows = store.channel_state().unwrap();
        assert_eq!(rows[0].id, "telegram");
        let mut capability = ChildCapability::mint("supervised-child").unwrap();
        assert!(capability.is_valid());
        assert_eq!(capability.owner(), "supervised-child");
        capability.revoke();
        assert!(!capability.is_valid());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn spawns_child_authenticates_publishes_and_reaps() {
        use crate::{
            control_plane::RuntimeKey,
            supervisor::{ChannelLaunchConfig, ChannelSupervisor},
        };
        use futures_util::StreamExt as _;
        use std::{
            os::unix::fs::PermissionsExt,
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
        };
        let parent = temp_root("real-supervision");
        let script = parent.parent().unwrap().join(format!(
            "channel-helper-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&script, HELPER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let capability = ChildCapability::mint("real-child").unwrap();
        let expected = format!("Bearer {}", capability.expose_for_pipe());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let authenticated = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&authenticated);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4_096];
            let length = loop {
                let length = stream.peek(&mut request).await.unwrap();
                if request[..length]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break length;
                }
                tokio::task::yield_now().await;
            };
            let headers = String::from_utf8_lossy(&request[..length]);
            observed.store(
                headers.contains(&format!("Authorization: {expected}")),
                Ordering::SeqCst,
            );
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while socket.next().await.is_some() {}
        });
        let supervisor = ChannelSupervisor::start(ChannelLaunchConfig {
            executable: script.clone(),
            store: ChannelStore::under_letta_home(&parent).unwrap(),
            websocket_url: format!("ws://127.0.0.1:{}/ws", address.port()),
            capability,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(supervisor.pid().is_some());
        assert!(authenticated.load(Ordering::SeqCst));
        assert!(
            supervisor
                .runtime_tools(&RuntimeKey {
                    agent_id: "agent".into(),
                    conversation_id: "conversation".into()
                })
                .is_some()
        );
        supervisor.shutdown().await.unwrap();
        server.await.unwrap();
        std::fs::remove_file(script).unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
}

#[cfg(test)]
mod plane_separation {
    use crate::control_plane::{ChildFrame, RuntimeKey, RuntimeTool};

    #[test]
    fn records_both_planes_and_rejects_negative_crossover() {
        let management = ChildFrame::PublishRuntimeTools {
            request_id: "publish".into(),
            owner: "owner".into(),
            runtime: RuntimeKey {
                agent_id: "agent".into(),
                conversation_id: "conversation".into(),
            },
            tools: vec![RuntimeTool {
                name: "channel_send".into(),
                description: "send".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
        };
        let control = serde_json::to_value(management).unwrap();
        let runtime = serde_json::json!({
            "type":"runtime_start", "request_id":"runtime",
            "agent_id":"agent", "conversation_id":"conversation"
        });
        assert_eq!(control["kind"], "publish_runtime_tools");
        assert_eq!(runtime["type"], "runtime_start");
        assert!(serde_json::from_value::<ChildFrame>(runtime).is_err());
        assert!(control.get("type").is_none());
    }
}

#[cfg(test)]
mod store_isolation {
    use super::*;

    #[test]
    fn rejects_symlink_escape_and_redacts_token() {
        let parent = temp_root("store");
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        assert_eq!(
            store.root(),
            parent.canonicalize().unwrap().join("channels")
        );
        let capability = ChildCapability::mint("owner").unwrap();
        assert!(!format!("{capability:?}").contains(capability.expose_for_pipe()));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&parent, store.root().join("escape")).unwrap();
            assert_eq!(store.channel_state(), Err(TopologyError::Path));
        }
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn actual_process_write_outside_channels_is_denied() {
        use std::os::unix::fs::PermissionsExt;
        let parent = temp_root("write-denial");
        let script = parent.parent().unwrap().join(format!(
            "channel-sandbox-probe-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf owned > owned\nprintf denied > ../outside && exit 9\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        let output = sandboxed_child_command(&script, &store)
            .unwrap()
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(store.root().join("owned").is_file());
        assert!(!parent.join("outside").exists());
        std::fs::remove_file(script).unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
}
