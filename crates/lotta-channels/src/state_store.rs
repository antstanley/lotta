//! Typed, bounded reader for the pinned ChannelGateway side-store layout.

use crate::{
    control_plane::{CHANNEL_STATE_ROWS_MAX, ChannelState},
    topology::{ChannelStore, TopologyError},
};
use lotta_domain::{ChannelChatType, ChannelRoute};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Maximum entries recursively inspected below one channel directory.
pub const CHANNEL_TREE_ENTRIES_MAX: usize = 512;
/// Maximum plugin-owned configuration entries retained from a custom account.
pub const CHANNEL_PLUGIN_CONFIG_ENTRIES_MAX: usize = 128;
/// Pinned legacy account identifier used when an old record omits `accountId`.
pub const LEGACY_CHANNEL_ACCOUNT_ID: &str = "__legacy_migrated__";

/// Typed canonical channel state reader.
#[derive(Clone, Debug)]
pub struct ChannelStateStore {
    root: PathBuf,
}

/// Common, typed fields in the pinned `snake_case` `config.yaml` format.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    /// Administrative enabled state. Missing means enabled in the baseline codecs.
    pub enabled: Option<bool>,
    /// Telegram/Discord token.
    pub token: Option<String>,
    /// Slack bot token.
    pub bot_token: Option<String>,
    /// Slack app token.
    pub app_token: Option<String>,
    /// Direct-message policy.
    pub dm_policy: Option<DmPolicy>,
    /// Allowed sender identifiers.
    pub allowed_users: Option<Vec<String>>,
    /// Group handling mode.
    pub group_mode: Option<GroupMode>,
    /// Voice transcription toggle.
    pub transcribe_voice: Option<bool>,
    /// Telegram rich private-chat default.
    pub rich_private_chat_default: Option<bool>,
    /// Telegram rich draft streaming toggle.
    pub rich_draft_streaming: Option<bool>,
    /// Signal bridge base URL.
    pub base_url: Option<String>,
    /// Account-bound agent, where supported. Explicit null is meaningful.
    pub agent_id: Option<Option<String>>,
    /// Self-chat mode.
    pub self_chat_mode: Option<bool>,
    /// Account selector for Signal.
    pub account: Option<String>,
    /// Optional allowed groups.
    pub allowed_groups: Option<Vec<String>>,
    /// Optional allowed Discord channels.
    pub allowed_channels: Option<AllowedChannels>,
    /// Default permission mode.
    pub default_permission_mode: Option<PermissionMode>,
    /// Inbound debounce bounded to 0..10000 after decode.
    pub inbound_debounce_ms: Option<u16>,
    /// Media download toggle.
    pub download_media: Option<bool>,
    /// Maximum media bytes.
    pub media_max_bytes: Option<u64>,
}

/// Pinned direct-message policy vocabulary.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Pair unknown users.
    Pairing,
    /// Permit allowlisted users.
    Allowlist,
    /// Permit all users.
    Open,
}

/// Pinned union of first-party group modes.
#[derive(Clone, Copy, Debug, Deserialize)]
pub enum GroupMode {
    /// Open group messages.
    #[serde(rename = "open")]
    Open,
    /// Mention-only group messages.
    #[serde(rename = "mention-only")]
    MentionOnly,
    /// Mention-gated group messages.
    #[serde(rename = "mention")]
    Mention,
    /// Disabled group messages.
    #[serde(rename = "disabled")]
    Disabled,
}

/// Exact canonical `accounts.json` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAccounts {
    accounts: Vec<CanonicalAccount>,
}

