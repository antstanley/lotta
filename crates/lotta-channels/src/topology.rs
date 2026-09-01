//! Channel process topology, persistent-root validation, and per-child capability material.

use crate::control_plane::{CHANNEL_STATE_ROWS_MAX, ChannelState};
#[cfg(target_os = "macos")]
use lotta_tools::sandbox::SEATBELT_PROGRAM;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};
use tokio::process::Command;

/// Child startup handshake deadline.
pub const CHANNEL_STARTUP_DEADLINE_MS: u64 = 10_000;
/// Maximum child owner identity bytes.
pub const CHANNEL_OWNER_ID_BYTES_MAX: usize = 128;
/// Canonical channels directory name below the Letta home.
pub const CHANNELS_DIRECTORY_NAME: &str = "channels";
/// Compatibility environment indicating persisted channels should be restored.
pub const RESTORE_ENABLED_CHANNELS_ENV: &str = "LETTA_RESTORE_ENABLED_CHANNELS";
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
    /// No supported mandatory filesystem sandbox is available.
    #[error("channel filesystem sandbox is unavailable")]
    Sandbox,
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
        create_secure_directory(&root.join(".host-tmp"))?;
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
            if id.starts_with('.') {
                continue;
            }
            validate_channel_id(&id)?;
            validate_channel_tree(&entry.path())?;
            let (accounts, account_enabled) = account_state(&entry.path())?;
            rows.push(ChannelState {
                id,
                enabled: config_enabled(&entry.path())? || account_enabled,
                accounts,
                routes: yaml_sequence_count(&entry.path().join("routing.yaml"), "routes")?,
                pending_pairings: yaml_sequence_count(
                    &entry.path().join("pairing.yaml"),
                    "pending",
                )?,
                targets: json_array_count(&entry.path().join("targets.json"), "targets")?,
            });
        }
        if rows.len() > CHANNEL_STATE_ROWS_MAX {
            return Err(TopologyError::Path);
        }
        rows.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(rows)
    }
}

/// Returns whether canonical persisted state requests established channel restoration.
///
/// # Errors
/// Fails closed when canonical state cannot be validated.
pub fn channels_enabled(store: &ChannelStore) -> Result<bool, TopologyError> {
    if std::env::var_os(RESTORE_ENABLED_CHANNELS_ENV).is_none_or(|value| value != "1") {
        return Ok(false);
    }
    Ok(store.channel_state()?.iter().any(|channel| channel.enabled))
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
        .env("TMPDIR", store.root().join(".host-tmp"))
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
            channel_seatbelt_arguments(
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
                    channel_bubblewrap_arguments(
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

#[cfg(target_os = "macos")]
fn channel_seatbelt_arguments(
    channels: &Path,
    isolation: &Path,
    executable: &str,
    child_arguments: &[String],
) -> Vec<String> {
    const PROFILE: &str = concat!(
        "(version 1)\n",
        "(allow default)\n",
        "(deny file-write*)\n",
        "(deny file-read* (subpath (param \"ISOLATION_ROOT\")))\n",
        "(allow file-read-metadata (literal (param \"ISOLATION_ROOT\")))\n",
        "(allow file-read* file-write* (subpath (param \"CHANNELS_ROOT\")))\n",
        "(deny file-write* (subpath \"/dev\"))\n",
        "(allow file-read* (subpath \"/dev\"))\n",
        "(allow file-read-metadata (literal \"/dev/null\"))\n",
        "(allow file-read-data file-write-data (literal \"/dev/null\"))"
    );
    let mut arguments = vec![
        "-p".into(),
        PROFILE.into(),
        format!("-DISOLATION_ROOT={}", isolation.display()),
        format!("-DCHANNELS_ROOT={}", channels.display()),
        "--".into(),
        executable.into(),
    ];
    arguments.extend(child_arguments.iter().cloned());
    arguments
}

#[cfg(target_os = "linux")]
fn channel_bubblewrap_arguments(
    channels: &Path,
    isolation: &Path,
    executable: &str,
    child_arguments: &[String],
) -> Vec<String> {
    let channels = channels.to_string_lossy().into_owned();
    let isolation = isolation.to_string_lossy().into_owned();
    let mut arguments = vec![
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        isolation,
        "--bind".into(),
        channels.clone(),
        channels,
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--unshare-user".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--unshare-cgroup".into(),
        "--proc".into(),
        "/proc".into(),
        "--new-session".into(),
        "--die-with-parent".into(),
        "--close-fds".into(),
        "--".into(),
        executable.into(),
    ];
    arguments.extend(child_arguments.iter().cloned());
    arguments
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountsFile {
    accounts: Vec<lotta_domain::ChannelAccount>,
}

fn account_state(channel: &Path) -> Result<(usize, bool), TopologyError> {
    let path = channel.join("accounts.json");
    if !path.exists() {
        return Ok((0, false));
    }
    let bytes = read_regular_bounded(&path)?;
    let value: AccountsFile = serde_json::from_slice(&bytes).map_err(|_| TopologyError::Path)?;
    if value.accounts.len() > CHANNEL_STATE_ROWS_MAX {
        return Err(TopologyError::Path);
    }
    Ok((
        value.accounts.len(),
        value
            .accounts
            .iter()
            .any(|account| account.enabled && account.configured),
    ))
}

fn config_enabled(channel: &Path) -> Result<bool, TopologyError> {
    let path = channel.join("config.yaml");
    if !path.exists() {
        return Ok(false);
    }
    let bytes = read_regular_bounded(&path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| TopologyError::Path)?;
    let mut enabled = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "enabled" {
            if enabled.is_some() {
                return Err(TopologyError::Path);
            }
            enabled = Some(match value.trim() {
                "true" => true,
                "false" => false,
                _ => return Err(TopologyError::Path),
            });
        }
    }
    enabled.ok_or(TopologyError::Path)
}

fn json_array_count(path: &Path, key: &str) -> Result<usize, TopologyError> {
    if !path.exists() {
        return Ok(0);
    }
    let bytes = read_regular_bounded(path)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| TopologyError::Path)?;
    let object = value.as_object().ok_or(TopologyError::Path)?;
    if object.len() != 1 {
        return Err(TopologyError::Path);
    }
    let count = object
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or(TopologyError::Path)?
        .len();
    (count <= CHANNEL_STATE_ROWS_MAX)
        .then_some(count)
        .ok_or(TopologyError::Path)
}

fn yaml_sequence_count(path: &Path, key: &str) -> Result<usize, TopologyError> {
    if !path.exists() {
        return Ok(0);
    }
    let bytes = read_regular_bounded(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| TopologyError::Path)?;
    let marker = format!("{key}:");
    let mut active = false;
    let mut count = 0_usize;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            active = line.trim() == marker;
            continue;
        }
        if active && line.trim_start().starts_with("- ") {
            count = count.checked_add(1).ok_or(TopologyError::Path)?;
        }
    }
    (count <= CHANNEL_STATE_ROWS_MAX)
        .then_some(count)
        .ok_or(TopologyError::Path)
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>, TopologyError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| TopologyError::Path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TopologyError::Path);
    }
    #[cfg(unix)]
    if std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
        return Err(TopologyError::Path);
    }
    if metadata.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX as u64 {
        return Err(TopologyError::Path);
    }
    let bytes = std::fs::read(path).map_err(|_| TopologyError::Path)?;
    (bytes.len() <= crate::control_plane::CONTROL_FRAME_BYTES_MAX)
        .then_some(bytes)
        .ok_or(TopologyError::Path)
}

