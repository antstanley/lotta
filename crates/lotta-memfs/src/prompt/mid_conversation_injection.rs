use super::test_support::*;
use super::*;
use crate::tests::{message, path, valid};
use lotta_runtime::ports::MemFsPort;
use std::sync::atomic::AtomicUsize;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn capability_controls_injection_or_boundary_replacement() {
    let (root, port, agent) = setup().await;
    let supporting = cache(&root, "supporting-identity");
    let unsupported = cache(&root, "unsupported-identity");
    let renders = AtomicUsize::new(0);
    for store in [&supporting, &unsupported] {
        store
            .get_or_compile(
                &compiler(&port, &renders),
                &inputs("base {CORE_MEMORY}", "2000-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("base");
    }
    port.write(&agent, &path("system/persona.md"), &valid("fresh persona"))
        .await
        .expect("write");
    port.commit(&agent, &message("fresh"))
        .await
        .expect("commit");
    let supported = supporting
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("base {CORE_MEMORY}", "2001-01-01T00:00:00Z"),
            DeliveryCapability::MidConversationSystem,
            CancellationToken::new(),
        )
        .await
        .expect("supported");
    assert!(
        supported
            .delivery
            .mid_conversation_system_prompt
            .as_deref()
            .expect("injection")
            .contains("<memory_update>")
    );
    assert!(!supported.delivery.content.contains("fresh persona"));
    assert!(supported.persisted.mid_conversation_system_prompt.is_none());
    let boundary = unsupported
        .get_or_compile(
            &compiler(&port, &renders),
            &inputs("base {CORE_MEMORY}", "2001-01-01T00:00:00Z"),
            DeliveryCapability::RequestBoundaryOnly,
            CancellationToken::new(),
        )
        .await
        .expect("boundary");
    assert!(boundary.delivery.mid_conversation_system_prompt.is_none());
    assert!(boundary.delivery.content.contains("fresh persona"));
}
