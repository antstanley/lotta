use super::{FileToolBundle, control::OperationControl, operations};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    root: PathBuf,
    artifacts: PathBuf,
}
impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("lotta-patch-{label}-{}", std::process::id()));
        let artifacts = root.join("artifacts");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&artifacts).unwrap_or_else(|error| panic!("fixture: {error}"));
        Self {
            root: root
                .canonicalize()
                .unwrap_or_else(|error| panic!("root: {error}")),
            artifacts: artifacts
                .canonicalize()
                .unwrap_or_else(|error| panic!("artifacts: {error}")),
        }
    }
    fn write(&self, path: &str, text: &str) {
        let full = self.root.join(path);
        fs::create_dir_all(full.parent().unwrap_or(Path::new("")))
            .unwrap_or_else(|error| panic!("parent: {error}"));
        fs::write(full, text).unwrap_or_else(|error| panic!("write: {error}"));
    }
    fn apply(&self, patch: &str) -> Result<String, super::fs::FileError> {
        let bundle =
            FileToolBundle::new(&self.root, &self.artifacts).unwrap_or_else(|_| panic!("bundle"));
        operations::execute(
            &bundle.state,
            "apply_patch",
            &json!({"input":patch}),
            &OperationControl::new(CancellationToken::new(), Duration::from_secs(30))?,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn add_delete_update_multiple_move_crlf_and_newlines() {
    let fixture = Fixture::new("semantics");
    fixture.write("delete.txt", "gone\n");
    fixture.write("edit.txt", "one\ntwo\nthree");
    fixture.write("old.txt", "old\n");
    let patch = concat!(
        "*** Begin Patch\r\n*** Add File: empty.txt\r\n",
        "*** Add File: add.txt\r\n+hello\r\n*** Delete File: delete.txt\r\n",
        "*** Update File: edit.txt\r\n@@\r\n-one\r\n+ONE\r\n",
        "@@\r\n-three\r\n+THREE\r\n*** End of File\r\n",
        "*** Update File: old.txt\r\n*** Move to: moved.txt\r\n",
        "@@\r\n-old\r\n+new\r\n*** End Patch",
    );
    assert_eq!(fixture.apply(patch), Ok("Done!".into()));
    assert_eq!(
        fs::read(fixture.root.join("empty.txt")).unwrap_or_default(),
        b""
    );
    assert_eq!(
        fs::read(fixture.root.join("add.txt")).unwrap_or_default(),
        b"hello\n"
    );
    assert!(!fixture.root.join("delete.txt").exists());
    assert_eq!(
        fs::read(fixture.root.join("edit.txt")).unwrap_or_default(),
        b"ONE\ntwo\nTHREE"
    );
    assert!(!fixture.root.join("old.txt").exists());
    assert_eq!(
        fs::read(fixture.root.join("moved.txt")).unwrap_or_default(),
        b"new\n"
    );
}

#[test]
fn context_header_missing_ambiguous_and_malformed_reject() {
    let fixture = Fixture::new("matching");
    fixture.write("file.txt", "function a\nold\nfunction b\nold\n");
    let good =
        "*** Begin Patch\n*** Update File: file.txt\n@@ function b\n-old\n+new\n*** End Patch";
    let result = fixture.apply(good);
    assert!(result.is_ok(), "{result:?}");
    let malformed = "*** Begin Patch\n*** Add File: bad.txt\nnot-plus\n*** End Patch";
    assert!(fixture.apply(malformed).is_err());
    fixture.write("repeat.txt", "old\nold\n");
    let repeated = "*** Begin Patch\n*** Update File: repeat.txt\n@@\n-old\n+new\n*** End Patch";
    assert!(fixture.apply(repeated).is_ok());
    assert_eq!(
        fs::read(fixture.root.join("repeat.txt")).unwrap_or_default(),
        b"new\nold\n"
    );
    let missing = "*** Begin Patch\n*** Update File: repeat.txt\n@@\n-nope\n+new\n*** End Patch";
    assert!(fixture.apply(missing).is_err());
}

#[test]
fn conflicts_existing_and_missing_reject_without_effects() {
    let fixture = Fixture::new("preflight");
    fixture.write("exists.txt", "original\n");
    let overwrite = "*** Begin Patch\n*** Add File: exists.txt\n+new\n*** End Patch";
    assert_eq!(fixture.apply(overwrite), Ok("Done!".into()));
    assert_eq!(
        fs::read(fixture.root.join("exists.txt")).unwrap_or_default(),
        b"new\n"
    );
    fixture.write("exists.txt", "original\n");
    for patch in [
        "*** Begin Patch\n*** Delete File: missing.txt\n*** End Patch",
        "*** Begin Patch\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch",
        "*** Begin Patch\n*** Add File: same.txt\n+a\n*** Delete File: same.txt\n*** End Patch",
        concat!(
            "*** Begin Patch\n*** Update File: exists.txt\n",
            "*** Move to: target.txt\n@@\n-original\n+new\n",
            "*** Add File: target.txt\n+x\n*** End Patch",
        ),
    ] {
        assert!(fixture.apply(patch).is_err());
    }
    fixture.write("target.txt", "target\n");
    let move_existing = concat!(
        "*** Begin Patch\n*** Update File: exists.txt\n",
        "*** Move to: target.txt\n@@\n-original\n+new\n*** End Patch",
    );
    assert!(fixture.apply(move_existing).is_err());
    assert_eq!(
        fs::read(fixture.root.join("exists.txt")).unwrap_or_default(),
        b"original\n"
    );
}

#[test]
fn transaction_failure_restores_every_path_and_cleans_residue() {
    for failure in [1, 2] {
        let fixture = Fixture::new(&format!("rollback-{failure}"));
        fixture.write("one.txt", "one\n");
        fixture.write("two.txt", "two\n");
        super::patch::transaction::fail_commit_at(failure);
        let patch = concat!(
            "*** Begin Patch\n*** Update File: one.txt\n@@\n-one\n+ONE\n",
            "*** Update File: two.txt\n@@\n-two\n+TWO\n",
            "*** Add File: add.txt\n+ADD\n*** End Patch",
        );
        assert!(fixture.apply(patch).is_err());
        assert_eq!(
            fs::read(fixture.root.join("one.txt")).unwrap_or_default(),
            b"one\n"
        );
        assert_eq!(
            fs::read(fixture.root.join("two.txt")).unwrap_or_default(),
            b"two\n"
        );
        assert!(!fixture.root.join("add.txt").exists());
        let residue = fs::read_dir(&fixture.root)
            .unwrap_or_else(|error| panic!("read dir: {error}"))
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".lotta-patch-")
            });
        assert!(!residue);
    }
    super::patch::transaction::fail_commit_at(usize::MAX);
}

