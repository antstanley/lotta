use super::test_support::{get, launch, roots};

#[tokio::test(flavor = "multi_thread")]
async fn v1_absent_without_flag() {
    let roots = roots("route-off");
    let handle = launch(&roots, false, None).await;
    let response = get(&handle, "/v1/models", "").await;
    assert_eq!(response.status, 404);
    assert!(response.body.contains("not_found"));
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn v1_present_with_flag() {
    let roots = roots("route-on");
    let handle = launch(&roots, true, None).await;
    let response = get(&handle, "/v1/models", "").await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, r#"{"object":"list","data":[]}"#);
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn health_always_present() {
    for enabled in [false, true] {
        let roots = roots(if enabled { "health-on" } else { "health-off" });
        let handle = launch(&roots, enabled, None).await;
        assert_eq!(get(&handle, "/healthz", "").await.status, 200);
        assert_eq!(get(&handle, "/app-server-info", "").await.status, 200);
        handle
            .wait()
            .await
            .unwrap_or_else(|error| panic!("stop: {error}"));
    }
}
