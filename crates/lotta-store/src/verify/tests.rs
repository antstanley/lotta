use super::*;
use crate::transcript::task25_test_support::{corpus, json_lines, snapshot, write_json_lines};
use lotta_testkit::roots::TemporaryRoot;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

fn current(label: &str) -> crate::transcript::task25_test_support::CorpusStore {
    corpus("baseline_tolerated_versioned_rows", "", label)
}

fn report(corpus: &crate::transcript::task25_test_support::CorpusStore) -> VerificationReport {
    verify_transcripts(corpus.root.path().join("local-backend")).expect("verify")
}

fn findings(report: &VerificationReport) -> Vec<(String, usize, VerificationClass)> {
    report
        .findings
        .iter()
        .map(|finding| {
            let path = finding
                .path
                .strip_prefix(&report.storage_root)
                .expect("relative finding")
                .to_string_lossy()
                .into_owned();
            (path, finding.row, finding.class)
        })
        .collect()
}

fn path() -> String {
    concat!(
        "conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/messages.jsonl"
    )
    .to_owned()
}

fn assert_exact(
    corpus: &crate::transcript::task25_test_support::CorpusStore,
    expected: &[(usize, VerificationClass)],
) {
    let before = snapshot(corpus.root.path());
    let actual = findings(&report(corpus));
    let expected = expected
        .iter()
        .map(|(row, class)| (path(), *row, *class))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[test]
fn duplicate_entry_ids() {
    let corpus = current("verify-duplicate");
    let mut rows = json_lines(&corpus.messages());
    let mut duplicate = rows[1].clone();
    duplicate["parentId"] = rows[1]["id"].clone();
    rows.push(duplicate);
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(
        &corpus,
        &[
            (3, VerificationClass::DuplicateEntryId),
            (3, VerificationClass::InvalidParentLink),
        ],
    );
}

#[test]
fn invalid_parent_links() {
    let corpus = current("verify-parent");
    let mut rows = json_lines(&corpus.messages());
    rows[1]["parentId"] = rows[0]["id"].clone();
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(2, VerificationClass::InvalidParentLink)]);
}

#[test]
fn compaction_parent_rules() {
    let corpus = current("verify-compaction-parent");
    let mut rows = json_lines(&corpus.messages());
    let first_id = rows[1]["id"].clone();
    let mut first = rows[1].clone();
    first["id"] = "compaction-one".into();
    first["parentId"] = first_id.clone();
    first["type"] = "compaction".into();
    first["summary"] = "first".into();
    first["firstKeptEntryId"] = first_id.clone();
    first["tokensBefore"] = 1.into();
    let mut second = first.clone();
    second["id"] = "compaction-two".into();
    second["parentId"] = Value::Null;
    rows.push(first);
    rows.push(second);
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(4, VerificationClass::InvalidParentLink)]);
}

#[test]
fn header_timestamp_must_be_string() {
    let corpus = current("verify-header-timestamp");
    let mut rows = json_lines(&corpus.messages());
    rows[0]["timestamp"] = 1.into();
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(1, VerificationClass::InvalidSessionHeader)]);
}

#[test]
fn absent_multiple_session_header() {
    let corpus = current("verify-header");
    let mut rows = json_lines(&corpus.messages());
    let mut second = rows[0].clone();
    second["id"] = "second-session".into();
    rows.push(second);
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(3, VerificationClass::InvalidSessionHeader)]);
}

#[test]
fn orphan_tool_result() {
    let corpus = current("verify-orphan");
    let mut rows = json_lines(&corpus.messages());
    let mut orphan = rows[1].clone();
    orphan["id"] = "entry-orphan".into();
    orphan["parentId"] = rows[1]["id"].clone();
    orphan["message"] = tool_result("message-orphan", "missing-call");
    rows.push(orphan);
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(3, VerificationClass::OrphanToolResult)]);
}

#[test]
fn invalid_compaction_reference() {
    let corpus = current("verify-compaction");
    let mut rows = json_lines(&corpus.messages());
    let mut compaction = rows[1].clone();
    compaction["id"] = "entry-compaction".into();
    compaction["parentId"] = rows[1]["id"].clone();
    compaction["type"] = "compaction".into();
    compaction["summary"] = "summary".into();
    compaction["firstKeptEntryId"] = "missing-entry".into();
    compaction["tokensBefore"] = 7.into();
    rows.push(compaction);
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(
        &corpus,
        &[(3, VerificationClass::InvalidCompactionReference)],
    );
}

#[test]
fn unsupported_outer_fields() {
    let corpus = current("verify-fields");
    let mut rows = json_lines(&corpus.messages());
    rows[1]["unsupported"] = "must-not-be-printed".into();
    rows[1]["message"]["forwardCompatible"] = json!({"anything": true});
    write_json_lines(&corpus.messages(), &rows, true);
    assert_exact(&corpus, &[(2, VerificationClass::UnsupportedOuterField)]);
}

