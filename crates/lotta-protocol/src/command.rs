//! Exact protocol tag membership extracted from the pinned TypeScript union.

use serde::{Deserialize, Serialize};

macro_rules! tagged_enum {
    ($name:ident, $all:ident, $(($variant:ident, $tag:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(tag = "type")]
        /// A protocol tag envelope; payload schemas are deferred to Tasks 62–73.
        pub enum $name {
            $(
                #[doc = concat!("Exact `", $tag, "` protocol tag.")]
                #[serde(rename = $tag)]
                $variant,
            )+
        }

        /// Every supported discriminant in pinned union order.
        pub const $all: &[&str] = &[$($tag),+];

        impl $name {
            /// Returns the stable wire discriminant.
            #[must_use]
            pub const fn discriminant(self) -> &'static str {
                match self { $(Self::$variant => $tag),+ }
            }

            /// Resolves an exact supported wire discriminant.
            #[must_use]
            pub fn from_discriminant(value: &str) -> Option<Self> {
                match value { $($tag => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

tagged_enum! {
    WsProtocolCommand,
    ALL_COMMAND_DISCRIMINANTS,
    (Input, "input"),
    (ChangeDeviceState, "change_device_state"),
    (AbortMessage, "abort_message"),
    (Sync, "sync"),
    (RuntimeStart, "runtime_start"),
    (TeleportProbe, "teleport_probe"),
    (TeleportRequest, "teleport_request"),
    (TeleportFailed, "teleport_failed"),
    (RuntimeExternalToolsUpdate, "runtime_external_tools_update"),
    (ExternalToolCallResponse, "external_tool_call_response"),
    (TerminalSpawn, "terminal_spawn"),
    (TerminalInput, "terminal_input"),
    (TerminalResize, "terminal_resize"),
    (TerminalKill, "terminal_kill"),
    (SearchFiles, "search_files"),
    (GrepInFiles, "grep_in_files"),
    (ListInDirectory, "list_in_directory"),
    (GetTree, "get_tree"),
    (ReadFile, "read_file"),
    (WriteFile, "write_file"),
    (WatchFile, "watch_file"),
    (UnwatchFile, "unwatch_file"),
    (EditFile, "edit_file"),
    (FileOps, "file_ops"),
    (ListMemory, "list_memory"),
    (MemoryHistory, "memory_history"),
    (MemoryFileAtRef, "memory_file_at_ref"),
    (MemoryCommitDiff, "memory_commit_diff"),
    (ReadMemoryFile, "read_memory_file"),
    (WriteMemoryFile, "write_memory_file"),
    (DeleteMemoryFile, "delete_memory_file"),
    (EnableMemfs, "enable_memfs"),
    (ListModels, "list_models"),
    (ListConnectProviders, "list_connect_providers"),
    (ConnectProvider, "connect_provider"),
    (DisconnectProvider, "disconnect_provider"),
    (ChatgptUsageRead, "chatgpt_usage_read"),
    (UpdateModel, "update_model"),
    (UpdateToolset, "update_toolset"),
    (CronList, "cron_list"),
    (CronAdd, "cron_add"),
    (CronGet, "cron_get"),
    (CronRuns, "cron_runs"),
    (CronTrigger, "cron_trigger"),
    (CronUpdate, "cron_update"),
    (CronDelete, "cron_delete"),
    (CronDeleteAll, "cron_delete_all"),
    (SkillEnable, "skill_enable"),
    (SkillDisable, "skill_disable"),
    (CreateAgent, "create_agent"),
    (AppServerInfo, "app_server_info"),
    (AgentList, "agent_list"),
    (AgentRetrieve, "agent_retrieve"),
    (AgentCreate, "agent_create"),
    (AgentUpdate, "agent_update"),
    (AgentDelete, "agent_delete"),
    (ConversationList, "conversation_list"),
    (ConversationRetrieve, "conversation_retrieve"),
    (ConversationCreate, "conversation_create"),
    (ConversationUpdate, "conversation_update"),
    (ConversationRecompile, "conversation_recompile"),
    (ConversationFork, "conversation_fork"),
    (ConversationMessagesList, "conversation_messages_list"),
    (ConversationCompact, "conversation_compact"),
    (GetCwdMap, "get_cwd_map"),
    (GetReflectionSettings, "get_reflection_settings"),
    (SetReflectionSettings, "set_reflection_settings"),
    (GetExperiments, "get_experiments"),
    (SetExperiment, "set_experiment"),
    (ChannelsList, "channels_list"),
    (ChannelAccountsList, "channel_accounts_list"),
    (ChannelAccountCreate, "channel_account_create"),
    (ChannelAccountUpdate, "channel_account_update"),
    (ChannelAccountBind, "channel_account_bind"),
    (ChannelAccountUnbind, "channel_account_unbind"),
    (ChannelAccountDelete, "channel_account_delete"),
    (ChannelAccountStart, "channel_account_start"),
    (ChannelAccountStop, "channel_account_stop"),
    (ChannelGetConfig, "channel_get_config"),
    (ChannelSetConfig, "channel_set_config"),
    (ChannelStart, "channel_start"),
    (ChannelStop, "channel_stop"),
    (ChannelPairingsList, "channel_pairings_list"),
    (ChannelPairingBind, "channel_pairing_bind"),
    (ChannelRoutesList, "channel_routes_list"),
    (ChannelTargetsList, "channel_targets_list"),
    (ChannelTargetBind, "channel_target_bind"),
    (ChannelRouteRemove, "channel_route_remove"),
    (ChannelRouteUpdate, "channel_route_update"),
    (ExecuteCommand, "execute_command"),
    (RemoveQueueItem, "remove_queue_item"),
    (SearchBranches, "search_branches"),
    (CheckoutBranch, "checkout_branch"),
    (SecretList, "secret_list"),
    (SecretApply, "secret_apply"),
}
