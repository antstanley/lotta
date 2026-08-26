use super::test_support::*;
use super::*;

#[test]
fn rejects_at_subscription_max() {
    let (router, _, _, id) = router();
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    for index in 0..lotta_domain::bounds::RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX.value {
        r.connections
            .subscribe(id, scope(index))
            .unwrap_or_else(|e| panic!("subscribe:{e}"));
    }
    r.connections
        .subscribe(id, scope(0))
        .unwrap_or_else(|e| panic!("duplicate:{e}"));
    assert!(r.connections.subscribe(id, scope(257)).is_err());
    assert_eq!(r.connections.subscription_count(id), Some(256));
}
#[test]
fn sandbox_subscription_count_tracks_non_resumable_close() {
    let (router, _, _, connection) = router();
    let mut router = lock_router(&router).unwrap_or_else(|error| panic!("lock:{error}"));
    let target = scope(1);
    router
        .connections
        .subscribe(connection, target.clone())
        .unwrap_or_else(|error| panic!("subscribe:{error}"));
    assert_eq!(router.connections.subscription_count_for(&target), 1);
    router.connections.suspend(connection);
    assert_eq!(router.connections.subscription_count_for(&target), 0);
}

#[test]
fn failed_subscription_update_is_retained_once_for_retry() {
    let (router, _, _, _) = router();
    let mut router = lock_router(&router).unwrap_or_else(|error| panic!("lock:{error}"));
    let target = scope(1);
    router.connections.retry_subscription_update(target.clone());
    router.connections.retry_subscription_update(target.clone());
    assert_eq!(router.connections.take_expired_subscriptions(), [target]);
}

#[test]
fn broadcast_preserves_ordinal_order() {
    let (router, _, _, first) = router();
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    let second = r.connections.open().unwrap_or_else(|e| panic!("open:{e}"));
    r.connections
        .initialize(second)
        .unwrap_or_else(|e| panic!("init:{e}"));
    for id in [second, first] {
        r.connections
            .subscribe(id, scope(1))
            .unwrap_or_else(|e| panic!("sub:{e}"));
    }
    let values = r
        .broadcast(&scope(1), &status_event("x"))
        .unwrap_or_else(|e| panic!("broadcast:{e}"));
    assert_eq!(
        values
            .as_slice()
            .iter()
            .map(|v| v.ordinal)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}
