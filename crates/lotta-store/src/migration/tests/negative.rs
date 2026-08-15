use super::support::*;
use super::*;
use lotta_testkit::roots::TemporaryRoot;

#[test]
fn malformed_is_typed_and_unchanged() {
    let root = copy_fixture("persistence/unversioned_legacy_transcript");
    let path = conversation(root.path()).join("messages.jsonl");
    std::fs::write(&path, b"{bad\n").expect("malformed");
    let before = snapshot(root.path());
    let error = migrate_transcripts(root.path(), false).expect_err("parse error");
    assert_eq!(error.kind(), StoreErrorKind::Parse);
    assert_eq!(snapshot(root.path()), before);
}

#[test]
#[cfg(unix)]
fn symlink_conversation_is_not_followed() {
    use std::os::unix::fs::symlink;
    let root = TemporaryRoot::new("task26-symlink").expect("root");
    std::fs::create_dir_all(root.path().join("conversations")).expect("conversations");
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).expect("outside");
    symlink(&outside, root.path().join("conversations").join(KEY)).expect("symlink");
    let error = migrate_transcripts(root.path(), false).expect_err("invalid path");
    assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
}
