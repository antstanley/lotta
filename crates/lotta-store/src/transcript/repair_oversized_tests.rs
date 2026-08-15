use crate::transcript::projection::REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX;
use crate::transcript::task25_test_support::{
    corpus, ids, json_lines, snapshot, tool_result_text, write_json_lines,
};
use serde_json::{Value, json};

fn seed_tool_flow(corpus: &super::super::task25_test_support::CorpusStore, text: &str) {
    let session = json_lines(&corpus.messages()).remove(0);
    let assistant = json!({
        "type":"message", "id":"entry-call", "parentId":null,
        "timestamp":"2000-01-01T00:00:00.000Z",
        "message":{"id":"msg-call","role":"assistant","timestamp":946_684_800_000_f64,
            "content":[{"type":"toolCall","id":"call-1","name":"fixture","arguments":{}}],
            "api":"fixture-api","provider":"fixture-provider","model":"fixture-model",
            "usage":{},"stopReason":"toolUse"}
    });
    let result = json!({
        "type":"message", "id":"entry-result", "parentId":"entry-call",
        "timestamp":"2000-01-01T00:00:00.000Z",
        "message":{"id":"msg-result","role":"toolResult","timestamp":946_684_800_001_f64,
            "content":[{"type":"text","text":text}],"toolCallId":"call-1",
            "toolName":"fixture","isError":false}
    });
    write_json_lines(&corpus.messages(), &[session, assistant, result], true);
    let mut conversation: Value =
        serde_json::from_slice(&std::fs::read(corpus.conversation_path()).expect("conversation"))
            .expect("conversation JSON");
    conversation["in_context_message_ids"] = json!(["msg-call", "msg-result"]);
    std::fs::write(
        corpus.conversation_path(),
        serde_json::to_vec_pretty(&conversation).expect("conversation bytes"),
    )
    .expect("conversation context");
}

async fn public_clip(text: &str, label: &str) -> (String, bool) {
    let corpus = corpus("current_typescript_state", "", label);
    seed_tool_flow(&corpus, text);
    let before = snapshot(corpus.root.path());
    let loaded = corpus
        .store
        .load_transcript(&corpus.agent, &corpus.conversation)
        .await
        .expect("public clipping load");
    assert_eq!(ids(&loaded), ["msg-call", "msg-result"]);
    let clipped = loaded.clipped_tool_results();
    let projected = tool_result_text(&loaded);
    assert_eq!(snapshot(corpus.root.path()), before);
    (projected, clipped)
}

#[tokio::test]
async fn below_at_above_and_unicode_clip_exactly() {
    for length in [
        REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX - 1,
        REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX,
    ] {
        let input = "x".repeat(length);
        let (projected, clipped) = public_clip(&input, "task25-clip-within").await;
        assert_eq!(projected, input);
        assert!(!clipped);
    }

    let input = "x".repeat(REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX + 1);
    let (projected, clipped) = public_clip(&input, "task25-clip-above").await;
    let expected = format!(
        "{}\n[Tool result truncated during local transcript repair: omitted 75 chars]\n{}",
        "x".repeat(19_963),
        "x".repeat(19_963)
    );
    assert_eq!(projected, expected);
    assert_eq!(
        projected.encode_utf16().count(),
        REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX
    );
    assert!(clipped);

    let unicode = "😀".repeat(20_001);
    let (projected, clipped) = public_clip(&unicode, "task25-clip-unicode").await;
    let expected = format!(
        "{}\n[Tool result truncated during local transcript repair: omitted 78 chars]\n{}",
        "😀".repeat(9_981),
        "😀".repeat(9_981)
    );
    assert_eq!(projected, expected);
    assert_eq!(projected.encode_utf16().count(), 39_998);
    assert!(!projected.contains('\u{fffd}'));
    assert!(clipped);
}
