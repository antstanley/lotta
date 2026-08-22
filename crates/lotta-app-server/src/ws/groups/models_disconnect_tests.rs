//! Wire disconnect inherits the Task 52 active-turn guard and forced ordering.

use super::support::{CONNECTION_A, bridge_with_openai, encoded, find_row};
use serde_json::json;

#[tokio::test]
async fn refuses_while_active() {
    let models = bridge_with_openai("sk-openai-refuse").await;
    models.attach_active_openai_turn().await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "disconnect_provider",
                "request_id": "dr1",
                "target": "local",
                "provider_id": "openai",
            }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    let refused = encoded(&responses[0]);
    assert_eq!(refused["type"], "disconnect_provider_response");
    assert_eq!(refused["request_id"], "dr1");
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error"], "provider connection has active turns");
    assert_eq!(refused["providers"], json!([]));
    assert_eq!(refused["models_may_have_changed"], false);
    // The connection stays attached so later turns keep using it.
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "list_connect_providers",
                "request_id": "dr2",
                "target": "local",
            }),
        )
        .await;
    let listing = encoded(models.messages_for(CONNECTION_A).last().expect("listing"));
    let rows = listing["providers"].as_array().expect("provider rows");
    let openai = find_row(rows, "openai");
    assert_eq!(openai["connected"]["is_connected"], true);
}

#[tokio::test]
async fn force_cancels_then_disconnects() {
    let models = bridge_with_openai("sk-openai-force").await;
    models.attach_active_openai_turn().await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "disconnect_provider",
                "request_id": "df1",
                "target": "local",
                "provider_id": "openai",
                "force": true,
            }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    let removed = encoded(&responses[0]);
    assert_eq!(removed["type"], "disconnect_provider_response");
    assert_eq!(removed["request_id"], "df1");
    assert_eq!(removed["success"], true);
    assert_eq!(removed["models_may_have_changed"], true);
    let rows = removed["providers"].as_array().expect("provider rows");
    let openai = find_row(rows, "openai");
    assert_eq!(openai["connected"]["is_connected"], false);
    assert_eq!(openai["connected_providers"], json!([]));
    // Cancellation was observed and acknowledged before the removal.
    let order = models.cancellation.order.lock().expect("order lock");
    assert_eq!(*order, vec!["cancel", "ack"]);
}
