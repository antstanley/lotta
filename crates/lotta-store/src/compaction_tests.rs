use crate::{
    CompactionClaim, CompactionProjection, CompactionTransactionState, LocalStore, StoreErrorKind,
    StorePaths,
};
use lotta_domain::{
    AgentId, BoundedJsonValue, CompactionEntry, CompactionEntryType, ConversationId, LocalMessage,
    LocalMessageRole, MessageId, NonEmptyString, ProviderStack, RuntimeScope, SessionEntry,
    SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest, TranscriptMessageFormat,
};
use std::sync::{Arc, Barrier};

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "lotta-compaction-{name}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn scope(name: &str) -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(format!("agent-{name}")).unwrap(),
        ConversationId::accept(format!("conversation-{name}")).unwrap(),
        None,
    )
}

fn store(root: &std::path::Path) -> LocalStore {
    LocalStore::new(StorePaths::new(root.to_path_buf()).unwrap())
}

fn request() -> NonEmptyString {
    NonEmptyString::new("request-1").unwrap()
}

fn projection() -> CompactionProjection {
    CompactionProjection {
        summary: "safe summary".into(),
        retained_message_ids: Vec::new(),
        tokens_before: 100,
        tokens_after: 20,
        messages_before: 4,
        messages_after: 1,
    }
}

async fn initialized(name: &str) -> (std::path::PathBuf, LocalStore, RuntimeScope) {
    let root = root(name);
    let store = store(&root);
    let scope = scope(name);
    let now = Timestamp::from_utc(chrono::Utc::now());
    let manifest = TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: now,
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    };
    let session = TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        id: NonEmptyString::new("session-1").unwrap(),
        version: 3,
        timestamp: now,
        cwd: "/tmp".into(),
    });
    let directory = store
        .paths()
        .conversation_dir(&scope.agent_id, &scope.conversation_id)
        .unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    store
        .initialize_transcript(&scope.agent_id, &scope.conversation_id, &manifest, &session)
        .await
        .unwrap();
    (root, store, scope)
}

fn entry(id: &NonEmptyString) -> TranscriptEntry {
    let now = Timestamp::from_utc(chrono::Utc::now());
    TranscriptEntry::Compaction(CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: id.clone(),
        parent_id: None,
        timestamp: now,
        summary: "safe summary".into(),
        first_kept_entry_id: None,
        tokens_before: 100,
        tokens_after: Some(20),
        messages_before: Some(4),
        messages_after: Some(1),
        message: LocalMessage {
            id: MessageId::accept(format!("compact-{}", id.as_str())).unwrap(),
            role: LocalMessageRole::User,
            content: Some(BoundedJsonValue::new(serde_json::json!("safe summary")).unwrap()),
            timestamp: 1_776_214_923_456.0,
            metadata: None,
            extras: lotta_domain::EntityExtras::default(),
        },
        details: None,
    })
}

mod compaction {
    use super::*;

    mod durability {
        use super::*;

