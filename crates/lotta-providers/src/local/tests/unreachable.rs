use super::llama_cpp::LlamaCpp;
use super::lmstudio::LmStudio;
use super::ollama::{Ollama, OllamaCloud};
use super::{DiscoveryClient, DiscoveryDialect};
use lotta_runtime::ports::{ProviderError, ProviderEvent, ProviderPort};
use lotta_testkit::contract::fixtures::provider_request;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn adapter_unavailable(adapter: impl ProviderPort) {
    let request = provider_request(CancellationToken::new());
    let (sink, mut receiver) =
        lotta_runtime::ports::provider_event_channel(8, &request.cancellation).expect("channel");
    adapter
        .stream(request, sink)
        .await
        .expect("stream boundary");
    assert!(matches!(
        receiver.receive().await.expect("receive"),
        Some(ProviderEvent::Error {
            error: ProviderError::Unavailable(_)
        })
    ));
    assert!(receiver.receive().await.expect("terminal").is_none());
}

#[tokio::test]
// Process readiness is structurally independent: the app server owns `/readyz` as a constant
// handler and provider adapters have no reference to listener state. Task 84 will add stronger
// readiness inputs; until then this test proves the complete provider-side outage behavior.
async fn all_public_local_adapters_map_connection_refused_once() {
    let endpoint = reqwest::Url::parse("http://127.0.0.1:1").expect("URL");
    adapter_unavailable(Ollama::new(&endpoint, "").expect("Ollama")).await;
    adapter_unavailable(LmStudio::new(&endpoint, "").expect("LM Studio")).await;
    adapter_unavailable(LlamaCpp::new(&endpoint, "").expect("llama.cpp")).await;
    let cloud = reqwest::Url::parse("https://127.0.0.1:1").expect("URL");
    adapter_unavailable(OllamaCloud::new(&cloud, "credential").expect("Cloud")).await;
}

#[test]
fn endpoint_security_policy_is_shared_by_all_local_paths() {
    for endpoint in [
        "http://127.0.0.1:11434",
        "http://[::1]:11434",
        "http://192.168.1.20:11434",
        "http://[fd00::1]:11434",
    ] {
        let url = reqwest::Url::parse(endpoint).expect("URL");
        assert!(Ollama::new(&url, "").is_ok(), "Ollama {endpoint}");
        assert!(LmStudio::new(&url, "").is_ok(), "LM Studio {endpoint}");
        assert!(LlamaCpp::new(&url, "").is_ok(), "llama.cpp {endpoint}");
        assert!(DiscoveryClient::new(&url, "", DiscoveryDialect::Ollama).is_ok());
        let credentialed_allowed =
            matches!(endpoint, "http://127.0.0.1:11434" | "http://[::1]:11434");
        assert_eq!(
            Ollama::new(&url, "credential").is_ok(),
            credentialed_allowed
        );
        assert_eq!(
            LmStudio::new(&url, "credential").is_ok(),
            credentialed_allowed
        );
    }
    let cloud = reqwest::Url::parse("https://ollama.com").expect("URL");
    assert!(OllamaCloud::new(&cloud, "credential").is_ok());
}

#[tokio::test]
async fn all_discovery_dialects_map_connection_refused() {
    for dialect in [
        DiscoveryDialect::Ollama,
        DiscoveryDialect::OllamaCloud,
        DiscoveryDialect::LmStudio,
        DiscoveryDialect::LlamaCpp,
    ] {
        let (endpoint, credential) = if dialect == DiscoveryDialect::OllamaCloud {
            ("https://127.0.0.1:1", "credential")
        } else {
            ("http://127.0.0.1:1", "")
        };
        let endpoint = reqwest::Url::parse(endpoint).expect("URL");
        let client = DiscoveryClient::new(&endpoint, credential, dialect).expect("discovery");
        assert!(matches!(
            client
                .discover(&CancellationToken::new(), Duration::from_secs(1))
                .await,
            Err(ProviderError::Unavailable(_))
        ));
    }
}
