use super::test_support::*;
use super::*;

#[tokio::test]
async fn byte_identical_across_runs() {
    async fn once() -> (Vec<u64>, Vec<RetryEvent>) {
        let time = FakeTime::default();
        let events = Events::default();
        let port = ScriptedPort::new(vec![
            vec![failure(ProviderFailureKind::Transient)],
            vec![failure(ProviderFailureKind::Transient)],
            vec![failure(ProviderFailureKind::Transient)],
            vec![stop()],
        ]);
        run(RetryPolicy::default(), &time, &events, &port)
            .await
            .unwrap();
        let notices = events.values.lock().unwrap().clone();
        (time.delays(), notices)
    }
    assert_eq!(once().await, once().await);
}
