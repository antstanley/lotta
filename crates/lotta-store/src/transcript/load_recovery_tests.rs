use crate::StoreErrorKind;
use crate::transcript::task25_test_support::{corpus, ids, snapshot};
use crate::transcript::{TRANSCRIPT_BYTES_MAX, TRANSCRIPT_LINE_BYTES_MAX};
use lotta_testkit::fixtures::FixtureLoader;
use std::io::Write as _;

#[tokio::test]
async fn interrupted_append_recovers_complete_prefix() {
    let corpus = corpus("interrupted_append", "input", "task25-interrupted-append");
    let source = corpus.messages();
    let before = std::fs::read(&source).expect("source");
    let expected = FixtureLoader::new()
        .load_bytes("persistence/interrupted_append/expected/complete-prefix.jsonl")
        .expect("expected prefix");
    assert!(before.starts_with(&expected));
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("prefix load");
    assert_eq!(ids(&loaded), ["msg-user-fixture"]);
    assert_eq!(std::fs::read(source).expect("unchanged"), before);
}

#[tokio::test]
async fn interrupted_replacement_preserves_original() {
    let corpus = corpus("interrupted_replacement", "input", "task25-replacement");
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("original load");
    assert_eq!(ids(&loaded), ["msg-active-fixture"]);
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn corrupt_and_unsupported_manifests_reject_without_mutation() {
    for variant in [
        "corrupt_json/input",
        "unsupported_schema/input",
        "unsupported_provider/input",
    ] {
        let corpus = corpus("corrupt_unsupported_manifests", variant, "task25-corrupt");
        let before = snapshot(corpus.root.path());
        let error = corpus
            .store
            .load_transcript(&corpus.agent, &corpus.conversation)
            .await
            .expect_err("manifest rejection");
        assert_eq!(error.kind(), StoreErrorKind::Parse, "{variant}");
        assert_eq!(snapshot(corpus.root.path()), before);
    }
}

#[tokio::test]
async fn complete_malformed_rejects_but_valid_unterminated_row_loads() {
    let malformed = corpus("current_typescript_state", "", "task25-malformed-row");
    let before = std::fs::read(malformed.messages()).expect("before");
    let mut bytes = before.clone();
    bytes.extend_from_slice(b"{not-json}\n");
    std::fs::write(malformed.messages(), &bytes).expect("malformed row");
    let error = malformed
        .store
        .load_transcript(&malformed.agent, &malformed.conversation)
        .await
        .expect_err("terminated malformed row");
    assert_eq!(error.kind(), StoreErrorKind::Parse);
    assert_eq!(
        std::fs::read(malformed.messages()).expect("unchanged"),
        bytes
    );

    let valid = corpus("current_typescript_state", "", "task25-valid-tail");
    let mut bytes = std::fs::read(valid.messages()).expect("valid bytes");
    assert_eq!(bytes.pop(), Some(b'\n'));
    std::fs::write(valid.messages(), bytes).expect("unterminated valid row");
    let loaded = valid
        .store
        .load_transcript(&valid.agent, &valid.conversation)
        .await
        .expect("valid unterminated row");
    assert!(!loaded.messages().is_empty());
}

#[tokio::test]
async fn line_and_total_above_bounds_are_limit_without_mutation() {
    let line = corpus("current_typescript_state", "", "task25-line-limit");
    let payload = vec![b'x'; TRANSCRIPT_LINE_BYTES_MAX + 1];
    let mut file = std::fs::File::create(line.messages()).expect("line file");
    file.write_all(&payload).expect("line payload");
    file.write_all(b"\n").expect("line LF");
    drop(file);
    let before = std::fs::metadata(line.messages())
        .expect("line metadata")
        .len();
    let error = line
        .store
        .load_transcript(&line.agent, &line.conversation)
        .await
        .expect_err("line limit");
    assert_eq!(error.kind(), StoreErrorKind::Limit);
    assert_eq!(
        std::fs::metadata(line.messages()).expect("after").len(),
        before
    );

    let total = corpus("current_typescript_state", "", "task25-total-limit");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(total.messages())
        .expect("total file");
    file.set_len(TRANSCRIPT_BYTES_MAX + 1)
        .expect("sparse total");
    let error = total
        .store
        .load_transcript(&total.agent, &total.conversation)
        .await
        .expect_err("total limit");
    assert_eq!(error.kind(), StoreErrorKind::Limit);
    assert_eq!(
        std::fs::metadata(total.messages())
            .expect("total metadata")
            .len(),
        TRANSCRIPT_BYTES_MAX + 1
    );
    file.set_len(0).expect("sparse cleanup");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_source_rejects_without_touching_outside() {
    use std::os::unix::fs::symlink;
    let corpus = corpus("current_typescript_state", "", "task25-load-symlink");
    std::fs::remove_file(corpus.messages()).expect("remove messages");
    let outside = lotta_testkit::roots::TemporaryRoot::new("task25-outside").expect("outside");
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"outside-safe").expect("sentinel");
    symlink(&sentinel, corpus.messages()).expect("messages symlink");
    assert!(
        corpus
            .store
            .load_transcript(&corpus.agent, &corpus.conversation)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(sentinel).expect("outside after"),
        b"outside-safe"
    );
}
