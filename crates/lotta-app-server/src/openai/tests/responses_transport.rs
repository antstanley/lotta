use std::{
    future::Future,
    io::{Read, Write},
    net::TcpStream,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use lotta_domain::{AgentId, BoundedJsonValue, ConversationId};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::openai::test_support::{
    agent, launch_responses_custom, launch_with_runtime, launch_with_runtime_and_controller, post,
    post_at, roots, seed,
};

type RepositoryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, crate::error::AppServerError>> + Send + 'a>>;

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

#[derive(Default)]
struct UnsupportedRepository {
    lookups: AtomicUsize,
    forks: AtomicUsize,
    deletes: AtomicUsize,
}

impl crate::ws::conversations::ConversationCommandRepository for UnsupportedRepository {
    fn supports_hidden_fork(&self) -> bool {
        false
    }

    fn contains_for_openai<'a>(
        &'a self,
        _: &'a AgentId,
        _: &'a ConversationId,
    ) -> RepositoryFuture<'a, bool> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(crate::error::AppServerError::Unavailable) })
    }

    fn fork_for_openai<'a>(
        &'a self,
        _: &'a AgentId,
        _: &'a ConversationId,
    ) -> RepositoryFuture<'a, ConversationId> {
        self.forks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(crate::error::AppServerError::Unavailable) })
    }

    fn delete_for_openai<'a>(
        &'a self,
        _: &'a AgentId,
        _: &'a ConversationId,
    ) -> RepositoryFuture<'a, ()> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(crate::error::AppServerError::Unavailable) })
    }
}

struct ExitGuard<'a>(&'a AtomicUsize);

impl Drop for ExitGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct GatedController {
    blocked: usize,
    emissions: usize,
    calls: AtomicUsize,
    entered: AtomicUsize,
    exited: AtomicUsize,
    changed: Notify,
    release: Semaphore,
}

impl GatedController {
    fn new(blocked: usize, emissions: usize) -> Self {
        Self {
            blocked,
            emissions,
            calls: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            exited: AtomicUsize::new(0),
            changed: Notify::new(),
            release: Semaphore::new(0),
        }
    }

    async fn wait_for(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let changed = self.changed.notified();
                if self.entered.load(Ordering::SeqCst) >= expected {
                    break;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected} owners did not enter controller"));
    }

    fn release(&self, count: usize) {
        self.release.add_permits(count);
    }
}

impl crate::ws::TurnController for GatedController {
    fn submit_turn(
        &self,
        _: crate::ws::command::InputCommand,
        deferred: crate::ws::DeferredInput,
        cancellation: CancellationToken,
        sink: Arc<dyn crate::ws::RuntimeEventSink>,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::error::AppServerError>> + Send + '_>> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if index < self.blocked {
                let _exit = ExitGuard(&self.exited);
                for event in 0..self.emissions {
                    let value = BoundedJsonValue::new(json!({
                        "kind":"text","text":format!("event-{event:04}-{}", "x".repeat(512))
                    }))
                    .unwrap_or_else(|error| panic!("bounded event: {error}"));
                    sink.emit(
                        &deferred.scope,
                        crate::ws::RuntimeEvent::StreamDelta {
                            delta: crate::ws::event::StreamDelta::Other(value),
                            subagent_id: None,
                        },
                    )?;
                }
                self.entered.fetch_add(1, Ordering::SeqCst);
                self.changed.notify_waiters();
                tokio::select! {
                    permit = self.release.acquire() => drop(permit),
                    () = cancellation.cancelled() => {}
                }
            }
            Ok(())
        })
    }
}

fn conversations(root: &std::path::Path) -> usize {
    std::fs::read_dir(root.join("conversations")).map_or(0, Iterator::count)
}

async fn wait_clean(state: &crate::openai::responses::ResponsesState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state.owner_count().await == 0 && state.setup_lock_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Responses owners did not quiesce"));
}

