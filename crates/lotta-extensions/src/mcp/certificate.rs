use super::{client::McpServerConfig, oauth::SecretValue};
use serde_json::json;

pub mod credentials {
    use super::*;

    #[test]
    fn credentials() {
        let marker = "marker-secret";
        let secret = SecretValue::new(marker.as_bytes().to_vec()).unwrap();
        assert!(!format!("{secret:?} {secret}").contains(marker));
        for url in [
            "https://u:p@a.test/mcp",
            "https://a.test/mcp?q=x",
            "https://a.test/mcp#x",
        ] {
            assert!(
                serde_json::from_value::<McpServerConfig>(
                    json!({"type":"http","name":"x","url":url})
                )
                .is_err()
            );
        }
    }
}
