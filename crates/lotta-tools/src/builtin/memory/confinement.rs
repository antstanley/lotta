use super::patch;
use lotta_runtime::boundary::RepositoryPath;

#[path = "execution_evidence.rs"]
mod execution_evidence;

#[test]
fn traversal_absolute_and_non_markdown_paths_are_rejected() {
    for value in [
        "../peer.md",
        "/tmp/peer.md",
        ".",
        "..",
        "system/file.txt",
        "system/\0.md",
    ] {
        assert!(patch::memory_path(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn patch_move_source_and_target_are_both_confined() {
    for patch in [
        "*** Begin Patch\n*** Update File: ../peer.md\n@@\n-old\n+new\n*** End Patch",
        "*** Begin Patch\n*** Update File: system/x.md\n*** Move to: /tmp/x.md\n@@\n-old\n+new\n*** End Patch",
    ] {
        assert!(patch::parse(patch).is_err());
    }
}

#[test]
fn repository_path_rejects_lexical_escape_before_port_use() {
    assert!(RepositoryPath::new("../peer.md".into()).is_err());
    assert!(RepositoryPath::new("/tmp/peer.md".into()).is_err());
}