fn open_stream(
    address: std::net::SocketAddr,
    headers: &str,
    body: &str,
    marker: &str,
) -> TcpStream {
    let mut socket = TcpStream::connect(address).unwrap_or_else(|error| panic!("connect: {error}"));
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap_or_else(|error| panic!("read timeout: {error}"));
    let request = format!(
        concat!(
            "POST /v1/responses HTTP/1.1\r\nHost: localhost\r\n",
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            "{}\r\n{}"
        ),
        body.len(),
        headers,
        body
    );
    socket
        .write_all(request.as_bytes())
        .unwrap_or_else(|error| panic!("request write: {error}"));
    let mut received = Vec::new();
    let mut chunk = [0_u8; 256];
    while !String::from_utf8_lossy(&received).contains(marker) {
        let count = socket
            .read(&mut chunk)
            .unwrap_or_else(|error| panic!("response read: {error}"));
        assert_ne!(count, 0, "response ended before {marker}");
        received.extend_from_slice(&chunk[..count]);
    }
    socket
}

fn response_id(response: &crate::openai::test_support::HttpResponse) -> String {
    let value: Value = serde_json::from_str(&response.body)
        .unwrap_or_else(|error| panic!("response JSON: {error}; {}", response.body));
    value["id"]
        .as_str()
        .unwrap_or_else(|| panic!("response id"))
        .to_owned()
}

mod cursor {
    use super::*;
    use base64::Engine as _;

