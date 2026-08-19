//! Object-safe effect boundaries implemented by adapters.

mod ids;
mod memfs;
mod model;
mod process;
#[path = "provider.rs"]
mod provider_contract;
mod provider_event;
mod store;
mod tool;
mod transcript;

pub use ids::IdGenerator;
pub use lotta_domain::Clock;
pub use memfs::{
    MemFsCommitAuthor, MemFsHistoryEntry, MemFsMutation, MemFsPort, MemFsStatus,
    MemFsTransactionResult, MemFsTreeEntry,
};
pub use model::{ModelCapabilityPort, ModelCapabilityRequest, ModelCapabilityResponse};
pub use process::{
    ChildProcessPort, InteractiveSandboxPort, PROCESS_TIMEOUT_DISABLED, ProcessEvent, ProcessInput,
    ProcessOutcome, ProcessRequest, ProcessSession, ProcessSessionFuture, ProcessSessionParts,
    SandboxPort,
};
pub use provider_contract::{
    ImagePolicy, ProviderContent, ProviderContentPart, ProviderContext, ProviderContextDecision,
    ProviderContextOverflowDetail, ProviderContextTokenCount, ProviderContextTokenProvenance,
    ProviderDeadline, ProviderError, ProviderErrorContext, ProviderEventReceiver,
    ProviderEventSink, ProviderMessage, ProviderMessageRole, ProviderMessages, ProviderMetadata,
    ProviderMetadataInput, ProviderPort, ProviderRequest, ProviderToolChoice,
    ProviderToolDefinition, ProviderTools, ReasoningControls, StopReason, TokenLimit,
    ToolArgumentBuffer, ToolCallAccumulator, ToolCallId, estimate_request_tokens,
    provider_event_channel, provider_timeout_default, validate_provider_image_bytes,
    validate_provider_request_bytes,
};
pub use provider_event::{ProviderEvent, ProviderUsage};
pub use store::{AgentStore, ConversationStore};
pub use tool::{
    InternalToolName, ModelFacingToolName, ParallelCertificationId, ParallelSafety,
    PermissionAction, SecretFieldPath, SecretRedactionPolicy, SecretRedactionSpec,
    ToolApprovalGrant, ToolApprovalPolicy, ToolDefinition, ToolDescriptionAsset,
    ToolExecutionOwner, ToolExecutionRequest, ToolInputSchema, ToolOutcome, ToolOutcomeCode,
    ToolOutcomeMessage, ToolOutputLimit, ToolPort, ToolResultText, ToolTimeout, ValidatedToolInput,
};
pub use transcript::{TranscriptItem, TranscriptStore};

use std::{future::Future, pin::Pin};

/// Object-safe future returned by port methods.
pub type PortFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, crate::RuntimeError>> + Send + 'a>>;

#[cfg(test)]
mod provider_event_variants {
    #[test]
    fn exact_ten_variants() {
        super::provider_contract::contract::provider_event_variants();
    }
}
#[cfg(test)]
mod model_descriptor_is_domain_type {
    #[test]
    fn provider_contract() {
        super::provider_contract::contract::model_descriptor_is_domain_type();
    }
}
#[cfg(test)]
mod streaming_invariants {
    #[test]
    fn text_reasoning_arrival_order_preserved_provider() {
        super::provider_contract::contract::streaming_invariants::
            text_reasoning_arrival_order_preserved();
    }
    #[test]
    fn stable_tool_call_id_across_start_deltas_end_provider() {
        super::provider_contract::contract::streaming_invariants::
            stable_tool_call_id_across_start_deltas_end();
    }
    #[test]
    fn partial_json_bounded_incrementally_and_parsed_only_at_end_provider() {
        super::provider_contract::contract::streaming_invariants::
            partial_json_bounded_incrementally_and_parsed_only_at_end();
    }
    #[test]
    fn usage_monotonic_and_final_snapshot_retained_provider() {
        super::provider_contract::contract::streaming_invariants::
            usage_monotonic_and_final_snapshot_retained();
    }
    #[test]
    fn exactly_one_stop_provider() {
        super::provider_contract::contract::streaming_invariants::exactly_one_stop();
    }
    #[test]
    fn cancellation_closes_and_suppresses_late_adapter_sends_provider() {
        super::provider_contract::contract::streaming_invariants::
            cancellation_closes_and_suppresses_late_adapter_sends();
    }
    #[test]
    fn strict_drop_image_policy_behavior_provider() {
        super::provider_contract::contract::streaming_invariants::strict_drop_image_policy_behavior(
        );
    }
    #[test]
    fn metadata_rejects_secrets_retaining_continuation_provider() {
        super::provider_contract::contract::streaming_invariants::
            persisted_provider_metadata_rejects_secrets_retaining_continuation();
    }
}

#[cfg(test)]
mod provider {
    use super::provider_contract::contract;