fn validate_channel_tree(root: &Path) -> Result<(), TopologyError> {
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|_| TopologyError::Path)? {
            visited = visited.checked_add(1).ok_or(TopologyError::Path)?;
            if visited > 512 {
                return Err(TopologyError::Path);
            }
            let entry = entry.map_err(|_| TopologyError::Path)?;
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| TopologyError::Path)?;
            if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
                return Err(TopologyError::Path);
            }
            #[cfg(unix)]
            if metadata.is_file() && std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
                return Err(TopologyError::Path);
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
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

    async fn serve_test_websocket(
        listener: tokio::net::TcpListener,
        observed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        use futures_util::StreamExt as _;
        use std::sync::atomic::Ordering;
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
        observed.store(headers.contains("Authorization: Bearer "), Ordering::SeqCst);
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while socket.next().await.is_some() {}
    }

    #[test]
    fn authenticates_over_loopback_and_reads_shared_channel_config() {
        let parent = temp_root("supervision");
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        std::fs::create_dir(store.root().join("telegram")).unwrap();
        std::fs::write(store.root().join("telegram/config.yaml"), "enabled: true\n").unwrap();
        std::fs::write(
            store.root().join("telegram/accounts.json"),
            "{\"accounts\":[]}",
        )
        .unwrap();
        let rows = store.channel_state().unwrap();
        assert_eq!(rows[0].id, "telegram");
        assert!(rows[0].enabled);
        assert_eq!(rows[0].accounts, 0);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn spawns_child_authenticates_publishes_and_reaps() {
        use crate::{
            control_plane::RuntimeKey,
            supervisor::{ChannelLaunchConfig, ChannelSupervisor},
        };
        use std::{
            os::unix::fs::PermissionsExt,
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            time::Duration,
        };
        let parent = temp_root("real-supervision");
        let script = parent.parent().unwrap().join(format!(
            "channel-helper-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&script, HELPER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let authenticator =
            lotta_app_server::auth::channel_session::ChannelSessionAuthenticator::new();
        authenticator
            .bind_listener("127.0.0.1", "/ws", "test-listener")
            .unwrap();
        let registry = Arc::new(lotta_tools::ToolRegistry::new([]).unwrap());
        let tools = Arc::new(lotta_tools::external::ChannelExternalToolManager::new(
            registry,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let authenticated = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&authenticated);
        let server = tokio::spawn(serve_test_websocket(listener, observed));
        let supervisor = ChannelSupervisor::start(ChannelLaunchConfig {
            executable: script.clone(),
            store: ChannelStore::under_letta_home(&parent).unwrap(),
            websocket_url: format!("ws://127.0.0.1:{}/ws", address.port()),
            owner_prefix: "real-child".into(),
            authenticator,
            tools,
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
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&parent, store.root().join("escape")).unwrap();
            assert_eq!(store.channel_state(), Err(TopologyError::Path));
        }
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_hardlinked_channel_state() {
        let parent = temp_root("hardlink");
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        let channel = store.root().join("telegram");
        std::fs::create_dir(&channel).unwrap();
        let source = parent.join("outside.json");
        std::fs::write(&source, "{\"accounts\":[]}").unwrap();
        std::fs::hard_link(&source, channel.join("accounts.json")).unwrap();
        assert_eq!(store.channel_state(), Err(TopologyError::Path));
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
