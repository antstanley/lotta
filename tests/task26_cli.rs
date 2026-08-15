use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::roots::TemporaryRoot;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl";

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lotta"))
        .args(arguments)
        .output()
        .expect("execute lotta")
}

fn copy_fixture(case: &str, suffix: &str, target: &Path) {
    let loader = FixtureLoader::new();
    let source = format!("persistence/{case}/{suffix}");
    for relative in loader.list_tree(&source).expect("fixture tree") {
        let destination = target.join(&relative);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("directories");
        std::fs::write(
            destination,
            loader
                .load_bytes(format!("{source}/{relative}"))
                .expect("fixture bytes"),
        )
        .expect("fixture copy");
    }
}

#[derive(Debug, Eq, PartialEq)]
struct FileState {
    bytes: Vec<u8>,
    hash: [u8; 32],
    len: u64,
    modified: std::time::SystemTime,
    inode: u64,
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, FileState> {
    let mut files = BTreeMap::new();
    collect(root, root, &mut files);
    files
}

fn collect(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, FileState>) {
    let mut entries = std::fs::read_dir(current)
        .expect("directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("entries");
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if entry.file_type().expect("type").is_dir() {
            collect(root, &path, files);
        } else {
            let bytes = std::fs::read(&path).expect("bytes");
            let metadata = std::fs::metadata(&path).expect("metadata");
            files.insert(
                path.strip_prefix(root).expect("relative").to_owned(),
                FileState {
                    hash: Sha256::digest(&bytes).into(),
                    len: metadata.len(),
                    modified: metadata.modified().expect("modified"),
                    inode: inode(&metadata),
                    bytes,
                },
            );
        }
    }
}

#[cfg(unix)]
fn inode(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.ino()
}
#[cfg(not(unix))]
const fn inode(_metadata: &std::fs::Metadata) -> u64 {
    0
}

#[test]
fn real_migration_then_verify_is_nonmutating() {
    let root = TemporaryRoot::new("task26-cli-migration").expect("root");
    let backend = root.path().join("backend");
    copy_fixture("unversioned_legacy_transcript", "", &backend);
    let storage = backend.to_str().expect("UTF-8 path");
    let before = tree_snapshot(&backend);
    let directory = backend.join("conversations").join(KEY);
    let original_messages =
        std::fs::read(directory.join("messages.jsonl")).expect("original messages");
    let dry = run(&[
        "local-backend",
        "migrate-transcripts",
        "--storage-dir",
        storage,
        "--dry-run",
    ]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(tree_snapshot(&backend), before);
    assert_eq!(dry.stderr, b"");
    assert_eq!(
        String::from_utf8(dry.stdout).expect("stdout"),
        format!(
            "migration dry_run=true conversations=1\nmigration path=conversations/{KEY} status=converted messages=1 backup=none\n"
        )
    );
    let migration = run(&[
        "local-backend",
        "migrate-transcripts",
        "--storage-dir",
        storage,
    ]);
    assert!(migration.status.success());
    assert!(migration.stderr.is_empty());
    let migration_out = String::from_utf8(migration.stdout).expect("stdout");
    let lines = migration_out.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "migration dry_run=false conversations=1");
    assert!(lines[1].starts_with(&format!("migration path=conversations/{KEY} status=converted messages=1 backup=conversations/{KEY}/messages.jsonl.lotta-upgrade-")));
    assert!(lines[1].ends_with(".bak"));
    assert_eq!(lines.len(), 2);
    let backup_rel = lines[1].split(" backup=").nth(1).expect("backup path");
    assert_eq!(
        std::fs::read(backend.join(backup_rel)).expect("backup"),
        original_messages
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(
        manifest["backup_path"].as_str(),
        Path::new(backup_rel)
            .file_name()
            .and_then(|name| name.to_str())
    );
    assert!(
        manifest["migrated_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let rows = std::fs::read_to_string(directory.join("messages.jsonl")).expect("messages");
    assert!(
        rows.lines()
            .next()
            .is_some_and(|line| line.contains("\"version\":3"))
    );
    let conversation: Value = serde_json::from_slice(
        &std::fs::read(directory.join("conversation.json")).expect("conversation"),
    )
    .expect("conversation json");
    assert_eq!(
        conversation["in_context_message_ids"],
        json!(["ui-msg-fixture"])
    );
    let after_migration = tree_snapshot(&backend);
    assert_ne!(after_migration, before);
    let rerun = run(&[
        "local-backend",
        "migrate-transcripts",
        "--storage-dir",
        storage,
    ]);
    assert!(rerun.status.success());
    assert!(rerun.stderr.is_empty());
    assert_eq!(
        String::from_utf8(rerun.stdout).expect("rerun stdout"),
        format!(
            "migration dry_run=false conversations=1\nmigration path=conversations/{KEY} status=already_current messages=0 backup=none\n"
        )
    );
    assert_eq!(tree_snapshot(&backend), after_migration);
    let verify = run(&["local-backend", "verify", "--storage-dir", storage]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    assert_eq!(tree_snapshot(&backend), after_migration);
    assert_eq!(
        String::from_utf8(verify.stdout).expect("stdout"),
        concat!(
            "verification conversations=1 rows=2 findings=0\n",
            "class duplicate_entry_id count=0\n",
            "class invalid_parent_link count=0\n",
            "class invalid_session_header count=0\n",
            "class orphan_tool_result count=0\n",
            "class invalid_compaction_reference count=0\n",
            "class unsupported_outer_field count=0\n",
        )
    );
}

#[test]
fn real_verify_reports_all_six_classes_without_contents() {
    let root = TemporaryRoot::new("task26-cli-six").expect("root");
    let backend = root.path().join("backend");
    copy_fixture("baseline_tolerated_versioned_rows", "", &backend);
    let messages = backend
        .join("conversations")
        .join(KEY)
        .join("messages.jsonl");
    let text = std::fs::read_to_string(&messages).expect("messages");
    let mut rows = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON"))
        .collect::<Vec<_>>();
    rows[0]["unsupportedSecret"] = "NEVER_PRINT_THIS_CONTENT".into();
    rows[1]["parentId"] = "missing-parent".into();
    let mut duplicate = rows[1].clone();
    duplicate["parentId"] = rows[1]["id"].clone();
    rows.push(duplicate);
    let mut misplaced = rows[0].clone();
    misplaced["id"] = "second-session".into();
    rows.push(misplaced);
    let mut orphan = rows[1].clone();
    orphan["id"] = "orphan-entry".into();
    orphan["parentId"] = rows[1]["id"].clone();
    orphan["message"] = json!({
        "id":"orphan-message", "role":"toolResult", "timestamp":1.0,
        "content":[{"type":"text","text":"NEVER_PRINT_TOOL_CONTENT"}],
        "toolCallId":"missing-call", "toolName":"read"
    });
    rows.push(orphan);
    let mut compaction = rows[1].clone();
    compaction["type"] = "compaction".into();
    compaction["id"] = "compaction-entry".into();
    compaction["parentId"] = "orphan-entry".into();
    compaction["summary"] = "NEVER_PRINT_SUMMARY".into();
    compaction["firstKeptEntryId"] = "missing-retained".into();
    compaction["tokensBefore"] = 1.into();
    rows.push(compaction);
    let serialized = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&messages, serialized).expect("crafted transcript");
    let before = tree_snapshot(&backend);
    let output = run(&[
        "local-backend",
        "verify",
        "--storage-dir",
        backend.to_str().expect("path"),
    ]);
    assert!(output.status.success());
    assert_eq!(tree_snapshot(&backend), before);
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    for class in [
        "duplicate_entry_id",
        "invalid_parent_link",
        "invalid_session_header",
        "orphan_tool_result",
        "invalid_compaction_reference",
        "unsupported_outer_field",
    ] {
        assert!(
            stdout.contains(&format!("class {class} count=")),
            "{stdout}"
        );
        assert!(stdout.contains(&format!("class={class}")), "{stdout}");
    }
    for secret in [
        "NEVER_PRINT_THIS_CONTENT",
        "NEVER_PRINT_TOOL_CONTENT",
        "NEVER_PRINT_SUMMARY",
    ] {
        assert!(!stdout.contains(secret));
    }
    assert!(output.stderr.is_empty());
}

#[test]
fn exact_cli_negatives_exit_two_before_server_bind() {
    let cases: &[&[&str]] = &[
        &["local-backend", "verify"],
        &["local-backend", "verify", "--storage-dir", "relative"],
        &["local-backend", "verify", "--unknown"],
        &[
            "local-backend",
            "migrate-transcripts",
            "--storage-dir",
            "/tmp/a",
            "--dry-run",
            "--dry-run",
        ],
        &["server", "--backend", "remote", "--listen"],
    ];
    for arguments in cases {
        let output = run(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(!stderr.contains("NEVER_PRINT"));
    }
}