    #[tokio::test(flavor = "multi_thread")]
    async fn stored_response_cursor_has_four_unsigned_fields_over_tcp() {
        let roots = roots("responses-cursor");
        let agent_id = "agent-local-response-cursor";
        seed(&roots.storage, &[agent(agent_id, "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut listener = launch_with_runtime(&roots, true, None, runtime).await;
        let response = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"retain","store":true}"#,
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.body);
        let id = response_id(&response);
        let payload = id.strip_prefix("resp_letta_").unwrap();
        assert!(!payload.contains('='));
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap();
        let fields: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(fields.as_object().unwrap().len(), 4);
        assert_eq!(fields["version"], 1);
        assert_eq!(fields["agent_id"], agent_id);
        assert!(uuid::Uuid::parse_str(fields["nonce"].as_str().unwrap()).is_ok());
        assert!(!fields["conversation_id"].as_str().unwrap().is_empty());
        listener.shutdown();
        listener.wait().await.unwrap();
    }
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

    #[tokio::test(flavor = "multi_thread")]
    async fn ordinary_response_id_is_not_a_stored_cursor() {
        let roots = roots("responses-nonstored-id");
        seed(&roots.storage, &[agent("agent-local-response-id", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut listener = launch_with_runtime(&roots, true, None, runtime).await;
        let response = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","store":false}"#,
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert!(response_id(&response).starts_with("resp_"));
        assert!(!response_id(&response).starts_with("resp_letta_"));
        listener.shutdown();
        listener.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn headerless_ephemeral_artifact_is_deleted() {
        let roots = roots("responses-nonstored-delete");
        seed(&roots.storage, &[agent("agent-local-response-delete", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut listener = launch_with_runtime(&roots, true, None, runtime.clone()).await;
        let response = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","store":false}"#,
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 1);
        assert_eq!(conversations(&roots.storage), 0);
        listener.shutdown();
        listener.wait().await.unwrap();
    }
}

mod no_idempotency {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn idempotency_header_ignored() {
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
    async fn failed_outcome_not_stored() {
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
    async fn creates_hidden_fork() {
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
            "previous_response_id":first_id.clone()
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

        let ephemeral_body = serde_json::json!({
            "model":"memo","input":"temporary branch","store":false,
            "previous_response_id":first_id
        })
        .to_string();
        let ephemeral = post(&handle, "/v1/responses", "", &ephemeral_body).await;
        assert_eq!(ephemeral.status, 200, "{}", ephemeral.body);
        assert!(!response_id(&ephemeral).starts_with("resp_letta_"));
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 6);
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 1);
        let retained = std::fs::read_dir(roots.storage.join("conversations"))
            .map_or(0, std::iter::Iterator::count);
        assert_eq!(retained, 2);
        handle.shutdown();
        handle.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unsupported_backend_501_precedes_repository_and_execution() {
        let roots = roots("responses-unsupported-repository");
        seed(&roots.storage, &[agent("agent-local-response-unsupported", "memo", false)]).await;
        let token = "responses-capability-token";
        let first_runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut first_listener =
            launch_with_runtime(&roots, true, Some(token), first_runtime).await;
        let auth = format!("Authorization: Bearer {token}\r\n");
        let stored = post(
            &first_listener,
            "/v1/responses",
            &auth,
            r#"{"model":"memo","input":"stored","store":true}"#,
        )
        .await;
        assert_eq!(stored.status, 200, "{}", stored.body);
        let cursor = response_id(&stored);
        assert!(cursor.starts_with("resp_letta_"));
        first_listener.shutdown();
        first_listener.wait().await.unwrap();

        let repository = Arc::new(UnsupportedRepository::default());
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(crate::ws::ServiceBackedTurnController::new(runtime.clone()));
        let repository_port: Arc<dyn crate::ws::conversations::ConversationCommandRepository> =
            repository.clone();
        let (mut listener, state) = launch_responses_custom(
            &roots,
            Some(token),
            runtime.clone(),
            controller,
            Some(repository_port),
            2,
        )
        .await;
        let before = conversations(&roots.storage);
        let body = json!({
            "model":"memo","input":"must not execute","previous_response_id":cursor
        })
        .to_string();
        let response = post(&listener, "/v1/responses", &auth, &body).await;
        assert_eq!(response.status, 501);
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            response.body,
            concat!(
                r#"{"error":{"message":"previous_response_id is unavailable because this backend "#,
                r#"cannot fork conversations","type":"server_error","param":null,"#,
                r#""code":"unsupported_backend"}}"#
            )
        );
        assert_eq!(repository.lookups.load(Ordering::SeqCst), 0);
        assert_eq!(repository.forks.load(Ordering::SeqCst), 0);
        assert_eq!(repository.deletes.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
        assert_eq!(conversations(&roots.storage), before);
        assert_eq!(state.owner_count().await, 0);
        assert_eq!(state.setup_lock_count(), 0);
        listener.shutdown();
        listener.wait().await.unwrap();
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

mod setup_lock_recovery {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn real_tcp_allocation_panic_releases_waiter_prunes_lock_and_retries() {
        let roots = roots("responses-setup-panic");
        let agent_id = "agent-local-responses-setup-panic";
        seed(&roots.storage, &[agent(agent_id, "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(crate::ws::ServiceBackedTurnController::new(runtime.clone()));
        let (mut listener, state) = launch_responses_custom(
            &roots, None, runtime.clone(), controller, None, 3,
        ).await;
        let (entered, release) =
            crate::ws::conversations::gate_next_openai_create_panic(agent_id);
        let address = listener.address();
        let body = r#"{"model":"memo","input":"serialized","store":false}"#;
        let owner = tokio::spawn(post_at(address, "/v1/responses", "", body));
        tokio::time::timeout(Duration::from_secs(5), entered.acquire())
            .await.unwrap().unwrap().forget();
        let waiter = tokio::spawn(post_at(address, "/v1/responses", "", body));
        tokio::task::yield_now().await;
        assert_eq!(state.setup_lock_count(), 1);
        release.add_permits(1);
        let owner = owner.await.unwrap();
        let waiter = waiter.await.unwrap();
        assert_eq!(owner.status, 500, "{}", owner.body);
        assert_eq!(waiter.status, 200, "{}", waiter.body);
        wait_clean(&state).await;
        assert_eq!(state.setup_lock_count(), 0);
        assert_eq!(state.owner_count().await, 0);

        let retry = post(&listener, "/v1/responses", "", body).await;
        assert_eq!(retry.status, 200, "{}", retry.body);
        wait_clean(&state).await;
        assert_eq!(state.setup_lock_count(), 0);
        assert_eq!(conversations(&roots.storage), 0);
        listener.shutdown();
        listener.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_during_repository_setup_drops_lock_and_joins_owner() {
        let roots = roots("responses-setup-shutdown");
        let agent_id = "agent-local-responses-setup-shutdown";
        seed(&roots.storage, &[agent(agent_id, "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(crate::ws::ServiceBackedTurnController::new(runtime.clone()));
        let (mut listener, state) =
            launch_responses_custom(&roots, None, runtime, controller, None, 1).await;
        let (entered, _release) =
            crate::ws::conversations::gate_next_openai_create_panic(agent_id);
        let address = listener.address();
        let request = tokio::spawn(post_at(
            address,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"shutdown","store":false}"#,
        ));
        tokio::time::timeout(Duration::from_secs(5), entered.acquire())
            .await.unwrap().unwrap().forget();
        assert_eq!(state.setup_lock_count(), 1);
        listener.shutdown();
        let response = tokio::time::timeout(Duration::from_secs(5), request)
            .await.unwrap().unwrap();
        assert_eq!(response.status, 500, "{}", response.body);
        tokio::time::timeout(Duration::from_secs(5), listener.wait())
            .await.unwrap().unwrap();
        assert_eq!(state.setup_lock_count(), 0);
        assert_eq!(state.owner_count().await, 0);
        assert_eq!(conversations(&roots.storage), 0);
    }
}

mod streaming {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn named_owner_cap_bounds_all_active_requests_and_recovers_immediately() {
        const TEST_OWNERS_MAX: usize = 2;
        assert_eq!(super::execution::OPENAI_RESPONSES_OWNERS_MAX, 128);
        let roots = roots("responses-owner-cap");
        seed(&roots.storage, &[agent("agent-local-response-owner-cap", "memo", false)]).await;
        let token = "responses-owner-cap-token";
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(GatedController::new(TEST_OWNERS_MAX, 0));
        let (mut listener, state) = launch_responses_custom(
            &roots,
            Some(token),
            runtime.clone(),
            controller.clone(),
            None,
            TEST_OWNERS_MAX,
        )
        .await;
        let address = listener.address();
        let auth = format!("Authorization: Bearer {token}\r\n");
        let body = r#"{"model":"memo","input":"held","store":false}"#;
        let first_auth = auth.clone();
        let mut first = tokio::spawn(async move {
            post_at(address, "/v1/responses", &first_auth, body).await
        });
        tokio::select! {
            () = controller.wait_for(1) => {}
            response = &mut first => {
                let response = response.unwrap();
                panic!("first owner settled early: {} {}", response.status, response.body);
            }
        }
        assert_eq!(state.owner_count().await, TEST_OWNERS_MAX - 1);
        let second_auth = auth.clone();
        let second = tokio::spawn(async move {
            post_at(address, "/v1/responses", &second_auth, body).await
        });
        controller.wait_for(TEST_OWNERS_MAX).await;
        assert_eq!(state.owner_count().await, TEST_OWNERS_MAX);
        let calls_at_capacity = runtime.calls.load(Ordering::SeqCst);
        let artifacts_at_capacity = conversations(&roots.storage);

        let rejected = post_at(address, "/v1/responses", &auth, body).await;
        assert_eq!(rejected.status, 503);
        assert_eq!(rejected.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            rejected.body,
            concat!(
                r#"{"error":{"message":"Responses execution capacity unavailable","#,
                r#""type":"server_error","param":null,"code":null}}"#
            )
        );
        assert_eq!(runtime.calls.load(Ordering::SeqCst), calls_at_capacity);
        assert_eq!(conversations(&roots.storage), artifacts_at_capacity);
        assert_eq!(state.owner_count().await, TEST_OWNERS_MAX);
        assert_eq!(controller.exited.load(Ordering::SeqCst), 0);

        controller.release(TEST_OWNERS_MAX);
        assert_eq!(first.await.unwrap().status, 200);
        assert_eq!(second.await.unwrap().status, 200);
        let subsequent = post_at(address, "/v1/responses", &auth, body).await;
        assert_eq!(subsequent.status, 200, "{}", subsequent.body);
        assert_eq!(controller.exited.load(Ordering::SeqCst), TEST_OWNERS_MAX);
        assert_eq!(state.owner_count().await, 0);
        assert_eq!(state.setup_lock_count(), 0);
        assert_eq!(conversations(&roots.storage), 0);
        listener.shutdown();
        listener.wait().await.unwrap();
        assert_eq!(state.owner_count().await, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unread_backpressured_sse_does_not_own_execution_or_block_unrelated_response() {
        let roots = roots("responses-unread-backpressure");
        seed(&roots.storage, &[agent("agent-local-response-unread", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(GatedController::new(1, 4_096));
        let (mut listener, state) = launch_responses_custom(
            &roots,
            None,
            runtime.clone(),
            controller.clone(),
            None,
            2,
        )
        .await;
        let address = listener.address();
        let socket = tokio::task::spawn_blocking(move || {
            open_stream(
                address,
                "",
                r#"{"model":"memo","input":"many","stream":true}"#,
                "response.in_progress",
            )
        })
        .await
        .unwrap();
        controller.wait_for(1).await;
        assert_eq!(state.owner_count().await, 1);

        let unrelated = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"independent"}"#,
        )
        .await;
        assert_eq!(unrelated.status, 200, "{}", unrelated.body);
        assert_eq!(state.owner_count().await, 1);
        controller.release(1);
        wait_clean(&state).await;
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 2);
        assert_eq!(conversations(&roots.storage), 0);
        drop(socket);
        listener.shutdown();
        tokio::time::timeout(Duration::from_secs(10), listener.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.owner_count().await, 0);
        assert_eq!(state.setup_lock_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sse_disconnect_then_shutdown_cancels_and_joins_backpressured_owner() {
        let roots = roots("responses-sse-shutdown");
        seed(&roots.storage, &[agent("agent-local-response-sse-shutdown", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let controller = Arc::new(GatedController::new(1, 4_096));
        let (mut listener, state) = launch_responses_custom(
            &roots,
            None,
            runtime.clone(),
            controller.clone(),
            None,
            1,
        )
        .await;
        let address = listener.address();
        let socket = tokio::task::spawn_blocking(move || {
            open_stream(
                address,
                "",
                r#"{"model":"memo","input":"shutdown","stream":true}"#,
                "response.created",
            )
        })
        .await
        .unwrap();
        controller.wait_for(1).await;
        assert_eq!(state.owner_count().await, 1);
        listener.shutdown();
        drop(socket);
        tokio::time::timeout(Duration::from_secs(10), listener.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(controller.exited.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.ephemeral_teardowns.load(Ordering::SeqCst), 1);
        assert_eq!(state.owner_count().await, 0);
        assert_eq!(state.setup_lock_count(), 0);
        assert_eq!(conversations(&roots.storage), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_terminal_state() {
        let roots = roots("responses-stream-completed");
        seed(&roots.storage, &[agent("agent-local-response-completed", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut listener = launch_with_runtime(&roots, true, None, runtime).await;
        let response = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","stream":true}"#,
        )
        .await;
        assert_eq!(response.status, 200);
        assert!(response.body.contains("response.completed"));
        assert!(!response.body.contains("response.failed"));
        listener.shutdown();
        listener.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_terminal_state() {
        let roots = roots("responses-stream-failed-terminal");
        seed(&roots.storage, &[agent("agent-local-response-failed-terminal", "memo", false)]).await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut listener = launch_with_runtime_and_controller(
            &roots,
            true,
            None,
            runtime,
            Arc::new(FailingController),
        )
        .await;
        let response = post(
            &listener,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","stream":true}"#,
        )
        .await;
        assert_eq!(response.status, 200);
        assert!(response.body.contains("response.failed"));
        assert!(!response.body.contains("response.completed"));
        listener.shutdown();
        listener.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_names_follow_openai_without_done_sentinel() {
        let stream_roots = roots("responses-stream");
        seed(
            &stream_roots.storage,
            &[agent("agent-local-response-stream", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut handle = launch_with_runtime(&stream_roots, true, None, runtime).await;
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

        let failed_roots = roots("responses-stream-failed");
        seed(
            &failed_roots.storage,
            &[agent("agent-local-response-stream-failed", "memo", false)],
        )
        .await;
        let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
        let mut failed_handle = launch_with_runtime_and_controller(
            &failed_roots,
            true,
            None,
            runtime,
            Arc::new(FailingController),
        )
        .await;
        let failed = post(
            &failed_handle,
            "/v1/responses",
            "",
            r#"{"model":"memo","input":"hello","stream":true}"#,
        )
        .await;
        assert_eq!(failed.status, 200);
        assert!(failed.body.contains("response.created"));
        assert!(failed.body.contains("response.failed"));
        assert!(!failed.body.contains("response.completed"));
        assert!(!failed.body.contains("[DONE]"));
        failed_handle.shutdown();
        failed_handle.wait().await.unwrap();
    }
}
