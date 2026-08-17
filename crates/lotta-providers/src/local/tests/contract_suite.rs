use super::llama_cpp::LlamaCpp;
use super::lmstudio::LmStudio;
use super::ollama::{Ollama, OllamaCloud};
use crate::native::loopback::{Loopback, ResponseScript};
use lotta_runtime::ports::ProviderRequest;
use lotta_testkit::contract::fixtures::provider_request;
use lotta_testkit::contract::{
    ProviderContractScenario, ProviderContractShape, provider_contract,
    provider_contract_with_shape,
};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"hello\",\"reasoning_content\":\"think\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"redacted_reasoning\":\"hidden\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"tool\",\"arguments\":\"{}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3}}\n\n",
    "data: {\"id\":\"id\",\"model\":\"model\",\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":5}}\n\n",
    "data: [DONE]\n\n"
);
const ERROR: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
    "data: {\"error\":{\"type\":\"protocol_error\",\"code\":\"protocol\",\"message\":\"contract\"}}\n\n"
);
const OLLAMA_SUCCESS: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: 293\r\nconnection: close\r\n\r\n{\"message\":{\"content\":\"hello\"},\"done\":false}\n{\"message\":{\"thinking\":\"think\",\"tool_calls\":[{\"id\":\"call-1\",\"function\":{\"name\":\"tool\",\"arguments\":{}}}]},\"done\":false}\n{\"message\":{},\"done\":false,\"prompt_eval_count\":2,\"eval_count\":3}\n{\"message\":{},\"done\":true,\"prompt_eval_count\":3,\"eval_count\":5}\n";
const OLLAMA_ERROR: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: 102\r\nconnection: close\r\n\r\n{\"message\":{\"content\":\"hello\"},\"done\":false}\n{\"error\":{\"type\":\"protocol_error\",\"message\":\"contract\"}}\n";

fn request() -> ProviderRequest {
    provider_request(CancellationToken::new())
}

async fn server(scenario: ProviderContractScenario, success: &str, error: &str) -> Loopback {
    match scenario {
        ProviderContractScenario::Success => {
            Loopback::scripted(ResponseScript::Complete(success.as_bytes().to_vec())).await
        }
        ProviderContractScenario::TerminalError => {
            Loopback::scripted(ResponseScript::Complete(error.as_bytes().to_vec())).await
        }
        ProviderContractScenario::Cancelled | ProviderContractScenario::ReceiverClosed => {
            Loopback::scripted(ResponseScript::WaitForNotify {
                prefix: Vec::new(),
                notify: Arc::new(Notify::new()),
            })
            .await
        }
    }
}

#[tokio::test]
async fn ollama_invokes_provider_contract_with_shape() {
    provider_contract_with_shape(
        |scenario| async move {
            let server = server(scenario, OLLAMA_SUCCESS, OLLAMA_ERROR).await;
            Ollama::new(server.base(), "").expect("adapter")
        },
        request(),
        ProviderContractShape::OLLAMA,
    )
    .await;
}

#[tokio::test]
async fn ollama_cloud_invokes_provider_contract_with_shape() {
    provider_contract_with_shape(
        |scenario| async move {
            let server = server(scenario, OLLAMA_SUCCESS, OLLAMA_ERROR).await;
            OllamaCloud::with_test_core(server.base(), "contract-credential", false)
                .expect("adapter")
        },
        request(),
        ProviderContractShape::OLLAMA,
    )
    .await;
}

#[tokio::test]
async fn lmstudio_invokes_provider_contract() {
    provider_contract(
        |scenario| async move {
            let server = server(scenario, SUCCESS, ERROR).await;
            LmStudio::new(server.base(), "").expect("adapter")
        },
        request(),
    )
    .await;
}

#[tokio::test]
async fn llama_cpp_invokes_provider_contract() {
    provider_contract(
        |scenario| async move {
            let server = server(scenario, SUCCESS, ERROR).await;
            LlamaCpp::new(server.base(), "").expect("adapter")
        },
        request(),
    )
    .await;
}
