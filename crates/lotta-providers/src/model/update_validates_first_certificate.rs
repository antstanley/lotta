use super::test_support::*;

#[test]
fn unavailable_writes_zero_and_preserves_arc_state() {
    let store = Arc::new(Store::new());
    let prior = store
        .state
        .lock()
        .unwrap_or_else(|error| panic!("lock: {error}"))
        .clone();
    let result = ModelUpdateService::new(&Availability(Ok(false)), store.as_ref()).update(
        7,
        handle("next"),
        settings(&json!({})),
    );
    assert_eq!(result, Err(ModelUpdateError::Unavailable));
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        *store
            .state
            .lock()
            .unwrap_or_else(|error| panic!("lock: {error}")),
        prior
    );
}
#[test]
fn check_error_writes_zero() {
    let store = Store::new();
    assert_eq!(
        ModelUpdateService::new(&Availability(Err(AvailabilityError::CheckFailed)), &store).update(
            7,
            handle("next"),
            settings(&json!({}))
        ),
        Err(ModelUpdateError::AvailabilityCheck)
    );
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
}
#[test]
fn revision_race_is_atomic() {
    let store = Store::new();
    let prior = store
        .state
        .lock()
        .unwrap_or_else(|error| panic!("lock: {error}"))
        .clone();
    assert_eq!(
        ModelUpdateService::new(&Availability(Ok(true)), &store).update(
            6,
            handle("next"),
            settings(&json!({}))
        ),
        Err(ModelUpdateError::RevisionConflict)
    );
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        *store
            .state
            .lock()
            .unwrap_or_else(|error| panic!("lock: {error}")),
        prior
    );
}
#[test]
fn available_writes_once() {
    let store = Store::new();
    assert!(
        ModelUpdateService::new(&Availability(Ok(true)), &store)
            .update(7, handle("next"), settings(&json!({})))
            .is_ok()
    );
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
}
