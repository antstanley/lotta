use super::test_support::*;

#[test]
fn handle_display_and_serde_are_exact() {
    let value = handle("gpt-4.1");
    assert_eq!(value.to_string(), "openai/gpt-4.1");
    assert_eq!(
        serde_json::to_string(&value).unwrap_or_else(|error| panic!("serialize: {error}")),
        "\"openai/gpt-4.1\""
    );
    assert_eq!(
        serde_json::from_str::<ModelHandle>("\"openai/gpt-4.1\"")
            .unwrap_or_else(|error| panic!("deserialize: {error}")),
        value
    );
}
#[test]
fn handle_accepts_pinned_nested_model_ids() {
    for value in [
        "openrouter/deepseek/deepseek-v4-pro",
        "fireworks/accounts/fireworks/models/llama-v3p1-405b-instruct",
        "cloudflare-ai-gateway/workers-ai/@cf/meta/llama-3.3-70b-instruct-fp8-fast",
    ] {
        let parsed = value
            .parse::<ModelHandle>()
            .unwrap_or_else(|error| panic!("parse {value}: {error}"));
        assert_eq!(parsed.to_string(), value);
        assert_eq!(
            serde_json::from_value::<ModelHandle>(json!(value))
                .unwrap_or_else(|error| panic!("serde {value}: {error}")),
            parsed
        );
    }
}

#[test]
fn handle_rejects_malformed_paths_and_controls() {
    for value in ["/model", "provider/", "provider/a//b", "provider/a\0b"] {
        assert!(value.parse::<ModelHandle>().is_err(), "accepted {value:?}");
    }
    assert!(ModelHandle::new("https://api.example", "m").is_err());
}
