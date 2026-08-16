use super::framing::{read_frame, write_frame};
use super::*;
use serde_json::Value;

#[tokio::test]
async fn provider_mod_and_caller_bounded_profiles_share_one_codec() {
    for limit in [
        SidecarFrameLimit::provider_host(),
        SidecarFrameLimit::mod_host(),
        SidecarFrameLimit::bounded(64 * 1024),
    ] {
        let mut wire = Vec::new();
        write_frame(&mut wire, limit, &serde_json::json!({"profile": "shared"}))
            .await
            .unwrap();
        let decoded: Value = read_frame(&mut std::io::Cursor::new(wire), limit)
            .await
            .unwrap();
        assert_eq!(decoded["profile"], "shared");
    }
}

#[test]
fn no_second_framing_implementation_in_future_consumers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for module in ["mods", "subagents", "provider_host"] {
        let path = root.join(module);
        if !path.exists() {
            continue;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(path).unwrap();
            assert!(
                !source.contains("u32::from_be_bytes") && !source.contains("to_be_bytes()"),
                "second framing implementation"
            );
        }
    }
}
