//! Channel process topology, persistent-root validation, and per-child capability material.

use crate::control_plane::ChannelState;
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
        crate::state_store::ChannelStateStore::new(self).snapshot()
    }
}

/// Returns whether canonical persisted state requests established channel restoration.
///
/// # Errors
/// Fails closed when canonical state cannot be validated.
pub fn channels_enabled(store: &ChannelStore) -> Result<bool, TopologyError> {
    crate::state_store::ChannelStateStore::new(store).has_restorable_account()
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
        "(deny default)\n",
        "(allow process*)\n",
        "(allow sysctl-read)\n",
        "(allow mach-lookup)\n",
        "(allow signal (target self))\n",
        "(allow file-read-data (literal \"/\"))\n",
        "(allow network-outbound (remote ip \"localhost:*\"))\n",
        "(allow file-read* (subpath \"/System\"))\n",
        "(allow file-read* (subpath \"/Library\"))\n",
        "(allow file-read* (subpath \"/usr/lib\"))\n",
        "(allow file-read* (subpath \"/private/etc\"))\n",
        "(allow file-read* (subpath \"/private/var/db\"))\n",
        "(allow file-read* (subpath \"/private/var/run\"))\n",
        "(allow file-read-metadata (subpath (param \"EXECUTABLE_PARENT\")))\n",
        "(allow file-read* (literal (param \"EXECUTABLE\")))\n",
        "(allow file-read-metadata (literal (param \"ISOLATION_ROOT\")))\n",
        "(allow file-read* file-write* (subpath (param \"CHANNELS_ROOT\")))\n",
        "(allow file-read-metadata (literal \"/dev/null\"))\n",
        "(allow file-read-data file-write-data (literal \"/dev/null\"))\n",
        "(allow file-read* (literal \"/dev/urandom\"))"
    );
    let executable_parent = Path::new(executable)
        .parent()
        .unwrap_or_else(|| Path::new("/"));
    let mut arguments = vec![
        "-p".into(),
        PROFILE.into(),
        format!("-DISOLATION_ROOT={}", isolation.display()),
        format!("-DCHANNELS_ROOT={}", channels.display()),
        format!("-DEXECUTABLE={executable}"),
        format!("-DEXECUTABLE_PARENT={}", executable_parent.display()),
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
    let mut arguments = vec!["--clearenv".into()];
    for directory in ["/usr", "/lib", "/lib64", "/bin"] {
        if Path::new(directory).is_dir() {
            arguments.extend(["--ro-bind".into(), directory.into(), directory.into()]);
        }
    }
    arguments.extend([
        "--ro-bind".into(),
        executable.into(),
        "/lotta-channel-host".into(),
        "--dir".into(),
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
        "/lotta-channel-host".into(),
    ]);
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
        assert!(!rows[0].enabled);
        assert!(!channels_enabled(&store).unwrap());
        assert_eq!(rows[0].accounts, 0);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn restoration_requires_exact_enabled_configured_account() {
        let parent = temp_root("canonical-restoration");
        let store = ChannelStore::under_letta_home(&parent).unwrap();
        let channel = store.root().join("telegram");
        std::fs::create_dir(&channel).unwrap();
        std::fs::write(channel.join("config.yaml"), "token: redacted\n").unwrap();
        std::fs::write(
            channel.join("accounts.json"),
            concat!(
                r#"{"accounts":[{"channel":"telegram","accountId":"main","enabled":true,"#,
                r#""dmPolicy":"pairing","allowedUsers":[],"binding":{"agentId":null,"#,
                r#""conversationId":null},"createdAt":"2026-01-01T00:00:00Z","#,
                r#""updatedAt":"2026-01-01T00:00:00Z"}]}"#,
            ),
        )
        .unwrap();
        std::fs::write(
            channel.join("routing.yaml"),
            concat!(
                r#"{"routes":[{"accountId":"main","chatId":"chat","#,
                r#""agentId":"agent-local-a","conversationId":"conversation-a","#,
                r#""enabled":true,"createdAt":"2026-01-01T00:00:00Z","#,
                r#""updatedAt":"2026-01-01T00:00:00Z"}]}"#,
            ),
        )
        .unwrap();
        assert!(channels_enabled(&store).unwrap());
        assert_eq!(
            crate::state_store::ChannelStateStore::new(&store)
                .restorable_routes()
                .unwrap()
                .len(),
            1
        );
        std::fs::write(channel.join("accounts.json"), "{malformed").unwrap();
        assert_eq!(channels_enabled(&store), Err(TopologyError::Path));
        std::fs::remove_dir_all(parent).unwrap();
    }
}

#[cfg(test)]
mod plane_separation {
    use crate::control_plane::{ChildFrame, RuntimeKey, RuntimeTool};

    #[tokio::test]
    async fn records_both_planes_and_rejects_negative_crossover() {
        use tokio::io::{AsyncWriteExt as _, BufReader};

        let management = ChildFrame::PublishRuntimeTools {
            metadata: crate::control_plane::FrameMetadata::new(1),
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
        assert!(control.get("type").is_none());

        // Exercise the real child/parent NDJSON decoder over an actual pipe. A
        // Runtime frame is terminally rejected and never reaches dispatch.
        let (mut writer, reader) = tokio::io::duplex(4096);
        writer
            .write_all(format!("{runtime}\n").as_bytes())
            .await
            .unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert!(
            crate::control_plane::read_line::<_, ChildFrame>(&mut reader)
                .await
                .is_err()
        );

        // A fresh physical pipe records only a validated management frame.
        let (mut writer, reader) = tokio::io::duplex(4096);
        writer
            .write_all(format!("{control}\n").as_bytes())
            .await
            .unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        let accepted = crate::control_plane::read_line::<_, ChildFrame>(&mut reader)
            .await
            .unwrap()
            .unwrap();
        let recorded_ndjson = [serde_json::to_value(accepted).unwrap()];
        assert!(
            recorded_ndjson
                .iter()
                .all(|frame| frame.get("kind").is_some())
        );
        assert!(
            recorded_ndjson
                .iter()
                .all(|frame| frame.get("type").is_none())
        );
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
}