#[test]
fn phase_races_reject_without_install_or_residue() {
    let replaced = Fixture::new("race-replaced");
    replaced.write("source.txt", "old\n");
    let source = replaced.root.join("source.txt");
    super::patch::transaction::set_phase_hook(move || {
        fs::write(source, "external\n").unwrap_or_else(|error| panic!("replace: {error}"));
    });
    let update = "*** Begin Patch\n*** Update File: source.txt\n@@\n-old\n+new\n*** End Patch";
    assert!(replaced.apply(update).is_err());
    assert_eq!(
        fs::read(replaced.root.join("source.txt")).unwrap_or_default(),
        b"external\n"
    );
    assert!(!has_residue(&replaced.root));

    let appeared = Fixture::new("race-appeared");
    let destination = appeared.root.join("new.txt");
    super::patch::transaction::set_phase_hook(move || {
        fs::write(destination, "external\n").unwrap_or_else(|error| panic!("appear: {error}"));
    });
    let add = "*** Begin Patch\n*** Add File: new.txt\n+patch\n*** End Patch";
    assert!(appeared.apply(add).is_err());
    assert_eq!(
        fs::read(appeared.root.join("new.txt")).unwrap_or_default(),
        b"external\n"
    );
    assert!(!has_residue(&appeared.root));
}

fn has_residue(root: &Path) -> bool {
    fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read dir: {error}"))
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".lotta-patch-")
        })
}

#[test]
fn cancelled_before_commit_has_no_effect() {
    let fixture = Fixture::new("cancel");
    let bundle =
        FileToolBundle::new(&fixture.root, &fixture.artifacts).unwrap_or_else(|_| panic!("bundle"));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let control = OperationControl::new(cancellation, Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("control"));
    assert!(
        operations::execute(
            &bundle.state,
            "apply_patch",
            &json!({"input":"*** Begin Patch\n*** Add File: no.txt\n+x\n*** End Patch"}),
            &control
        )
        .is_err()
    );
    assert!(!fixture.root.join("no.txt").exists());
}

#[test]
fn traversal_absolute_and_symlink_paths_are_confined() {
    let fixture = Fixture::new("confined");
    let external = fixture
        .root
        .parent()
        .unwrap_or(Path::new("/"))
        .join(format!("lotta-external-{}", std::process::id()));
    fs::write(&external, "marker\n").unwrap_or_else(|error| panic!("external: {error}"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, fixture.root.join("link.txt"))
        .unwrap_or_else(|error| panic!("symlink: {error}"));
    let absolute = external.to_string_lossy();
    for patch in [
        "*** Begin Patch\n*** Add File: ../outside.txt\n+x\n*** End Patch".to_owned(),
        format!("*** Begin Patch\n*** Update File: {absolute}\n@@\n-marker\n+x\n*** End Patch"),
        "*** Begin Patch\n*** Delete File: ../outside.txt\n*** End Patch".to_owned(),
        concat!(
            "*** Begin Patch\n*** Update File: link.txt\n",
            "*** Move to: moved.txt\n@@\n-marker\n+x\n*** End Patch",
        )
        .to_owned(),
    ] {
        assert!(fixture.apply(&patch).is_err());
    }
    assert_eq!(fs::read(&external).unwrap_or_default(), b"marker\n");
    let _ = fs::remove_file(external);
}

#[test]
fn scanner_uses_full_parser_and_extracts_move_destination() {
    let patch = concat!(
        "*** Begin Patch\n*** Update File: source.txt\n",
        "*** Move to: destination.txt\n@@\n-old\n+new\n*** End Patch",
    );
    assert_eq!(
        super::patch::scan_paths(patch),
        Ok(vec!["source.txt", "destination.txt"])
    );
    assert!(
        super::patch::scan_paths("*** Begin Patch\n*** Add File: x\nbad\n*** End Patch").is_err()
    );
}