        #[test]
        fn concurrent_100_same_request_has_one_owner_record() {
            let root = root("concurrent");
            let scope = scope("concurrent");
            let request = request();
            let barrier = Arc::new(Barrier::new(100));
            let threads = (0..100)
                .map(|_| {
                    let root = root.clone();
                    let scope = scope.clone();
                    let request = request.clone();
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        store(&root).claim_compaction(&scope, &request)
                    })
                })
                .collect::<Vec<_>>();
            for thread in threads {
                let (claim, row) = thread.join().unwrap().unwrap();
                assert_eq!(claim, CompactionClaim::Owned);
                assert_eq!(row.request_id, request);
            }
            let bytes = std::fs::read(root.join("runtime/compactions.json")).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["transactions"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn crash_before_append_is_retryable_after_restart() {
            let root = root("before-append");
            let scope = scope("before-append");
            let request = request();
            store(&root).claim_compaction(&scope, &request).unwrap();
            let (claim, recovered) = store(&root).claim_compaction(&scope, &request).unwrap();
            assert_eq!(claim, CompactionClaim::Owned);
            assert_eq!(recovered.state, CompactionTransactionState::Pending);
        }

        #[tokio::test]
        async fn crash_after_append_recovers_one_entry() {
            let (root, first, scope) = initialized("after-append").await;
            let request = request();
            let (_, pending) = first.claim_compaction(&scope, &request).unwrap();
            let planned = first
                .record_compaction_projection(pending.revision, &scope, &request, projection())
                .unwrap();
            first
                .append_compaction_if_absent(planned.revision, &scope, &request, &entry(&request))
                .await
                .unwrap();
            let restarted = store(&root);
            let (_, appended) = restarted.claim_compaction(&scope, &request).unwrap();
            assert!(matches!(
                appended.state,
                CompactionTransactionState::Appended { .. }
            ));
            restarted
                .append_compaction_if_absent(appended.revision, &scope, &request, &entry(&request))
                .await
                .unwrap();
            let path = restarted
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap()
                .join("messages.jsonl");
            assert_eq!(
                std::fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .filter(|line| line.contains("\"type\":\"compaction\""))
                    .count(),
                1
            );
        }

        #[test]
        fn publish_failure_retries_from_appended() {
            let root = root("publish-retry");
            let scope = scope("publish-retry");
            let request = request();
            let first = store(&root);
            let (_, pending) = first.claim_compaction(&scope, &request).unwrap();
            let planned = first
                .record_compaction_projection(pending.revision, &scope, &request, projection())
                .unwrap();
            let appended = first
                .mark_compaction_appended(planned.revision, &scope, &request, request.clone())
                .unwrap();
            let restarted = store(&root);
            assert!(matches!(
                restarted
                    .claim_compaction(&scope, &request)
                    .unwrap()
                    .1
                    .state,
                CompactionTransactionState::Appended { .. }
            ));
            restarted
                .mark_compaction_published(appended.revision, &scope, &request)
                .unwrap();
            assert_eq!(
                restarted.claim_compaction(&scope, &request).unwrap().0,
                CompactionClaim::Published
            );
        }

        #[test]
        fn scopes_are_isolated() {
            let root = root("scopes");
            let request = request();
            let store = store(&root);
            let (_, one) = store.claim_compaction(&scope("scope-a"), &request).unwrap();
            let (_, two) = store.claim_compaction(&scope("scope-b"), &request).unwrap();
            assert_ne!(one.scope, two.scope);
            let value: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("runtime/compactions.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(value["transactions"].as_array().unwrap().len(), 2);
        }

        #[cfg(unix)]
        #[test]
        fn security_and_limits_reject_symlink_permissive_and_oversize() {
            use std::os::unix::fs::{PermissionsExt as _, symlink};
            for (name, prepare, expected) in [
                ("symlink", 0_u8, StoreErrorKind::InvalidPath),
                ("permissive", 1_u8, StoreErrorKind::Parse),
                ("oversize", 2_u8, StoreErrorKind::Limit),
            ] {
                let root = root(name);
                let runtime = root.join("runtime");
                std::fs::create_dir_all(&runtime).unwrap();
                let path = runtime.join("compactions.json");
                match prepare {
                    0 => {
                        let target = root.join("target");
                        std::fs::write(&target, b"{}").unwrap();
                        symlink(target, &path).unwrap();
                    }
                    1 => {
                        std::fs::write(&path, b"{\"revision\":0,\"transactions\":[]}").unwrap();
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                            .unwrap();
                    }
                    _ => {
                        let file = std::fs::File::create(&path).unwrap();
                        file.set_len((crate::COMPACTION_JOURNAL_BYTES_MAX + 1) as u64)
                            .unwrap();
                    }
                }
                let error = store(&root)
                    .claim_compaction(&scope(name), &request())
                    .unwrap_err();
                assert_eq!(error.kind(), expected);
            }
        }
    }
}