/// Pinned camelCase common account record plus typed first-party plugin fields.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalAccount {
    channel: String,
    account_id: String,
    display_name: Option<String>,
    enabled: bool,
    dm_policy: DmPolicy,
    allowed_users: Vec<String>,
    group_policy: Option<GroupPolicy>,
    admin_users: Option<Vec<String>>,
    user_allowed_commands: Option<Vec<String>>,
    created_at: String,
    updated_at: String,
    token: Option<String>,
    mode: Option<SlackMode>,
    bot_token: Option<String>,
    app_token: Option<String>,
    binding: Option<AccountBinding>,
    agent_id: Option<String>,
    default_permission_mode: Option<PermissionMode>,
    group_mode: Option<GroupMode>,
    transcribe_voice: Option<bool>,
    rich_private_chat_default: Option<bool>,
    rich_draft_streaming: Option<bool>,
    listen_mode: Option<bool>,
    mention_only_channels: Option<Vec<String>>,
    allowed_channels: Option<AllowedChannels>,
    allow_bots: Option<AllowBots>,
    auto_thread_on_mention: Option<bool>,
    thread_policy_by_channel: Option<BTreeMap<String, bool>>,
    acknowledge_message_reaction: Option<bool>,
    remove_stale_routes: Option<bool>,
    inbound_debounce_ms: Option<u16>,
    base_url: Option<String>,
    account: Option<String>,
    account_uuid: Option<String>,
    self_chat_mode: Option<bool>,
    allowed_groups: Option<Vec<String>>,
    mention_patterns: Option<Vec<String>>,
    recipient_aliases: Option<BTreeMap<String, String>>,
    download_media: Option<bool>,
    media_max_bytes: Option<u64>,
    attachment_filter: Option<bool>,
    attachment_mime_types: Option<Vec<String>>,
    attachment_allowed_recipients: Option<Vec<String>>,
    attachment_allowed_paths: Option<Vec<String>>,
    attachment_path_recursive: Option<bool>,
    waiting_behavior: Option<WaitingBehavior>,
    message_prefix: Option<String>,
    config: Option<PluginConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GroupPolicy {
    Open,
    Allowlist,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SlackMode {
    Socket,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
enum AllowBots {
    Disabled(bool),
    Mentions(AllowBotsMentions),
}

#[derive(Clone, Copy, Debug, Deserialize)]
enum AllowBotsMentions {
    #[serde(rename = "mentions")]
    Mentions,
}

#[derive(Clone, Copy, Debug, Deserialize)]
enum WaitingBehavior {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "typing_indicator")]
    TypingIndicator,
}

/// Pinned permission-mode vocabulary.
#[derive(Clone, Copy, Debug, Deserialize)]
pub enum PermissionMode {
    /// Standard guarded mode.
    #[serde(rename = "standard")]
    Standard,
    /// Accept edits without unrestricted execution.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Unrestricted mode.
    #[serde(rename = "unrestricted")]
    Unrestricted,
}

/// Pinned Discord allowlist supports a list or per-channel mode map.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum AllowedChannels {
    /// Legacy mention-only channel identifiers.
    List(Vec<String>),
    /// Per-channel open or mention-only modes.
    Modes(BTreeMap<String, DiscordChannelMode>),
}

/// Discord channel routing mode.
#[derive(Clone, Copy, Debug, Deserialize)]
pub enum DiscordChannelMode {
    /// Process every message.
    #[serde(rename = "open")]
    Open,
    /// Process explicit mentions only.
    #[serde(rename = "mention-only")]
    MentionOnly,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountBinding {
    agent_id: Option<String>,
    conversation_id: Option<String>,
}

/// Baseline custom-channel account config is intentionally plugin-owned.
#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
struct PluginConfig(BTreeMap<String, Value>);

/// Exact canonical `routing.yaml` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRouting {
    routes: Vec<CanonicalRoute>,
}

/// Pinned camelCase route record. The directory supplies the channel identity.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CanonicalRoute {
    account_id: Option<String>,
    chat_id: String,
    chat_type: Option<ChannelChatType>,
    thread_id: Option<String>,
    agent_id: String,
    conversation_id: String,
    enabled: bool,
    outbound_enabled: Option<bool>,
    detached: Option<bool>,
    created_at: String,
    updated_at: Option<String>,
}

