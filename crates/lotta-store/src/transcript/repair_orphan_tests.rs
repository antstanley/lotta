use crate::StoreErrorKind;
use crate::transcript::task25_test_support::{corpus, ids, message_text, snapshot};
use serde_json::json;
use sha2::{Digest as _, Sha256};

#[cfg(unix)]
fn file_identity(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).expect("metadata").ino()
}

#[cfg(not(unix))]
const fn file_identity(_path: &std::path::Path) -> u64 {
    0
}

fn assert_only_conversation_changed(
    before: &std::collections::BTreeMap<String, Vec<u8>>,
    after: &std::collections::BTreeMap<String, Vec<u8>>,
) {
    let conversation = before
        .keys()
        .find(|path| path.ends_with("conversation.json"))
        .expect("conversation snapshot");
    assert_ne!(after[conversation], before[conversation]);
    for (path, bytes) in before {
        if path != conversation {
            assert_eq!(after[path].as_slice(), bytes.as_slice(), "{path}");
        }
    }
}

#[tokio::test]
async fn corpus_repairs_conversation_only() {
    let corpus = corpus("orphan_result_repair_input", "", "task25-orphan");
    let conversation_path = corpus.conversation_path();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&conversation_path).expect("conversation"))
            .expect("conversation JSON");
    value["future_field"] = json!({"nested":"preserved"});
    std::fs::write(
        &conversation_path,
        serde_json::to_vec_pretty(&value).expect("conversation bytes"),
    )
    .expect("future field");
    let before = snapshot(corpus.root.path());
    let message_bytes = std::fs::read(corpus.messages()).expect("messages");
    let message_hash: [u8; 32] = Sha256::digest(&message_bytes).into();
    let identity = file_identity(&corpus.messages());

    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("repair load");
    assert_eq!(
        ids(&loaded),
        ["msg-assistant-fixture", "msg-result-valid-fixture"]
    );
    assert_eq!(
        message_text(&loaded, "msg-result-valid-fixture"),
        "SANITIZED_FIXTURE_VALID_RESULT"
    );
    assert_eq!(
        std::fs::read(corpus.messages()).expect("after"),
        message_bytes
    );
    assert_eq!(file_identity(&corpus.messages()), identity);
    let after_hash: [u8; 32] =
        Sha256::digest(std::fs::read(corpus.messages()).expect("messages for hash")).into();
    assert_eq!(after_hash, message_hash);

    let persisted = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("public transcript reload");
    let repaired = persisted
        .conversation()
        .in_context_message_ids
        .as_slice()
        .iter()
        .map(lotta_domain::MessageId::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        repaired,
        ["msg-assistant-fixture", "msg-result-valid-fixture"]
    );
    let after = snapshot(corpus.root.path());
    assert_only_conversation_changed(&before, &after);
    let persisted_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(conversation_path).expect("persisted conversation"))
            .expect("persisted JSON");
    assert_eq!(
        persisted_json["future_field"],
        json!({"nested":"preserved"})
    );
}

#[tokio::test]
async fn external_conversation_change_conflicts_before_repair_commit() {
    let corpus = corpus("orphan_result_repair_input", "", "task25-orphan-conflict");
    let messages = std::fs::read(corpus.messages()).expect("messages before");
    let manifest = std::fs::read(corpus.manifest()).expect("manifest before");
    let mut external: serde_json::Value = serde_json::from_slice(
        &std::fs::read(corpus.conversation_path()).expect("conversation before"),
    )
    .expect("conversation JSON");
    external["summary"] = "EXTERNAL_WRITER_WINS".into();
    let external = serde_json::to_vec_pretty(&external).expect("external bytes");
    let expected = external.clone();
    let error = corpus
        .store
        .load_transcript_observed(&corpus.agent, &corpus.conversation, move |path| {
            std::fs::write(path, &external).map_err(|io| crate::StoreError::from_io(path, &io))
        })
        .await
        .expect_err("repair conflict");
    assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
    assert_eq!(
        std::fs::read(corpus.conversation_path()).expect("external survives"),
        expected
    );
    assert_eq!(
        std::fs::read(corpus.messages()).expect("messages after"),
        messages
    );
    assert_eq!(
        std::fs::read(corpus.manifest()).expect("manifest after"),
        manifest
    );
}
