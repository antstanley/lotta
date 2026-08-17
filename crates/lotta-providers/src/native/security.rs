use super::{anthropic::Anthropic, openai_compatible::OpenAiCompatible, shared};
use reqwest::Url;

#[test]
fn loopback_ipv4_and_ipv6_are_allowed_but_nonloopback_http_is_rejected() {
    for endpoint in ["http://127.0.0.1:1/v1", "http://[::1]:1/v1"] {
        let url = Url::parse(endpoint).expect("URL");
        assert!(OpenAiCompatible::new(&url, "secret").is_ok());
        assert!(Anthropic::new(&url, "secret").is_ok());
    }
    let url = Url::parse("http://192.0.2.1/v1").expect("URL");
    assert!(OpenAiCompatible::new(&url, "secret").is_err());
    assert!(Anthropic::new(&url, "secret").is_err());
}

#[test]
fn credentials_and_image_signatures_are_excluded_from_diagnostics() {
    let secret = "sk-task49-super-secret";
    let url = Url::parse("http://127.0.0.1:1/v1").expect("URL");
    let openai = OpenAiCompatible::new(&url, secret).expect("adapter");
    let anthropic = Anthropic::new(&url, secret).expect("adapter");
    assert!(!format!("{openai:?}").contains(secret));
    assert!(!format!("{anthropic:?}").contains(secret));
    let diagnostic = shared::sanitize("Bearer sk-task49-super-secret image=89504e47");
    assert!(!diagnostic.contains(secret));
}

#[test]
fn endpoint_composition_discards_query_and_fragment() {
    let base = Url::parse("https://example.invalid/v1?key=secret#fragment").expect("URL");
    let endpoint = shared::endpoint(&base, "messages");
    assert_eq!(endpoint.as_str(), "https://example.invalid/v1/messages");
}
