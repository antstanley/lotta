use super::tests::replay_case;

#[tokio::test]
async fn openai_compatible_happy_tool() {
    replay_case("openai-compatible/happy-tool").await;
}

#[tokio::test]
async fn anthropic_reasoning_redacted() {
    replay_case("anthropic/reasoning-redacted").await;
}

#[tokio::test]
async fn openai_compatible_retry_after() {
    replay_case("openai-compatible/retry-after").await;
}

#[tokio::test]
async fn anthropic_authentication() {
    replay_case("anthropic/authentication").await;
}

#[tokio::test]
async fn openai_compatible_authorization() {
    replay_case("openai-compatible/authorization").await;
}

#[tokio::test]
async fn anthropic_quota() {
    replay_case("anthropic/quota").await;
}

#[tokio::test]
async fn openai_compatible_protocol_error() {
    replay_case("openai-compatible/protocol-error").await;
}