/// Exact canonical `pairing.yaml` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPairing {
    pending: Vec<PendingPairing>,
    approved: Vec<ApprovedUser>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingPairing {
    account_id: Option<String>,
    code: String,
    sender_id: String,
    sender_name: Option<String>,
    chat_id: String,
    created_at: String,
    expires_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovedUser {
    account_id: Option<String>,
    sender_id: String,
    sender_name: Option<String>,
    approved_at: String,
}

/// Exact canonical `targets.json` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelTargets {
    targets: Vec<ChannelTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChannelTarget {
    account_id: Option<String>,
    target_id: String,
    target_type: TargetType,
    chat_id: String,
    label: String,
    discovered_at: String,
    last_seen_at: String,
    last_message_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TargetType {
    Direct,
    Channel,
}

/// One fully decoded canonical channel directory.
#[derive(Clone, Debug)]
pub struct CanonicalChannelState {
    /// Canonical channel identifier.
    pub id: String,
    /// Typed plugin configuration.
    pub config: Option<ChannelConfig>,
    /// Canonical accounts envelope.
    pub accounts: ChannelAccounts,
    /// Canonical routing envelope.
    pub routing: ChannelRouting,
    /// Canonical pairing envelope.
    pub pairing: ChannelPairing,
    /// Canonical targets envelope.
    pub targets: ChannelTargets,
}

impl ChannelStateStore {
    /// Creates a typed reader over an already validated canonical root.
    #[must_use]
    pub fn new(store: &ChannelStore) -> Self {
        Self::from_root(store.root())
    }

    pub(crate) fn from_root(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Decodes every canonical channel directory without aliases or shallow parsing.
    ///
    /// # Errors
    /// Fails closed for malformed, oversized, linked, special, or schema-invalid state.
    pub fn load(&self) -> Result<Vec<CanonicalChannelState>, TopologyError> {
        let mut channels = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|_| TopologyError::Path)? {
            let entry = entry.map_err(|_| TopologyError::Path)?;
            if !safe_metadata(&entry.path())?.is_dir() {
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
            validate_tree(&entry.path())?;
            channels.push(load_channel(id, &entry.path())?);
            bounded(channels.len())?;
        }
        channels.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(channels)
    }

    /// Returns the canonical bounded `/channels` projection.
    ///
    /// # Errors
    /// Fails closed when any canonical channel record is invalid.
    pub fn snapshot(&self) -> Result<Vec<ChannelState>, TopologyError> {
        self.load()?.into_iter().map(channel_snapshot).collect()
    }

    /// Returns every unique enabled route whose canonical account is enabled and matched.
    ///
    /// # Errors
    /// Fails closed for invalid records, identifiers, timestamps, or bounds.
    pub fn restorable_routes(&self) -> Result<Vec<ChannelRoute>, TopologyError> {
        let mut routes = Vec::new();
        let mut unique = std::collections::BTreeSet::new();
        for channel in self.load()? {
            append_restorable(&channel, &mut routes, &mut unique)?;
        }
        Ok(routes)
    }

    /// Returns whether canonical persisted state has an enabled matched route.
    ///
    /// # Errors
    /// Fails closed when canonical state cannot be validated.
    pub fn has_restorable_account(&self) -> Result<bool, TopologyError> {
        Ok(!self.restorable_routes()?.is_empty())
    }
}

fn channel_snapshot(channel: CanonicalChannelState) -> Result<ChannelState, TopologyError> {
    for count in [
        channel.accounts.accounts.len(),
        channel.routing.routes.len(),
        channel.pairing.pending.len(),
        channel.pairing.approved.len(),
        channel.targets.targets.len(),
    ] {
        bounded(count)?;
    }
    validate_decoded_records(&channel)?;
    let enabled = channel.routing.routes.iter().any(|route| {
        route.enabled && matched_enabled_account(&channel.accounts.accounts, &channel.id, route)
    });
    Ok(ChannelState {
        id: channel.id,
        enabled,
        accounts: channel.accounts.accounts.len(),
        routes: channel.routing.routes.len(),
        pending_pairings: channel.pairing.pending.len(),
        targets: channel.targets.targets.len(),
    })
}

type UniqueRouteKey = (String, String, String, Option<String>, String, String);

fn append_restorable(
    channel: &CanonicalChannelState,
    routes: &mut Vec<ChannelRoute>,
    unique: &mut std::collections::BTreeSet<UniqueRouteKey>,
) -> Result<(), TopologyError> {
    validate_decoded_records(channel)?;
    for route in &channel.routing.routes {
        if !route.enabled
            || !matched_enabled_account(&channel.accounts.accounts, &channel.id, route)
        {
            continue;
        }
        let projected = project_route(&channel.id, route)?;
        let key = (
            projected.channel_id.as_str().to_owned(),
            projected.account_id.as_str().to_owned(),
            projected.chat_id.as_str().to_owned(),
            projected.thread_id.clone().flatten(),
            projected.agent_id.as_str().to_owned(),
            projected.conversation_id.as_str().to_owned(),
        );
        if unique.insert(key) {
            routes.push(projected);
        }
        bounded(routes.len())?;
    }
    Ok(())
}

fn matched_enabled_account(
    accounts: &[CanonicalAccount],
    channel: &str,
    route: &CanonicalRoute,
) -> bool {
    let route_account = route
        .account_id
        .as_deref()
        .unwrap_or(LEGACY_CHANNEL_ACCOUNT_ID);
    accounts.iter().any(|account| {
        account.channel == channel
            && account.account_id == route_account
            && account.enabled
            && account_is_configured(account)
    })
}

fn account_is_configured(account: &CanonicalAccount) -> bool {
    match account.channel.as_str() {
        "telegram" => account.binding.is_some(),
        "slack" => account.mode.is_some(),
        "discord" => account.agent_id.is_some(),
        "whatsapp" => account.agent_id.is_some() && account.self_chat_mode.is_some(),
        "signal" => {
            account.agent_id.is_some() && account.base_url.as_deref().is_some_and(|v| !v.is_empty())
        }
        _ => account
            .config
            .as_ref()
            .is_some_and(|config| config.0.len() <= CHANNEL_PLUGIN_CONFIG_ENTRIES_MAX),
    }
}

fn project_route(channel: &str, route: &CanonicalRoute) -> Result<ChannelRoute, TopologyError> {
    let value = serde_json::json!({
        "channel_id": channel,
        "account_id": route.account_id.as_deref().unwrap_or(LEGACY_CHANNEL_ACCOUNT_ID),
        "chat_id": route.chat_id,
        "chat_type": route.chat_type,
        "thread_id": route.thread_id,
        "agent_id": route.agent_id,
        "conversation_id": route.conversation_id,
        "enabled": route.enabled,
        "outbound_enabled": route.outbound_enabled.unwrap_or(true),
        "created_at": route.created_at,
        "updated_at": route.updated_at.as_deref().unwrap_or(&route.created_at)
    });
    serde_json::from_value(value).map_err(|_| TopologyError::Path)
}

fn observe_account_schema(account: &CanonicalAccount) {
    let _ = (
        &account.display_name,
        &account.dm_policy,
        &account.group_policy,
        &account.admin_users,
        &account.user_allowed_commands,
        &account.created_at,
        &account.updated_at,
        &account.token,
        &account.bot_token,
        &account.app_token,
        &account.default_permission_mode,
        &account.group_mode,
        &account.transcribe_voice,
        &account.rich_private_chat_default,
        &account.rich_draft_streaming,
        &account.listen_mode,
        &account.mention_only_channels,
        &account.allowed_channels,
        &account.auto_thread_on_mention,
        &account.thread_policy_by_channel,
        &account.acknowledge_message_reaction,
        &account.remove_stale_routes,
        &account.account,
        &account.account_uuid,
        &account.allowed_groups,
        &account.mention_patterns,
        &account.recipient_aliases,
        &account.download_media,
        &account.media_max_bytes,
        &account.attachment_filter,
        &account.attachment_mime_types,
        &account.attachment_allowed_recipients,
        &account.attachment_allowed_paths,
        &account.attachment_path_recursive,
        &account.waiting_behavior,
        &account.message_prefix,
    );
    if let Some(binding) = &account.binding {
        let _ = (&binding.agent_id, &binding.conversation_id);
    }
    if let Some(AllowBots::Disabled(value)) = account.allow_bots {
        let _ = value;
    }
}

fn observe_other_schema(channel: &CanonicalChannelState) {
    for route in &channel.routing.routes {
        let _ = route.detached;
    }
    for pending in &channel.pairing.pending {
        let _ = (
            &pending.account_id,
            &pending.code,
            &pending.sender_id,
            &pending.sender_name,
            &pending.chat_id,
            &pending.created_at,
            &pending.expires_at,
        );
    }
    for approved in &channel.pairing.approved {
        let _ = (
            &approved.account_id,
            &approved.sender_id,
            &approved.sender_name,
            &approved.approved_at,
        );
    }
    for target in &channel.targets.targets {
        let _ = (
            &target.account_id,
            &target.target_id,
            &target.target_type,
            &target.chat_id,
            &target.label,
            &target.discovered_at,
            &target.last_seen_at,
            &target.last_message_id,
        );
    }
}

fn validate_decoded_records(channel: &CanonicalChannelState) -> Result<(), TopologyError> {
    observe_other_schema(channel);
    for account in &channel.accounts.accounts {
        observe_account_schema(account);
        validate_text(&account.channel)?;
        validate_text(&account.account_id)?;
        bounded(account.allowed_users.len())?;
        if account.channel != channel.id {
            return Err(TopologyError::Path);
        }
        if account
            .inbound_debounce_ms
            .is_some_and(|value| value > 10_000)
        {
            return Err(TopologyError::Path);
        }
        if account
            .config
            .as_ref()
            .is_some_and(|config| config.0.len() > CHANNEL_PLUGIN_CONFIG_ENTRIES_MAX)
        {
            return Err(TopologyError::Path);
        }
    }
    for route in &channel.routing.routes {
        validate_text(&route.chat_id)?;
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), TopologyError> {
    if value.is_empty() || value.len() > crate::control_plane::CONTROL_STRING_BYTES_MAX {
        Err(TopologyError::Path)
    } else {
        Ok(())
    }
}

fn load_channel(id: String, root: &Path) -> Result<CanonicalChannelState, TopologyError> {
    Ok(CanonicalChannelState {
        id,
        config: read_optional_yaml(&root.join("config.yaml"))?,
        accounts: read_json_or(
            &root.join("accounts.json"),
            ChannelAccounts { accounts: vec![] },
        )?,
        routing: read_yaml_or(
            &root.join("routing.yaml"),
            ChannelRouting { routes: vec![] },
        )?,
        pairing: read_yaml_or(
            &root.join("pairing.yaml"),
            ChannelPairing {
                pending: vec![],
                approved: vec![],
            },
        )?,
        targets: read_json_or(
            &root.join("targets.json"),
            ChannelTargets { targets: vec![] },
        )?,
    })
}

fn read_optional_yaml(path: &Path) -> Result<Option<ChannelConfig>, TopologyError> {
    if !path.exists() {
        return Ok(None);
    }
    serde_yaml::from_slice(&read_regular_bounded(path)?)
        .map(Some)
        .map_err(|_| TopologyError::Path)
}

fn read_json_or<T: for<'de> Deserialize<'de>>(path: &Path, default: T) -> Result<T, TopologyError> {
    if !path.exists() {
        return Ok(default);
    }
    serde_json::from_slice(&read_regular_bounded(path)?).map_err(|_| TopologyError::Path)
}

fn read_yaml_or<T: for<'de> Deserialize<'de>>(path: &Path, default: T) -> Result<T, TopologyError> {
    if !path.exists() {
        return Ok(default);
    }
    serde_yaml::from_slice(&read_regular_bounded(path)?).map_err(|_| TopologyError::Path)
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>, TopologyError> {
    let metadata = safe_metadata(path)?;
    if !metadata.is_file() || metadata.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX as u64
    {
        return Err(TopologyError::Path);
    }
    #[cfg(unix)]
    if std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
        return Err(TopologyError::Path);
    }
    let bytes = std::fs::read(path).map_err(|_| TopologyError::Path)?;
    if bytes.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX {
        return Err(TopologyError::Path);
    }
    Ok(bytes)
}

fn safe_metadata(path: &Path) -> Result<std::fs::Metadata, TopologyError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| TopologyError::Path)?;
    if metadata.file_type().is_symlink() {
        return Err(TopologyError::Path);
    }
    Ok(metadata)
}

fn validate_tree(root: &Path) -> Result<(), TopologyError> {
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| TopologyError::Path)? {
            visited = visited.checked_add(1).ok_or(TopologyError::Path)?;
            if visited > CHANNEL_TREE_ENTRIES_MAX {
                return Err(TopologyError::Path);
            }
            let entry = entry.map_err(|_| TopologyError::Path)?;
            let metadata = safe_metadata(&entry.path())?;
            if !(metadata.is_file() || metadata.is_dir()) {
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
        || id.len() > crate::topology::CHANNEL_OWNER_ID_BYTES_MAX
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(TopologyError::Path);
    }
    Ok(())
}

fn bounded(count: usize) -> Result<(), TopologyError> {
    (count <= CHANNEL_STATE_ROWS_MAX)
        .then_some(())
        .ok_or(TopologyError::Path)
}
