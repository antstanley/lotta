//! Object-safe effect boundaries implemented by adapters.

mod ids;
mod memfs;
mod process;
mod store;
mod transcript;

pub use ids::IdGenerator;
pub use lotta_domain::Clock;
pub use memfs::{MemFsHistoryEntry, MemFsPort, MemFsStatus, MemFsTreeEntry};
pub use process::{ChildProcessPort, ProcessEvent, ProcessOutcome, ProcessRequest, SandboxPort};
pub use store::{AgentStore, ConversationStore};
pub use transcript::{TranscriptItem, TranscriptStore};

use std::{future::Future, pin::Pin};

/// Object-safe future returned by port methods.
pub type PortFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, crate::RuntimeError>> + Send + 'a>>;

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
        fn sandbox(_: Arc<dyn SandboxPort>) {}
        fn child(_: Arc<dyn ChildProcessPort>) {}
        let _ = (id, agent, conversation, transcript, memfs, sandbox, child);
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
        assert_eq!(lotta_domain::bounds::RESOURCE_BOUNDS.len(), 9);
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
