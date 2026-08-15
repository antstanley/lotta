use crate::transcript::task25_test_support::{
    corpus, ids, json_lines, message_text, snapshot, write_json_lines,
};

async fn assert_tolerated(corpus: &super::super::task25_test_support::CorpusStore) {
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("public tolerant load");
    assert_eq!(ids(&loaded), ["msg-tolerated-fixture"]);
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn missing_session_header() {
    let corpus = corpus(
        "baseline_tolerated_versioned_rows",
        "",
        "task25-missing-header",
    );
    let mut rows = json_lines(&corpus.messages());
    assert_eq!(rows[0]["type"], "session");
    rows.remove(0);
    assert!(rows.iter().all(|row| row["type"] != "session"));
    write_json_lines(&corpus.messages(), &rows, true);
    assert_tolerated(&corpus).await;
}

#[tokio::test]
async fn duplicate_entry_ids() {
    let corpus = corpus(
        "baseline_tolerated_versioned_rows",
        "",
        "task25-duplicate-entry",
    );
    let mut rows = json_lines(&corpus.messages());
    let mut replacement = rows[1].clone();
    replacement["message"]["content"][0]["text"] = "LATEST_DUPLICATE".into();
    rows.push(replacement);
    assert_eq!(rows[1]["id"], rows[2]["id"]);
    write_json_lines(&corpus.messages(), &rows, true);
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("duplicate entry IDs load");
    assert_eq!(ids(&loaded), ["msg-tolerated-fixture"]);
    assert_eq!(
        message_text(&loaded, "msg-tolerated-fixture"),
        "LATEST_DUPLICATE"
    );
    assert_eq!(snapshot(corpus.root.path()), before);
}

#[tokio::test]
async fn unverified_parent_links() {
    let corpus = corpus(
        "baseline_tolerated_versioned_rows",
        "",
        "task25-parent-links",
    );
    let mut rows = json_lines(&corpus.messages());
    rows[1]["parentId"] = "entry-does-not-exist".into();
    assert_eq!(rows[1]["parentId"], "entry-does-not-exist");
    write_json_lines(&corpus.messages(), &rows, true);
    assert_tolerated(&corpus).await;
}

#[tokio::test]
async fn compaction_without_retained_graph() {
    let corpus = corpus(
        "baseline_tolerated_versioned_rows",
        "",
        "task25-compaction-graph",
    );
    let mut rows = json_lines(&corpus.messages());
    let row = &mut rows[1];
    row["type"] = "compaction".into();
    row["parentId"] = "missing-parent".into();
    row["summary"] = "summary without retained graph".into();
    row["firstKeptEntryId"] = "missing-retained-entry".into();
    row["tokensBefore"] = 42.into();
    assert_eq!(row["firstKeptEntryId"], "missing-retained-entry");
    write_json_lines(&corpus.messages(), &rows, true);
    assert_tolerated(&corpus).await;
}
