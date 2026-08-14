use crate::contract::common::assert_pending_once;
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{
    ChildProcessPort, PortFuture, ProcessEvent, ProcessOutcome, ProcessRequest, SandboxPort,
};
use std::future::Future;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Observable process behaviors that an adapter factory must arrange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessContractScenario {
    /// Two ordered output events followed by a successful zero exit.
    Success,
    /// Ordered output followed by a nonzero exit.
    NonZero,
    /// Execution completes with the normalized timeout outcome.
    TimedOut,
    /// Execution blocks on a capacity-one receiver until it is drained.
    Backpressure,
    /// Execution is cancelled while blocked on output delivery.
    Cancelled,
    /// The event receiver is closed before execution.
    ReceiverClosed,
    /// Execution terminates with an adapter runtime error.
    AdapterError,
}

/// Runs process success and cancellation checks with one isolated adapter per scenario.
pub async fn sandbox_contract<F, Fut, Adapter>(factory: F, request: ProcessRequest)
where
    F: Fn(ProcessContractScenario) -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: SandboxPort,
{
    process_contract(factory, request, |p, r, e, c| p.execute(r, e, c)).await;
}

/// Runs process success and cancellation checks with one isolated adapter per scenario.
pub async fn child_process_contract<F, Fut, Adapter>(factory: F, request: ProcessRequest)
where
    F: Fn(ProcessContractScenario) -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: ChildProcessPort,
{
    process_contract(factory, request, |p, r, e, c| p.run(r, e, c)).await;
}

async fn process_contract<F, Fut, P, Run>(factory: F, request: ProcessRequest, run: Run)
where
    F: Fn(ProcessContractScenario) -> Fut,
    Fut: Future<Output = P>,
    P: Sync,
    Run: Copy
        + for<'a> Fn(
            &'a P,
            ProcessRequest,
            mpsc::Sender<ProcessEvent>,
            CancellationToken,
        ) -> PortFuture<'a, ProcessOutcome>,
{
    let port = factory(ProcessContractScenario::Success).await;
    process_success(&port, request.clone(), run).await;
    let port = factory(ProcessContractScenario::NonZero).await;
    let (_, outcome) = collect_process(&port, request.clone(), run).await;
    assert_eq!(outcome.exit_code, Some(7));
    assert!(!outcome.timed_out);
    let port = factory(ProcessContractScenario::TimedOut).await;
    let (_, outcome) = collect_process(&port, request.clone(), run).await;
    assert_eq!(outcome.exit_code, None);
    assert!(outcome.timed_out);
    let port = factory(ProcessContractScenario::AdapterError).await;
    let (sender, _receiver) = mpsc::channel(1);
    assert!(matches!(
        run(&port, request.clone(), sender, CancellationToken::new()).await,
        Err(RuntimeError::AdapterFailure { .. })
    ));
    let port = factory(ProcessContractScenario::ReceiverClosed).await;
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    assert!(matches!(
        run(&port, request.clone(), sender, CancellationToken::new()).await,
        Err(RuntimeError::AdapterFailure { .. })
    ));
    let port = factory(ProcessContractScenario::Backpressure).await;
    let (sender, mut receiver) = mpsc::channel(1);
    let execution = run(&port, request.clone(), sender, CancellationToken::new());
    tokio::pin!(execution);
    assert_pending_once(execution.as_mut()).await;
    assert!(receiver.recv().await.is_some());
    assert!(execution.await.is_ok());
    assert!(receiver.recv().await.is_some());
    assert!(receiver.recv().await.is_none());
    let port = factory(ProcessContractScenario::Cancelled).await;
    let cancellation = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let execution = run(&port, request, sender, cancellation.clone());
    tokio::pin!(execution);
    assert_pending_once(execution.as_mut()).await;
    cancellation.cancel();
    assert!(matches!(
        execution.await,
        Err(RuntimeError::Cancelled { .. })
    ));
    assert!(receiver.recv().await.is_some());
    assert!(receiver.recv().await.is_none());
}

async fn process_success<'a, P, Run>(port: &'a P, request: ProcessRequest, run: Run)
where
    Run: Fn(
        &'a P,
        ProcessRequest,
        mpsc::Sender<ProcessEvent>,
        CancellationToken,
    ) -> PortFuture<'a, ProcessOutcome>,
{
    let (events, outcome) = collect_process(port, request, run).await;
    assert_eq!(
        outcome,
        ProcessOutcome {
            exit_code: Some(0),
            timed_out: false
        }
    );
    assert!(matches!(
        &events[..],
        [ProcessEvent::Stdout(out), ProcessEvent::Stderr(err)]
            if out.as_slice() == b"out" && err.as_slice() == b"err"
    ));
}

async fn collect_process<'a, P, Run>(
    port: &'a P,
    request: ProcessRequest,
    run: Run,
) -> (Vec<ProcessEvent>, ProcessOutcome)
where
    Run: Fn(
        &'a P,
        ProcessRequest,
        mpsc::Sender<ProcessEvent>,
        CancellationToken,
    ) -> PortFuture<'a, ProcessOutcome>,
{
    let (sender, mut receiver) = mpsc::channel(1);
    let execution = run(port, request, sender, CancellationToken::new());
    tokio::pin!(execution);
    let mut events = Vec::new();
    loop {
        tokio::select! {
            result = &mut execution => {
                let outcome = result.expect("process outcome");
                while let Some(value) = receiver.recv().await { events.push(value); }
                return (events, outcome);
            }
            value = receiver.recv() => if let Some(value) = value { events.push(value); }
        }
    }
}
