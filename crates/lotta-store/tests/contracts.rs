//! Task 09 shared store contracts executed against the real Task 22 adapter.

use lotta_store::{LocalStore, StorePaths};
use lotta_testkit::contract::fixtures;
use lotta_testkit::contract::{agent_store_contract, conversation_store_contract};
use lotta_testkit::roots::TemporaryRoot;

fn store(label: &str) -> (TemporaryRoot, LocalStore) {
    let root = TemporaryRoot::new(label).expect("temporary root");
    let paths = StorePaths::new(root.path().join("backend")).expect("store paths");
    (root, LocalStore::new(paths))
}

#[tokio::test]
async fn task09_agent_store_contract_real() {
    let (_root, adapter) = store("real-agent-contract");
    agent_store_contract(
        || {
            let value = adapter.clone();
            async move { value }
        },
        fixtures::agent("agent-a"),
        fixtures::agent("agent-b"),
    )
    .await;
}

#[tokio::test]
async fn task09_conversation_store_contract_real() {
    let (_root, adapter) = store("real-conversation-contract");
    conversation_store_contract(
        || {
            let value = adapter.clone();
            async move { value }
        },
        fixtures::conversation("agent-a", "default"),
        fixtures::conversation("agent-a", "conversation-b"),
    )
    .await;
}
