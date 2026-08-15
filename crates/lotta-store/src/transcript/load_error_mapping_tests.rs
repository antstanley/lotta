use crate::StoreErrorKind;
use crate::transcript::task25_test_support::{corpus, snapshot};

#[tokio::test]
async fn unversioned_nonempty_requires_migration() {
    let corpus = corpus("unversioned_legacy_transcript", "", "task25-unversioned");
    let before = snapshot(corpus.root.path());
    let error = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect_err("migration required");
    assert_eq!(error.kind(), StoreErrorKind::TranscriptMigrationRequired);
    assert_eq!(error.kind().code(), "transcript_migration_required");
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn versioned_legacy_ui_requires_repair() {
    let corpus = corpus("current_typescript_state", "", "task25-versioned-ui");
    let row = serde_json::json!({
        "type":"message", "id":"entry-ui", "parentId":null,
        "timestamp":"2000-01-01T00:00:00.000Z",
        "message":{"id":"ui-msg-1", "role":"user", "parts":[], "content":null}
    });
    std::fs::write(corpus.messages(), format!("{row}\n")).expect("legacy UI row");
    let before = snapshot(corpus.root.path());
    let error = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect_err("repair required");
    assert_eq!(error.kind(), StoreErrorKind::TranscriptRepairRequired);
    assert_eq!(error.kind().code(), "transcript_repair_required");
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn empty_unversioned_is_empty_boundary() {
    let corpus = corpus(
        "unversioned_legacy_transcript",
        "",
        "task25-empty-unversioned",
    );
    std::fs::write(corpus.messages(), b" \n\t").expect("empty transcript");
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("empty unversioned load");
    assert!(loaded.manifest().is_none());
    assert!(loaded.messages().is_empty());
    assert_eq!(snapshot(corpus.root.path()), before);
}
