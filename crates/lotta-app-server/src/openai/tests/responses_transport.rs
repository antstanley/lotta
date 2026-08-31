use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::openai::test_support::{
    agent, launch_with_runtime, launch_with_runtime_and_controller, post, roots, seed,
};

struct FailingController;

impl crate::ws::TurnController for FailingController {
    fn submit_turn(
        &self,
        _: crate::ws::command::InputCommand,
        _: crate::ws::DeferredInput,
        _: CancellationToken,
        _: Arc<dyn crate::ws::RuntimeEventSink>,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::error::AppServerError>> + Send + '_>> {
        Box::pin(async { Err(crate::error::AppServerError::Unavailable) })
    }
}

fn response_id(response: &crate::openai::test_support::HttpResponse) -> String {
    let value: Value = serde_json::from_str(&response.body)
        .unwrap_or_else(|error| panic!("response JSON: {error}; {}", response.body));
    value["id"]
        .as_str()
        .unwrap_or_else(|| panic!("response id"))
        .to_owned()
}

mod non_stored {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn headerless_is_deleted_and_chat_key_persists() {
        let roots = roots("responses-nonstored");
        seed(
            &roots.storage,
            &[agent("agent-local-response", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&roots, true, None, runtime.clone()).await;
        let body = r#"{"model":"memo","input":"hello","store":false}"#;
        let first = post(&handle, "/v1/responses", "", body).await;
        assert_eq!(first.status, 200);
        assert!(response_id(&first).starts_with("resp_"));
        assert!(!response_id(&first).starts_with("resp_letta_"));
        let keyed = post(
            &handle,
            "/v1/responses",
            "X-Letta-Chat-Key: persistent\r\n",
            body,
        )
        .await;
        assert_eq!(keyed.status, 200);
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 1);
        let remaining = std::fs::read_dir(roots.storage.join("conversations"))
            .map_or(0, std::iter::Iterator::count);
        assert_eq!(remaining, 1);
        handle.shutdown();
        handle.wait().await.unwrap();
    }
}

mod no_idempotency {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn idempotency_header_is_ignored_and_both_requests_execute() {
        let roots = roots("responses-no-idempotency");
        seed(
            &roots.storage,
            &[agent("agent-local-response-idem", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&roots, true, None, runtime.clone()).await;
        let headers = "Idempotency-Key: independently\r\n";
        let body = r#"{"model":"memo","input":"hello"}"#;
        let (first, second) = tokio::join!(
            post(&handle, "/v1/responses", headers, body),
            post(&handle, "/v1/responses", headers, body),
        );
        assert_eq!(
            (first.status, second.status),
            (200, 200),
            "first={} second={}",
            first.body,
            second.body
        );
        assert_ne!(response_id(&first), response_id(&second));
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 4);
        let stages = runtime.stages.lock().unwrap().clone();
        assert_eq!(stages, ["admit", "continue", "admit", "continue"]);
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 2);
        let remaining = std::fs::read_dir(roots.storage.join("conversations"))
            .map_or(0, std::iter::Iterator::count);
        assert_eq!(remaining, 0);
        handle.shutdown();
        handle.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_outcome_has_no_cursor_and_leaves_no_orphan() {
        let roots = roots("responses-failed-cleanup");
        seed(
            &roots.storage,
            &[agent("agent-local-response-fail", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime_and_controller(
            &roots,
            true,
            None,
            runtime.clone(),
            Arc::new(FailingController),
        )
        .await;
        let response = post(
            &handle,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","store":true}"#,
        )
        .await;
        assert_eq!(response.status, 500);
        assert!(!response_id(&response).starts_with("resp_letta_"));
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 1);
        let remaining = std::fs::read_dir(roots.storage.join("conversations"))
            .map_or(0, std::iter::Iterator::count);
        assert_eq!(remaining, 0);
        handle.shutdown();
        handle.wait().await.unwrap();
    }
}

mod previous_response_id {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn stored_cursor_forks_hidden_and_continues_independently() {
        let roots = roots("responses-stored-fork");
        let agent_id = "agent-local-response-store";
        seed(&roots.storage, &[agent(agent_id, "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&roots, true, None, runtime.clone()).await;
        let first = post(
            &handle,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"remember green","store":true}"#,
        )
        .await;
        assert_eq!(first.status, 200);
        let first_id = response_id(&first);
        let cursor = crate::openai::cursor::parse(&first_id).unwrap();
        assert_eq!(cursor.agent_id.as_str(), agent_id);
        let second_body = serde_json::json!({
            "model":"memo","input":"what color?","store":true,
            "previous_response_id":first_id
        })
        .to_string();
        let second = post(&handle, "/v1/responses", "", &second_body).await;
        assert_eq!(second.status, 200, "{}", second.body);
        let second_cursor = crate::openai::cursor::parse(&response_id(&second)).unwrap();
        assert_ne!(cursor.conversation_id, second_cursor.conversation_id);
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 4);
        let paths = lotta_store::StorePaths::new(roots.storage.clone()).unwrap();
        let source_record = paths
            .conversation_dir(&cursor.agent_id, &cursor.conversation_id)
            .unwrap()
            .join("conversation.json");
        let fork_record = paths
            .conversation_dir(&second_cursor.agent_id, &second_cursor.conversation_id)
            .unwrap()
            .join("conversation.json");
        let source_value: Value =
            serde_json::from_str(&std::fs::read_to_string(source_record).unwrap()).unwrap();
        let fork_value: Value =
            serde_json::from_str(&std::fs::read_to_string(fork_record).unwrap()).unwrap();
        assert_ne!(source_value["hidden"], true);
        assert_eq!(fork_value["hidden"], true);
        handle.shutdown();
        handle.wait().await.unwrap();
    }

    #[tokio::test]
    async fn malformed_cursor_is_exact_not_found() {
        let roots = roots("responses-missing-cursor");
        seed(
            &roots.storage,
            &[agent("agent-local-response-missing", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&roots, true, None, runtime).await;
        let response = post(
            &handle,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","previous_response_id":"resp_missing"}"#,
        )
        .await;
        assert_eq!(response.status, 404);
        let value: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(value["error"]["code"], "response_not_found");
        handle.shutdown();
        handle.wait().await.unwrap();
    }
}

mod streaming {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_stream_has_exact_terminals_and_no_done_sentinel() {
        let roots = roots("responses-stream");
        seed(
            &roots.storage,
            &[agent("agent-local-response-stream", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&roots, true, None, runtime).await;
        let response = post(
            &handle,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","stream":true}"#,
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type.as_deref(), Some("text/event-stream"));
        assert!(response.body.contains("response.created"));
        assert!(response.body.contains("response.in_progress"));
        assert!(response.body.contains("response.completed"));
        assert!(!response.body.contains("[DONE]"));
        handle.shutdown();
        handle.wait().await.unwrap();
    }
}
