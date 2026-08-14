//! Configurable process-port fakes.

use super::{cancelled, limit, lock};
use crate::TESTKIT_ITEMS_MAX;
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{
    ChildProcessPort, ProcessEvent, ProcessOutcome, ProcessRequest, SandboxPort,
};
use std::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
struct Script {
    events: Vec<ProcessEvent>,
    outcome: Result<ProcessOutcome, RuntimeError>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            outcome: Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            }),
        }
    }
}

/// In-memory sandbox fake with bounded configured output and cancellation.
#[derive(Debug, Default)]
pub struct FakeSandbox {
    script: Mutex<Script>,
}

/// In-memory child-process fake with bounded configured output and cancellation.
#[derive(Debug, Default)]
pub struct FakeChildProcess {
    script: Mutex<Script>,
}

macro_rules! configure {
    ($name:ident) => {
        impl $name {
            /// Replaces the next deterministic output script.
            ///
            /// # Errors
            /// Rejects scripts above the focused testkit event bound.
            pub fn configure(
                &self,
                events: Vec<ProcessEvent>,
                outcome: Result<ProcessOutcome, RuntimeError>,
            ) -> Result<(), RuntimeError> {
                if events.len() > TESTKIT_ITEMS_MAX {
                    return Err(limit("fake_process_events_max"));
                }
                *lock(&self.script) = Script { events, outcome };
                Ok(())
            }
        }
    };
}
configure!(FakeSandbox);
configure!(FakeChildProcess);

async fn execute(
    script: Script,
    events: Sender<ProcessEvent>,
    cancellation: CancellationToken,
) -> Result<ProcessOutcome, RuntimeError> {
    for event in script.events {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(cancelled("fake process")),
            result = events.send(event) => {
                result.map_err(|_| RuntimeError::AdapterFailure {
                    code: "testkit_process_channel_closed",
                    context: "fake process".into(),
                })?;
            }
        }
    }
    if cancellation.is_cancelled() {
        Err(cancelled("fake process"))
    } else {
        script.outcome
    }
}

impl SandboxPort for FakeSandbox {
    fn execute(
        &self,
        _request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ProcessOutcome> {
        let script = lock(&self.script).clone();
        Box::pin(execute(script, events, cancellation))
    }
}

impl ChildProcessPort for FakeChildProcess {
    fn run(
        &self,
        _request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ProcessOutcome> {
        let script = lock(&self.script).clone();
        Box::pin(execute(script, events, cancellation))
    }
}