#[test]
fn does_not_mutate() {
    let corpus = current("verify-no-mutation");
    let mut rows = json_lines(&corpus.messages());
    rows[1]["parentId"] = "missing".into();
    write_json_lines(&corpus.messages(), &rows, true);
    let before = metadata_snapshot(corpus.root.path());
    let first = report(&corpus);
    let second = report(&corpus);
    assert_eq!(first, second);
    assert_eq!(metadata_snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn does_not_narrow_loading() {
    for case in ToleratedCase::ALL {
        let corpus = current(case.label());
        case.mutate(&corpus.messages());
        let before = snapshot(corpus.root.path());
        let verified = report(&corpus);
        assert!(
            verified
                .findings
                .iter()
                .any(|finding| finding.class == case.class())
        );
        assert_eq!(snapshot(corpus.root.path()), before);
        let loaded = corpus
            .store
            .load_transcript(&corpus.agent, &corpus.conversation)
            .await
            .expect("Task25 tolerated public load");
        if matches!(case, ToleratedCase::Orphan) {
            assert!(
                !loaded
                    .messages()
                    .iter()
                    .any(|message| message.id.as_str() == "message-orphan")
            );
        }
    }

    let corpus = current("verify-tolerance-combined-six");
    let mut rows = json_lines(&corpus.messages());
    rows.remove(0);
    rows[0]["parentId"] = "missing-parent".into();
    rows[0]["unsupported"] = true.into();
    let mut duplicate = rows[0].clone();
    duplicate["parentId"] = rows[0]["id"].clone();
    rows.push(duplicate);
    let mut orphan = rows[0].clone();
    orphan["id"] = "entry-orphan".into();
    orphan["parentId"] = rows[0]["id"].clone();
    orphan["message"] = tool_result("message-orphan", "missing-call");
    rows.push(orphan);
    let mut compaction = rows[0].clone();
    compaction["id"] = "entry-compaction".into();
    compaction["parentId"] = "entry-orphan".into();
    compaction["type"] = "compaction".into();
    compaction["summary"] = "summary".into();
    compaction["firstKeptEntryId"] = "missing-entry".into();
    compaction["tokensBefore"] = 1.into();
    rows.push(compaction);
    write_json_lines(&corpus.messages(), &rows, true);
    let before = snapshot(corpus.root.path());
    assert_eq!(
        findings(&report(&corpus)),
        vec![
            (path(), 0, VerificationClass::InvalidSessionHeader),
            (path(), 1, VerificationClass::InvalidParentLink),
            (path(), 1, VerificationClass::UnsupportedOuterField),
            (path(), 2, VerificationClass::DuplicateEntryId),
            (path(), 2, VerificationClass::InvalidParentLink),
            (path(), 2, VerificationClass::UnsupportedOuterField),
            (path(), 3, VerificationClass::OrphanToolResult),
            (path(), 3, VerificationClass::UnsupportedOuterField),
            (path(), 4, VerificationClass::InvalidCompactionReference),
            (path(), 4, VerificationClass::UnsupportedOuterField),
        ]
    );
    assert_eq!(snapshot(corpus.root.path()), before);
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("combined Task25 tolerated public load");
    assert!(
        !loaded
            .messages()
            .iter()
            .any(|message| message.id.as_str() == "message-orphan")
    );
}

#[test]
fn malformed_unsupported_and_absent_are_nonmutating() {
    let root = TemporaryRoot::new("verify-negative-input").expect("root");
    let backend = root.path().join("backend");
    assert!(
        verify_transcripts(&backend)
            .expect("absent")
            .findings
            .is_empty()
    );
    let corpus = current("verify-malformed");
    let before = snapshot(corpus.root.path());
    std::fs::write(corpus.manifest(), b"{bad\n").expect("corrupt manifest");
    let changed = snapshot(corpus.root.path());
    assert_eq!(
        verify_transcripts(corpus.root.path().join("local-backend"))
            .expect_err("malformed")
            .kind(),
        StoreErrorKind::Parse
    );
    assert_eq!(snapshot(corpus.root.path()), changed);
    assert_ne!(before, changed);
}

#[test]
fn verify_row_helper_exact_boundary() {
    let path = Path::new("rows");
    let mut state = VerifyState::new(path);
    state.rows = VERIFY_ROWS_MAX - 1;
    state
        .inspect(VERIFY_ROWS_MAX, br#"{"type":"unknown"}"#, true)
        .expect("last accepted row");
    let error = state
        .inspect(VERIFY_ROWS_MAX + 1, b"not-json", true)
        .expect_err("bound checked before parsing");
    assert_eq!(error.kind(), StoreErrorKind::Limit);
}

#[test]
fn line_and_total_bounds_are_nonmutating() {
    let corpus = current("verify-bounds");
    let above = vec![b'x'; transcript::TRANSCRIPT_LINE_BYTES_MAX + 1];
    std::fs::write(corpus.messages(), above).expect("above line");
    let before = metadata_snapshot(corpus.root.path());
    assert_eq!(
        verify_transcripts(corpus.root.path().join("local-backend"))
            .expect_err("line bound")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(metadata_snapshot(corpus.root.path()), before);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(corpus.messages())
        .expect("open sparse");
    file.set_len(transcript::TRANSCRIPT_BYTES_MAX + 1)
        .expect("sparse total");
    assert_eq!(
        verify_transcripts(corpus.root.path().join("local-backend"))
            .expect_err("total bound")
            .kind(),
        StoreErrorKind::Limit
    );
    file.set_len(0).expect("truncate cleanup");
}

#[cfg(unix)]
#[test]
fn symlink_source_cannot_touch_outside_sentinel() {
    use std::os::unix::fs::symlink;
    let corpus = current("verify-symlink");
    let outside = corpus.root.path().join("outside-sentinel");
    std::fs::write(&outside, b"sentinel-secret").expect("sentinel");
    std::fs::remove_file(corpus.messages()).expect("remove source");
    symlink(&outside, corpus.messages()).expect("symlink source");
    assert_eq!(
        verify_transcripts(corpus.root.path().join("local-backend"))
            .expect_err("reject symlink")
            .kind(),
        StoreErrorKind::InvalidPath
    );
    assert_eq!(
        std::fs::read(outside).expect("sentinel unchanged"),
        b"sentinel-secret"
    );
}

fn tool_result(id: &str, call: &str) -> Value {
    json!({
        "id": id,
        "role": "toolResult",
        "content": [{"type":"text", "text":"safe"}],
        "timestamp": 1.0,
        "toolCallId": call,
        "toolName": "read"
    })
}

#[derive(Clone, Copy)]
enum ToleratedCase {
    MissingHeader,
    Duplicate,
    Parent,
    Compaction,
    Orphan,
}

impl ToleratedCase {
    const ALL: [Self; 5] = [
        Self::MissingHeader,
        Self::Duplicate,
        Self::Parent,
        Self::Compaction,
        Self::Orphan,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::MissingHeader => "verify-tolerance-header",
            Self::Duplicate => "verify-tolerance-duplicate",
            Self::Parent => "verify-tolerance-parent",
            Self::Compaction => "verify-tolerance-compaction",
            Self::Orphan => "verify-tolerance-orphan",
        }
    }

    const fn class(self) -> VerificationClass {
        match self {
            Self::MissingHeader => VerificationClass::InvalidSessionHeader,
            Self::Duplicate => VerificationClass::DuplicateEntryId,
            Self::Parent => VerificationClass::InvalidParentLink,
            Self::Compaction => VerificationClass::InvalidCompactionReference,
            Self::Orphan => VerificationClass::OrphanToolResult,
        }
    }

    fn mutate(self, path: &Path) {
        let mut rows = json_lines(path);
        match self {
            Self::MissingHeader => {
                rows.remove(0);
            }
            Self::Duplicate => rows.push(rows[1].clone()),
            Self::Parent => rows[1]["parentId"] = "missing".into(),
            Self::Compaction => {
                rows[1]["type"] = "compaction".into();
                rows[1]["summary"] = "summary".into();
                rows[1]["firstKeptEntryId"] = "missing".into();
                rows[1]["tokensBefore"] = 1.into();
            }
            Self::Orphan => {
                rows[1]["message"] = tool_result("message-orphan", "missing-call");
            }
        }
        write_json_lines(path, &rows, true);
    }
}

#[derive(Debug, Eq, PartialEq)]
struct FileState {
    bytes: Vec<u8>,
    hash: [u8; 32],
    inode: u64,
    modified_nanos: u128,
}

fn metadata_snapshot(root: &Path) -> BTreeMap<String, FileState> {
    let mut output = BTreeMap::new();
    metadata_snapshot_into(root, root, &mut output);
    output
}

fn metadata_snapshot_into(root: &Path, current: &Path, output: &mut BTreeMap<String, FileState>) {
    let mut entries = std::fs::read_dir(current)
        .expect("read directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("entries");
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if entry.file_type().expect("file type").is_dir() {
            metadata_snapshot_into(root, &path, output);
        } else {
            let bytes = std::fs::read(&path).expect("bytes");
            let metadata = std::fs::metadata(&path).expect("metadata");
            let modified_nanos = metadata
                .modified()
                .expect("mtime")
                .duration_since(std::time::UNIX_EPOCH)
                .expect("mtime after epoch")
                .as_nanos();
            let hash: [u8; 32] = Sha256::digest(&bytes).into();
            output.insert(
                path.strip_prefix(root)
                    .expect("relative")
                    .to_string_lossy()
                    .into_owned(),
                FileState {
                    bytes,
                    hash,
                    inode: metadata.ino(),
                    modified_nanos,
                },
            );
        }
    }
}
