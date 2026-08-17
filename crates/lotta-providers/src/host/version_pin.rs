use super::client::HostClient;
use super::pin::{PI_AI_PACKAGE_NAME, PinError, validate_package_root};
use super::tests::{config, owner};

#[tokio::test]
async fn accepts_pinned_version() {
    let (config, scratch) = config();
    let client = HostClient::spawn(config, owner()).await.unwrap();
    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn rejects_other_version() {
    let root = std::env::temp_dir().join(format!(
        "lotta-host-version-pin-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"{PI_AI_PACKAGE_NAME}","version":"0.82.2"}}"#),
    )
    .unwrap();
    let canonical = root.canonicalize().unwrap();
    assert!(matches!(
        validate_package_root(&canonical),
        Err(PinError::Version)
    ));
    std::fs::remove_dir_all(root).unwrap();
}
