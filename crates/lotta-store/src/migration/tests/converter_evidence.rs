use super::support::*;
use super::*;

#[test]
fn versioned_whitespace_and_unterminated_final_row_reload() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let path = directory.join("messages.jsonl");
    let source = bytes(&path);
    let mut changed = b" \t\n".to_vec();
    changed.extend_from_slice(source.strip_suffix(b"\n").expect("fixture LF"));
    std::fs::write(&path, changed).expect("whitespace corpus");
    let report = migrate_transcripts(root.path(), false).expect("migration");
    assert_eq!(report.items[0].message_count, 1);
    let store = crate::LocalStore::new(StorePaths::new(root.path()).expect("paths"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let loaded = runtime
        .block_on(store.load_transcript(
            &lotta_domain::AgentId::accept("agent-local-fixture").expect("agent"),
            &lotta_domain::ConversationId::default_for_agent(),
        ))
        .expect("public reload");
    assert_eq!(loaded.messages()[0].id.as_str(), "msg-legacy-fixture");
}

#[test]
fn whitespace_unversioned_real_empty_and_dry_run_exact() {
    let root = copy_fixture("persistence/unversioned_legacy_transcript");
    let directory = conversation(root.path());
    let path = directory.join("messages.jsonl");
    std::fs::write(&path, b" \t\n\r\n").expect("whitespace");
    let before = snapshot(root.path());
    let dry = migrate_transcripts(root.path(), true).expect("dry run");
    assert_eq!(dry.items[0].message_count, 0);
    assert_eq!(snapshot(root.path()), before);
    let real = migrate_transcripts(root.path(), false).expect("real run");
    assert_eq!(real.items[0].message_count, 0);
    assert!(directory.join("manifest.json").exists());
}
