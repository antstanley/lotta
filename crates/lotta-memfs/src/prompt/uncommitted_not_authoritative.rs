use super::test_support::*;
use super::*;
use crate::tests::{message, path, valid};
use lotta_runtime::ports::MemFsPort;
use std::sync::atomic::AtomicUsize;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn committed_revision_controls_cache_authority() {
    let (root, port, agent) = setup().await;
    let cache = cache(&root, "uncommitted-identity");
    let renders = AtomicUsize::new(0);
    let before = cache
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("raw {CORE_MEMORY}", "2000-01-01T00:00:00Z"),
            DeliveryCapability::RequestBoundaryOnly,
            CancellationToken::new(),
        )
        .await
        .expect("before");
    port.write(
        &agent,
        &path("system/persona.md"),
        &valid("working persona"),
    )
    .await
    .expect("write");
    let dirty = cache
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("raw {CORE_MEMORY}", "2001-01-01T00:00:00Z"),
            DeliveryCapability::RequestBoundaryOnly,
            CancellationToken::new(),
        )
        .await
        .expect("dirty");
    assert!(!dirty.rendered);
    assert_eq!(dirty.delivery.content, before.delivery.content);
    port.commit(&agent, &message("persona"))
        .await
        .expect("commit");
    let committed = cache
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("raw {CORE_MEMORY}", "2002-01-01T00:00:00Z"),
            DeliveryCapability::RequestBoundaryOnly,
            CancellationToken::new(),
        )
        .await
        .expect("committed");
    assert!(committed.delivery.content.contains("working persona"));
}