    #[test]
    fn event_exact_ten() {
        contract::provider_event_variants();
    }
    #[test]
    fn error_exact_twelve() {
        contract::provider_error_variants();
    }
    #[test]
    fn model_is_domain_descriptor() {
        contract::model_descriptor_is_domain_type();
    }
    #[test]
    fn request_has_exact_fields() {
        contract::provider_request_fields_have_exact_types();
    }
    #[test]
    fn request_wire_is_bounded() {
        contract::provider_request_aggregate_bytes_are_bounded();
    }
    #[test]
    fn async_channel_backpressure_and_cancellation() {
        contract::async_channel_backpressure_and_cancellation();
    }
    #[test]
    fn text_reasoning_arrival_order_preserved() {
        contract::streaming_invariants::text_reasoning_arrival_order_preserved();
    }
    #[test]
    fn stable_tool_call_id_across_start_deltas_end() {
        contract::streaming_invariants::stable_tool_call_id_across_start_deltas_end();
    }
    #[test]
    fn partial_json_bounded_incrementally_and_parsed_only_at_end() {
        contract::streaming_invariants::partial_json_bounded_incrementally_and_parsed_only_at_end();
    }
    #[test]
    fn usage_monotonic_and_final_snapshot_retained() {
        contract::streaming_invariants::usage_monotonic_and_final_snapshot_retained();
    }
    #[test]
    fn exactly_one_stop() {
        contract::streaming_invariants::exactly_one_stop();
    }
    #[test]
    fn cancellation_closes_and_suppresses_late_adapter_sends() {
        contract::streaming_invariants::cancellation_closes_and_suppresses_late_adapter_sends();
    }
    #[test]
    fn strict_drop_image_policy_behavior() {
        contract::streaming_invariants::strict_drop_image_policy_behavior();
    }
    #[test]
    fn metadata_rejects_secrets_retaining_continuation() {
        contract::streaming_invariants::
            persisted_provider_metadata_rejects_secrets_retaining_continuation();
    }

    #[test]
    fn port_is_object_safe_and_channel_is_opaque() {
        fn object_safe(_: std::sync::Arc<dyn super::ProviderPort>) {}
        let _ = object_safe;
        let request = contract::request_for_structural_test();
        let (_sink, _receiver): (super::ProviderEventSink, super::ProviderEventReceiver) =
            super::provider_event_channel(1, &request.cancellation).unwrap();
    }
}

#[cfg(test)]
mod tool_definition_fields {
    #[test]
    fn structural_eleven_field_case() {
        super::tool::tests::tool_definition_fields();
    }
}
#[cfg(test)]
mod parallel_safety_defaults_sequential {
    #[test]
    fn normal_constructor_is_sequential() {
        super::tool::tests::parallel_safety_defaults_sequential();
    }
}
#[cfg(test)]
mod execution_owner {
    #[test]
    fn exact_five_variants() {
        super::tool::tests::execution_owner();
    }
}
#[cfg(test)]
mod tool_outcome_round_trip {
    use super::tool::tests;
    #[test]
    fn success() {
        tests::outcome_success();
    }
    #[test]
    fn user_denied() {
        tests::outcome_user_denied();
    }
    #[test]
    fn interrupted() {
        tests::outcome_interrupted();
    }
    #[test]
    fn timeout() {
        tests::outcome_timeout();
    }
    #[test]
    fn validation_failure() {
        tests::outcome_validation_failure();
    }
    #[test]
    fn sandbox_denied() {
        tests::outcome_sandbox_denied();
    }
    #[test]
    fn spawn_failure() {
        tests::outcome_spawn_failure();
    }
    #[test]
    fn tool_defined_error() {
        tests::outcome_tool_defined_error();
    }
}
#[cfg(test)]
mod tool_contract {
    use super::tool::tests;
    #[test]
    fn contract() {
        tests::broad_contract();
    }
    #[test]
    fn object_safe_external_implementation() {
        tests::object_safe_external_implementation();
    }
    #[test]
    fn input_output_deadline_boundaries() {
        tests::input_output_deadline_boundaries();
    }
    #[test]
    fn schema_shape_and_malicious_serde() {
        tests::schema_shape_and_malicious_serde();
    }
}

