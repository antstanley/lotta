use super::test_support::*;
use super::*;
use crate::RuntimeError;

#[tokio::test]
async fn auth_invalid_and_schema_stop_once() {
    for kind in [
        ProviderFailureKind::Auth,
        ProviderFailureKind::Invalid,
        ProviderFailureKind::Schema,
    ] {
        let time = FakeTime::default();
        let events = Events::default();
        let port = ScriptedPort::new(vec![vec![failure(kind)]]);
        assert_single_attempt(&time, &events, &port).await;
    }
}

#[tokio::test]
async fn unsupported_uses_typed_runtime_error_path() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::attempts(vec![ScriptedAttempt::Error(RuntimeError::Unsupported {
        context: "unsupported model".into(),
    })]);
    assert_single_attempt(&time, &events, &port).await;
}

async fn assert_single_attempt(time: &FakeTime, events: &Events, port: &ScriptedPort) {
    let (terminal, output) = run(RetryPolicy::default(), time, events, port)
        .await
        .unwrap();
    assert!(matches!(terminal, RetryTerminal::Failure(_)));
    assert!(output.is_empty());
    assert_eq!(port.calls(), 1);
    assert!(events.values.lock().unwrap().is_empty());
}
