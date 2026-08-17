use super::{DiscoveryClient, DiscoveryDialect};
use crate::native::loopback::{Loopback, ResponseScript};
use lotta_runtime::ports::ProviderError;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn complete(status: u16, body: &str) -> ResponseScript {
    ResponseScript::Complete(
        format!(
            "HTTP/1.1 {status} TEST\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

async fn discover(dialect: DiscoveryDialect, scripts: Vec<ResponseScript>) -> Vec<String> {
    discover_with_record(dialect, scripts).await.0
}

async fn discover_with_record(
    dialect: DiscoveryDialect,
    scripts: Vec<ResponseScript>,
) -> (Vec<String>, crate::native::loopback::RecordedRequest) {
    let server = Loopback::sequence(scripts).await;
    let client = DiscoveryClient::new(server.base(), "", dialect).expect("client");
    let models = client
        .discover(&CancellationToken::new(), Duration::from_secs(1))
        .await
        .expect("discover");
    let recorded = server.recorded().await;
    let ids = models
        .into_iter()
        .map(|model| model.id.as_str().to_owned())
        .collect();
    (ids, recorded)
}

#[tokio::test]
async fn ollama_cloud_discovers_over_authenticated_test_core() {
    let server = Loopback::sequence([
        complete(200, r#"{"models":[{"name":"cloud-model"}]}"#),
        complete(200, r#"{"capabilities":["tools"]}"#),
    ])
    .await;
    let client = DiscoveryClient::with_test_core(
        server.base(),
        "cloud-credential",
        DiscoveryDialect::OllamaCloud,
    )
    .expect("client");
    let models = client
        .discover(&CancellationToken::new(), Duration::from_secs(1))
        .await
        .expect("discover");
    assert_eq!(models[0].id.as_str(), "cloud-model");
    let recorded = server.recorded().await;
    assert!(
        recorded
            .headers
            .iter()
            .any(|(name, value)| { name == "authorization" && value == "Bearer cloud-credential" })
    );
}

#[tokio::test]
async fn ollama_tags_show_sort_and_dedupe() {
    let ids = discover(
        DiscoveryDialect::Ollama,
        vec![
            complete(
                200,
                r#"{"models":[{"name":"z"},{"name":"a"},{"name":"a"}]}"#,
            ),
            complete(200, r#"{"capabilities":["vision"]}"#),
            complete(200, r#"{"capabilities":["tools"]}"#),
            complete(200, r#"{"capabilities":["tools"]}"#),
        ],
    )
    .await;
    assert_eq!(ids, ["a", "z"]);
}

#[test]
fn ollama_cloud_requires_tls_authentication() {
    let endpoint = reqwest::Url::parse("http://127.0.0.1:1").expect("URL");
    assert!(DiscoveryClient::new(&endpoint, "", DiscoveryDialect::OllamaCloud).is_err());
}

#[tokio::test]
async fn lmstudio_lists_downloaded_chat_models_and_openai_fallback() {
    let native = discover(
        DiscoveryDialect::LmStudio,
        vec![complete(
            200,
            r#"{"data":[{"id":"chat","type":"vlm","state":"not-loaded","loaded_context_length":2048,"max_context_length":8192},{"id":"embed","type":"embeddings"}]}"#,
        )],
    )
    .await;
    assert_eq!(native, ["chat"]);
    let (fallback, recorded) = discover_with_record(
        DiscoveryDialect::LmStudio,
        vec![
            complete(404, "{}"),
            complete(200, r#"{"data":[{"id":"fallback"}]}"#),
        ],
    )
    .await;
    assert_eq!(fallback, ["fallback"]);
    assert_eq!(recorded.target, "/v1/models");
}

#[tokio::test]
async fn llama_native_props_and_openai_fallback() {
    let (native, recorded) = discover_with_record(
        DiscoveryDialect::LlamaCpp,
        vec![
            complete(
                200,
                r#"{"models":[{"id":"router/name","meta":{"n_ctx":8192,"architecture":"llama","quantization":"Q4"},"architecture":{"input_modalities":["text","image"]}}]}"#,
            ),
            complete(200, r#"{"default_generation_settings":{"n_ctx":4096}}"#),
        ],
    )
    .await;
    assert_eq!(native, ["router/name"]);
    assert_eq!(recorded.target, "/props?model=router%2Fname");
    let fallback = discover(
        DiscoveryDialect::LlamaCpp,
        vec![
            complete(404, "{}"),
            complete(200, r#"{"data":[{"id":"fallback"}]}"#),
            complete(404, "{}"),
        ],
    )
    .await;
    assert_eq!(fallback, ["fallback"]);
}

#[tokio::test]
async fn llama_status_object_filters_failed_unloaded_models() {
    let ids = discover(
        DiscoveryDialect::LlamaCpp,
        vec![complete(
            200,
            concat!(
                r#"{"models":["#,
                r#"{"id":"loaded","status":{"value":"loaded","failed":false}},"#,
                r#"{"id":"sleeping","status":{"value":"sleeping","failed":false}},"#,
                r#"{"id":"loadable","status":{"value":"unloaded","failed":false}},"#,
                r#"{"id":"failed","status":{"value":"unloaded","failed":true}}]}"#,
            ),
        )],
    )
    .await;
    assert_eq!(ids, ["loadable", "loaded", "sleeping"]);
}

#[tokio::test]
async fn ollama_show_failure_degrades_only_that_model() {
    let ids = discover(
        DiscoveryDialect::Ollama,
        vec![
            complete(200, r#"{"models":[{"name":"a"},{"name":"b"}]}"#),
            complete(500, "{}"),
            complete(200, r#"{"capabilities":["vision"]}"#),
        ],
    )
    .await;
    assert_eq!(ids, ["a", "b"]);
}

#[tokio::test]
async fn malformed_cancel_deadline_and_status_are_typed() {
    let server = Loopback::scripted(complete(200, "{}")).await;
    let client = DiscoveryClient::new(server.base(), "", DiscoveryDialect::Ollama).expect("client");
    assert!(matches!(
        client
            .discover(&CancellationToken::new(), Duration::from_secs(1))
            .await,
        Err(ProviderError::Protocol(_))
    ));
    let _ = server.recorded().await;
    let endpoint = reqwest::Url::parse("http://127.0.0.1:1").expect("URL");
    let client = DiscoveryClient::new(&endpoint, "", DiscoveryDialect::Ollama).expect("client");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        client.discover(&cancelled, Duration::from_secs(1)).await,
        Err(ProviderError::Cancelled(_))
    ));
    assert!(matches!(
        client
            .discover(&CancellationToken::new(), Duration::ZERO)
            .await,
        Err(ProviderError::Timeout(_) | ProviderError::Unavailable(_))
    ));
    let server = Loopback::scripted(complete(503, "{}")).await;
    let client =
        DiscoveryClient::new(server.base(), "", DiscoveryDialect::LmStudio).expect("client");
    assert!(matches!(
        client
            .discover(&CancellationToken::new(), Duration::from_secs(1))
            .await,
        Err(ProviderError::Unavailable(_))
    ));
    let _ = server.recorded().await;
}
