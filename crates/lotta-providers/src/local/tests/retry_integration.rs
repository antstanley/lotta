use super::lmstudio::LmStudio;
use crate::native::loopback::{Loopback, ResponseScript};
use lotta_runtime::ports::provider_event_channel;
use lotta_runtime::retry::ProviderRoute;
use lotta_runtime::retry::{Clock, EventSink, RetryEvent, RetryPolicy, Sleeper};
use lotta_runtime::{RetryExecutor, RuntimeError};
use lotta_testkit::contract::fixtures::provider_request;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Time {
    now: std::sync::atomic::AtomicU64,
    delays: Mutex<Vec<u64>>,
}
impl Clock for Time {
    fn monotonic_ms(&self) -> u64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn unix_epoch_ms(&self) -> u64 {
        0
    }
}
impl Sleeper for Time {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        _: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let ms = u64::try_from(duration.as_millis()).expect("delay");
            self.delays.lock().expect("lock").push(ms);
            self.now.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    }
}
#[derive(Default)]
struct Events;
impl EventSink for Events {
    fn emit(&self, _: RetryEvent) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn response(status: u16, body: &str) -> ResponseScript {
    ResponseScript::Complete(format!("HTTP/1.1 {status} TEST\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).into_bytes())
}

async fn run() -> Vec<u64> {
    let server = Loopback::sequence([
        response(503, "{}"),
        response(200, "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"),
    ]).await;
    let adapter = LmStudio::new(server.base(), "").expect("adapter");
    let mut request = provider_request(CancellationToken::new());
    request.deadline =
        lotta_runtime::ports::ProviderDeadline::new(Duration::from_mins(2)).expect("deadline");
    let (sink, mut receiver) = provider_event_channel(16, &request.cancellation).expect("channel");
    let time = Time::default();
    let executor = RetryExecutor::new(&time, &time, &Events, RetryPolicy::default(), None);
    let terminal = executor
        .execute(
            ProviderRoute::new("local", "lmstudio"),
            &adapter,
            None,
            request,
            sink,
        )
        .await
        .expect("execute");
    assert!(matches!(terminal, lotta_runtime::RetryTerminal::Success));
    while receiver.receive().await.expect("receive").is_some() {}
    let _ = server.recorded().await;
    time.delays.into_inner().expect("delays")
}

#[tokio::test]
async fn real_adapter_retries_once_with_deterministic_delay() {
    let first = run().await;
    let second = run().await;
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
}
