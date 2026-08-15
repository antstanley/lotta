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
    WsProtocolMessage,
    ALL_MESSAGE_DISCRIMINANTS,
    (ControlRequest, "control_request"),
    (InputAccepted, "input_accepted"),
    (TeleportProbeResponse, "teleport_probe_response"),
    (TeleportReady, "teleport_ready"),
    (ExecuteCommandResponse, "execute_command_response"),
    (UpdateDeviceStatus, "update_device_status"),
    (UpdateLoopStatus, "update_loop_status"),
    (UpdateQueue, "update_queue"),
    (StreamDelta, "stream_delta"),
    (TurnFinished, "turn_finished"),
    (UpdateSubagentState, "update_subagent_state"),
    (ExternalToolCallRequest, "external_tool_call_request"),
    (AbortMessageResponse, "abort_message_response"),
    (SyncResponse, "sync_response"),
    (RuntimeExternalToolsUpdateResponse, "runtime_external_tools_update_response"),
    (TerminalOutput, "terminal_output"),
    (TerminalSpawned, "terminal_spawned"),
    (TerminalExited, "terminal_exited"),
    (SearchFilesResponse, "search_files_response"),
    (GrepInFilesResponse, "grep_in_files_response"),
    (ListInDirectoryResponse, "list_in_directory_response"),
    (GetTreeResponse, "get_tree_response"),
    (ReadFileResponse, "read_file_response"),
    (WriteFileResponse, "write_file_response"),
    (FileOps, "file_ops"),
    (EditFileResponse, "edit_file_response"),
    (FileChanged, "file_changed"),
    (ListMemoryResponse, "list_memory_response"),
    (MemoryHistoryResponse, "memory_history_response"),
    (MemoryFileAtRefResponse, "memory_file_at_ref_response"),
    (MemoryCommitDiffResponse, "memory_commit_diff_response"),
    (ReadMemoryFileResponse, "read_memory_file_response"),
    (WriteMemoryFileResponse, "write_memory_file_response"),
    (DeleteMemoryFileResponse, "delete_memory_file_response"),
    (EnableMemfsResponse, "enable_memfs_response"),
    (MemoryUpdated, "memory_updated"),
    (ListModelsResponse, "list_models_response"),
    (ListConnectProvidersResponse, "list_connect_providers_response"),
    (ConnectProviderResponse, "connect_provider_response"),
    (DisconnectProviderResponse, "disconnect_provider_response"),
    (ChatgptUsageReadResponse, "chatgpt_usage_read_response"),
    (UpdateModelResponse, "update_model_response"),
    (UpdateToolsetResponse, "update_toolset_response"),
    (CronListResponse, "cron_list_response"),
    (CronAddResponse, "cron_add_response"),
    (CronGetResponse, "cron_get_response"),
    (CronRunsResponse, "cron_runs_response"),
    (CronTriggerResponse, "cron_trigger_response"),
    (CronUpdateResponse, "cron_update_response"),
    (CronDeleteResponse, "cron_delete_response"),
    (CronDeleteAllResponse, "cron_delete_all_response"),
    (CronsUpdated, "crons_updated"),
    (SkillEnableResponse, "skill_enable_response"),
    (SkillDisableResponse, "skill_disable_response"),
    (SkillsUpdated, "skills_updated"),
    (CreateAgentResponse, "create_agent_response"),
    (AppServerInfoResponse, "app_server_info_response"),
    (AgentListResponse, "agent_list_response"),
    (AgentRetrieveResponse, "agent_retrieve_response"),
    (AgentCreateResponse, "agent_create_response"),
    (AgentUpdateResponse, "agent_update_response"),
    (AgentDeleteResponse, "agent_delete_response"),
    (ConversationListResponse, "conversation_list_response"),
    (ConversationRetrieveResponse, "conversation_retrieve_response"),
    (ConversationCreateResponse, "conversation_create_response"),
    (ConversationUpdateResponse, "conversation_update_response"),
    (ConversationRecompileResponse, "conversation_recompile_response"),
    (ConversationForkResponse, "conversation_fork_response"),
    (ConversationMessagesListResponse, "conversation_messages_list_response"),
    (ConversationCompactResponse, "conversation_compact_response"),
    (RuntimeStartResponse, "runtime_start_response"),
    (GetExperimentsResponse, "get_experiments_response"),
    (SetExperimentResponse, "set_experiment_response"),
    (GetReflectionSettingsResponse, "get_reflection_settings_response"),
    (SetReflectionSettingsResponse, "set_reflection_settings_response"),
    (ChannelsListResponse, "channels_list_response"),
    (ChannelAccountsListResponse, "channel_accounts_list_response"),
    (ChannelAccountCreateResponse, "channel_account_create_response"),
    (ChannelAccountUpdateResponse, "channel_account_update_response"),
    (ChannelAccountBindResponse, "channel_account_bind_response"),
    (ChannelAccountUnbindResponse, "channel_account_unbind_response"),
    (ChannelAccountDeleteResponse, "channel_account_delete_response"),
    (ChannelAccountStartResponse, "channel_account_start_response"),
    (ChannelAccountStopResponse, "channel_account_stop_response"),
    (ChannelGetConfigResponse, "channel_get_config_response"),
    (ChannelSetConfigResponse, "channel_set_config_response"),
    (ChannelStartResponse, "channel_start_response"),
    (ChannelStopResponse, "channel_stop_response"),
    (ChannelPairingsListResponse, "channel_pairings_list_response"),
    (ChannelPairingBindResponse, "channel_pairing_bind_response"),
    (ChannelRoutesListResponse, "channel_routes_list_response"),
    (ChannelTargetsListResponse, "channel_targets_list_response"),
    (ChannelTargetBindResponse, "channel_target_bind_response"),
    (ChannelRouteRemoveResponse, "channel_route_remove_response"),
    (ChannelRouteUpdateResponse, "channel_route_update_response"),
    (ChannelsUpdated, "channels_updated"),
    (ChannelAccountsUpdated, "channel_accounts_updated"),
    (ChannelPairingsUpdated, "channel_pairings_updated"),
    (ChannelRoutesUpdated, "channel_routes_updated"),
    (ChannelTargetsUpdated, "channel_targets_updated"),
    (GetCwdMapResponse, "get_cwd_map_response"),
    (SearchBranchesResponse, "search_branches_response"),
    (CheckoutBranchResponse, "checkout_branch_response"),
    (SecretListResponse, "secret_list_response"),
    (SecretApplyResponse, "secret_apply_response"),
    (RemoveQueueItemResponse, "remove_queue_item_response"),
}
