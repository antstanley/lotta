use super::anthropic::Anthropic;
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::OpenAiCompatible;
use lotta_runtime::ports::ProviderRequest;
use lotta_testkit::contract::fixtures::provider_request;
use lotta_testkit::contract::{ProviderContractScenario, provider_contract};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const OPENAI_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\",\"reasoning_content\":\"think\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"redacted_reasoning\":\"hidden\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"tool\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"prompt_tokens_details\":{\"cached_tokens\":1},\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
    "data: {\"id\":\"contract-id\",\"model\":\"contract-model\",\"system_fingerprint\":\"contract-fingerprint\",\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":5,\"prompt_tokens_details\":{\"cached_tokens\":1},\"completion_tokens_details\":{\"reasoning_tokens\":2}}}\n\n",
    "data: [DONE]\n\n"
);
const OPENAI_ERROR: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
    "data: {\"error\":{\"type\":\"protocol_error\",\"code\":\"protocol\",\"message\":\"contract\"}}\n\n"
);
const ANTHROPIC_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"contract-id\",\"model\":\"contract-model\",\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":1}}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"think\"}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"opaque\"}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call-1\",\"name\":\"tool\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":3}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0,\"reasoning_tokens\":0}}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":3,\"reasoning_tokens\":1}}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5,\"reasoning_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
);
const ANTHROPIC_ERROR: &str = concat!(
    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
    "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"protocol_error\",\"code\":\"protocol\",\"message\":\"contract\"}}\n\n"
);

fn request() -> ProviderRequest {
    provider_request(CancellationToken::new())
}

async fn script(scenario: ProviderContractScenario, success: &str, error: &str) -> Loopback {
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
async fn openai_compatible_passes_provider_contract() {
    provider_contract(
        |scenario| async move {
            let server = script(scenario, OPENAI_SUCCESS, OPENAI_ERROR).await;
            OpenAiCompatible::new(server.base(), "contract-credential").expect("adapter")
        },
        request(),
    )
    .await;
}

#[tokio::test]
async fn anthropic_passes_provider_contract() {
    provider_contract(
        |scenario| async move {
            let server = script(scenario, ANTHROPIC_SUCCESS, ANTHROPIC_ERROR).await;
            Anthropic::new(server.base(), "contract-credential").expect("adapter")
        },
        request(),
    )
    .await;
}
