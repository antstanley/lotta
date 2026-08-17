use super::client::HostClient;
use super::tests::{owner, test_config};

#[tokio::test]
async fn child_receives_only_explicit_argv_env_and_empty_scratch() {
    let (config, scratch) = test_config();
    let expected_argv = vec![
        config.host_script.display().to_string(),
        config.package_root.display().to_string(),
    ];
    let mut client = HostClient::spawn(config, owner()).await.unwrap();
    let snapshot = client.isolation_snapshot().await.unwrap();
    assert_eq!(
        snapshot["argv"].as_array().unwrap(),
        &expected_argv
            .into_iter()
            .map(serde_json::Value::String)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshot["env_keys"].as_array().unwrap(),
        &["LOTTA_HOST_TEST_MODE", "NO_COLOR"]
            .into_iter()
            .map(|value| serde_json::Value::String(value.into()))
            .collect::<Vec<_>>()
    );
    assert!(snapshot["cwd_entries"].as_array().unwrap().is_empty());
    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn framing_is_reused_and_ambient_roots_are_absent() {
    let source = include_str!("client.rs");
    assert!(source.contains("lotta_extensions::sidecar"));
    for forbidden in ["backend_root", "store_root", "workspace_root"] {
        assert!(!source.contains(forbidden));
    }
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(!host.contains("child_process"));
    assert!(!host.contains("models.json"));
}