#[cfg(test)]
mod dependency_direction {
    #[test]
    fn live_and_mutated_dependency_enforcement() {
        super::tests::parsed_runtime_dependencies_are_allowed();
        super::tests::dependency_assertion_rejects_each_forbidden_fixture();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lotta_domain::{DomainError, Timestamp};
    use std::{fs, path::PathBuf, process::Command, sync::Arc};

    const FORBIDDEN_ADAPTER_CRATES: [&str; 7] = [
        "lotta-store",
        "lotta-memfs",
        "lotta-providers",
        "lotta-tools",
        "lotta-extensions",
        "lotta-channels",
        "lotta-app-server",
    ];

    fn assert_dependencies_allowed(dependencies: &serde_json::Value) {
        let dependencies = dependencies.as_array().expect("dependencies array");
        for forbidden in FORBIDDEN_ADAPTER_CRATES {
            assert!(
                dependencies
                    .iter()
                    .all(|dependency| dependency["name"] != forbidden),
                "lotta-runtime must not depend on {forbidden}"
            );
        }
    }

    #[test]
    pub(super) fn dependency_assertion_rejects_each_forbidden_fixture() {
        for forbidden in FORBIDDEN_ADAPTER_CRATES {
            let fixture = serde_json::json!([{ "name": forbidden }]);
            assert!(std::panic::catch_unwind(|| assert_dependencies_allowed(&fixture)).is_err());
        }
    }

    #[test]
    pub(super) fn parsed_runtime_dependencies_are_allowed() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let output = Command::new(env!("CARGO"))
            .args([
                "metadata",
                "--format-version=1",
                "--no-deps",
                "--manifest-path",
            ])
            .arg(manifest)
            .output()
            .expect("cargo metadata must run");
        assert!(output.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
        let runtime = metadata["packages"]
            .as_array()
            .expect("packages")
            .iter()
            .find(|package| package["name"] == "lotta-runtime")
            .expect("runtime");
        assert_dependencies_allowed(&runtime["dependencies"]);
    }

    #[test]
    fn signatures_are_object_safe() {
        fn id(_: Arc<dyn IdGenerator>) {}
        fn agent(_: Arc<dyn AgentStore>) {}
        fn conversation(_: Arc<dyn ConversationStore>) {}
        fn transcript(_: Arc<dyn TranscriptStore>) {}
        fn memfs(_: Arc<dyn MemFsPort>) {}
        fn model(_: Arc<dyn ModelCapabilityPort>) {}
        fn sandbox(_: Arc<dyn SandboxPort>) {}
        fn child(_: Arc<dyn ChildProcessPort>) {}
        fn provider(_: Arc<dyn ProviderPort>) {}
        let _ = (
            id,
            agent,
            conversation,
            transcript,
            memfs,
            model,
            sandbox,
            child,
            provider,
        );
    }

    #[test]
    fn external_clock_uses_only_public_timestamp_api() {
        struct FakeClock;
        impl Clock for FakeClock {
            fn now(&self) -> Timestamp {
                Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56+00:00")
                    .expect("public parser")
            }
            fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
                Timestamp::parse_persisted_rfc3339(value)
            }
        }
        assert_eq!(
            Timestamp::now(&FakeClock).to_string(),
            "2026-08-14T12:34:56Z"
        );
    }

    #[test]
    fn runtime_bounds_do_not_leak_into_domain() {
        let domain_bounds = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lotta-domain/src/bounds.rs"),
        )
        .expect("domain bounds");
        for prefix in [
            "MEMORY_",
            "PROCESS_",
            "COMMIT_MESSAGE_",
            "REVISION_ID_",
            "WORKTREE_ID_",
            "MEMFS_DIFF_",
            "REPOSITORY_PATH_",
            "CONFINED_PATH_",
        ] {
            assert!(
                !domain_bounds.contains(prefix),
                "runtime bound leaked into domain: {prefix}"
            );
        }
        assert_eq!(lotta_domain::bounds::RESOURCE_BOUNDS.len(), 11);
        assert_eq!(crate::bounds::RUNTIME_RESOURCE_BOUNDS.len(), 19);
    }

    #[test]
    fn source_enforces_signature_and_effect_boundaries() {
        let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut production = String::new();
        for file in [
            "ports/store.rs",
            "ports/transcript.rs",
            "ports/memfs.rs",
            "ports/process.rs",
        ] {
            let source = fs::read_to_string(source_root.join(file)).expect("port source");
            production.push_str(source.split("#[cfg(test)]").next().unwrap_or(&source));
        }
        for forbidden in [
            " Vec<",
            "BTreeMap<",
            "PathBuf",
            "&Path",
            "String>",
            "Vec<u8>",
            "unbounded_channel",
            "snapshot(",
            "apply(",
            "pushed",
        ] {
            assert!(
                !production.contains(forbidden),
                "forbidden port boundary: {forbidden}"
            );
        }
        for method in ["fn list(", "fn list_for_agent(", "fn tree(", "fn history("] {
            let start = production.find(method).expect("stream method");
            let signature = &production[start
                ..production[start..]
                    .find(';')
                    .map_or(production.len(), |end| start + end)];
            assert!(signature.contains("Sender<") && signature.contains("CancellationToken"));
        }
        let transcript =
            fs::read_to_string(source_root.join("ports/transcript.rs")).expect("transcript source");
        let load = &transcript[transcript.find("fn load(").expect("load")..];
        let load = &load[..load.find(';').expect("load end")];
        assert!(load.contains("Sender<") && load.contains("CancellationToken"));
        let memfs = fs::read_to_string(source_root.join("ports/memfs.rs")).expect("memfs");
        for operation in [
            "initialize",
            "status",
            "tree",
            "read",
            "write",
            "delete",
            "rename",
            "history",
            "file_at_revision",
            "diff",
            "commit",
            "create_worktree",
            "merge_worktree",
        ] {
            assert!(
                memfs.contains(&format!("fn {operation}(")),
                "missing {operation}"
            );
        }
        let root = fs::read_to_string(source_root.join("lib.rs")).expect("root");
        for effect in ["std::fs", "std::process", "SystemTime::now", "rand::"] {
            assert!(!root.contains(effect));
        }
    }
}
