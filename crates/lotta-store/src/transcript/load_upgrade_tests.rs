use crate::transcript::task25_test_support::{corpus, directory_names, ids, json_lines, snapshot};
use lotta_domain::{ProviderStack, TranscriptManifest, TranscriptMessageFormat};
use sha2::{Digest as _, Sha256};

struct FailRewriteParentSync;

impl super::super::TranscriptRewriteObserver for FailRewriteParentSync {
    fn before_parent_sync(
        &self,
        parent: &std::path::Path,
        _temp: &std::path::Path,
    ) -> Result<(), crate::StoreError> {
        Err(crate::StoreError::new(crate::StoreErrorKind::Io, parent))
    }
}

fn assert_current_manifest(manifest: TranscriptManifest) -> String {
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(
        manifest.message_format,
        TranscriptMessageFormat::PiSessionEntryJsonl
    );
    assert_eq!(manifest.provider_stack, ProviderStack::PiAi);
    assert_eq!(
        manifest.migrated_from.as_deref(),
        Some("versioned-pi-ai-message-jsonl")
    );
    manifest.backup_path.expect("backup path")
}

#[tokio::test]
async fn empty_persistence_is_byte_identical_noop() {
    let corpus = corpus("versioned_legacy_transcript", "", "task25-upgrade-empty");
    std::fs::write(corpus.messages(), b"").expect("empty legacy transcript");
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("empty legacy load");
    assert!(loaded.messages().is_empty());
    let before = snapshot(corpus.root.path());
    corpus
        .store
        .persist_loaded_projection(&corpus.agent, &corpus.conversation, loaded)
        .await
        .expect("empty persistence");
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn nonempty_persistence_backs_up_and_upgrades() {
    let corpus = corpus("versioned_legacy_transcript", "", "task25-upgrade");
    let original = std::fs::read(corpus.messages()).expect("original messages");
    let original_hash: [u8; 32] = Sha256::digest(&original).into();
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("legacy load");
    assert_eq!(snapshot(corpus.root.path()), before);
    corpus
        .store
        .persist_loaded_projection(&corpus.agent, &corpus.conversation, loaded)
        .await
        .expect("legacy upgrade");

    let manifest = corpus
        .store
        .read_transcript_manifest(&corpus.agent, &corpus.conversation)
        .await
        .expect("current manifest");
    let backup_name = assert_current_manifest(manifest);
    assert!(backup_name.starts_with("messages.jsonl.lotta-upgrade-20000101-000000-"));
    assert!(!backup_name.contains('/'));
    let backup = corpus.directory.join(&backup_name);
    assert!(
        std::fs::symlink_metadata(&backup)
            .expect("backup metadata")
            .file_type()
            .is_file()
    );
    let backup_bytes = std::fs::read(&backup).expect("backup bytes");
    assert_eq!(backup_bytes, original);
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(&backup_bytes)),
        original_hash
    );

    let current = std::fs::read(corpus.messages()).expect("current messages");
    assert_ne!(current, original);
    let rows = json_lines(&corpus.messages());
    assert_eq!(rows[0]["type"], "session");
    assert_eq!(rows[0]["version"], 3);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["type"], "message");
    assert_eq!(rows[1]["message"]["id"], "msg-legacy-fixture");
    assert!(directory_names(&corpus.directory).iter().all(|name| {
        !std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
    }));
    let reloaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("current reload");
    assert_eq!(ids(&reloaded), ["msg-legacy-fixture"]);
    assert_eq!(
        reloaded.conversation().in_context_message_ids.as_slice()[0].as_str(),
        "msg-legacy-fixture"
    );
}

#[tokio::test]
async fn manifest_conflict_keeps_external_bytes_and_recovery_backup() {
    let corpus = corpus("versioned_legacy_transcript", "", "task25-upgrade-conflict");
    let original = std::fs::read(corpus.messages()).expect("original messages");
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("legacy load");
    let mut external: serde_json::Value =
        serde_json::from_slice(&std::fs::read(corpus.manifest()).expect("manifest"))
            .expect("manifest JSON");
    external["created_at"] = "2001-01-01T00:00:00.000Z".into();
    let external = serde_json::to_vec_pretty(&external).expect("external manifest");
    let expected = external.clone();
    let error = corpus
        .store
        .persist_loaded_projection_observed(
            &corpus.agent,
            &corpus.conversation,
            loaded,
            move |path| {
                std::fs::write(path, &external).map_err(|io| crate::StoreError::from_io(path, &io))
            },
        )
        .await
        .expect_err("manifest conflict");
    assert_eq!(error.kind(), crate::StoreErrorKind::StorageConflict);
    assert_eq!(
        std::fs::read(corpus.manifest()).expect("external survives"),
        expected
    );
    assert_eq!(
        std::fs::read(corpus.messages()).expect("restored legacy messages"),
        original
    );
    let backups = directory_names(&corpus.directory)
        .into_iter()
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bak"))
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read(corpus.directory.join(&backups[0])).expect("recovery backup"),
        original
    );
    let reloaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("consistent legacy reload");
    assert_eq!(ids(&reloaded), ["msg-legacy-fixture"]);
}

#[tokio::test]
async fn rewrite_postcommit_failure_restores_legacy_pair_and_keeps_backup() {
    let corpus = corpus(
        "versioned_legacy_transcript",
        "",
        "task25-upgrade-rewrite-fail",
    );
    let original_messages = std::fs::read(corpus.messages()).expect("messages");
    let original_manifest = std::fs::read(corpus.manifest()).expect("manifest");
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("legacy load");
    let error = corpus
        .store
        .persist_loaded_projection_rewrite_observed(
            &corpus.agent,
            &corpus.conversation,
            loaded,
            FailRewriteParentSync,
        )
        .await
        .expect_err("postcommit parent sync failure");
    assert_eq!(error.kind(), crate::StoreErrorKind::Io);
    assert_eq!(
        std::fs::read(corpus.messages()).expect("restored"),
        original_messages
    );
    assert_eq!(
        std::fs::read(corpus.manifest()).expect("unchanged"),
        original_manifest
    );
    let backups = directory_names(&corpus.directory)
        .into_iter()
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "bak")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read(corpus.directory.join(&backups[0])).expect("backup"),
        original_messages
    );
    assert!(
        directory_names(&corpus.directory)
            .iter()
            .all(|name| !name.contains(".tmp"))
    );
    let reloaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("consistent legacy reload");
    assert_eq!(ids(&reloaded), ["msg-legacy-fixture"]);
}
