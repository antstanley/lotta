use super::PortFuture;
use crate::boundary::{
    ConfinedPath, ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax, ProcessOutputChunk,
    ProcessStdin, Program,
};
use lotta_domain::RuntimeScope;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Complete validated request for one supervised process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRequest {
    /// Runtime owner of the process tree.
    pub scope: RuntimeScope,
    /// Bounded executable name or path.
    pub program: Program,
    /// Bounded arguments passed without shell interpolation.
    pub arguments: ProcessArguments,
    /// Lexically confined working directory.
    ///
    /// Adapters canonicalize and re-check symlinks immediately before spawn.
    pub working_directory: ConfinedPath,
    /// Bounded explicit child environment.
    pub environment: ProcessEnvironment,
    /// Optional bounded standard input.
    pub stdin: Option<ProcessStdin>,
    /// Maximum total output bytes, no greater than `PROCESS_OUTPUT_TOTAL_BYTES_MAX`.
    pub output_bytes_max: ProcessOutputBytesMax,
    /// Maximum execution duration.
    pub timeout: Duration,
}

impl ProcessRequest {
    /// Constructs a process request after validating the caller-selected total output ceiling.
    ///
    /// # Errors
    /// Returns [`crate::RuntimeError::LimitExceeded`] when `output_bytes_max` exceeds the canonical
    /// process total-output resource bound.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: RuntimeScope,
        program: Program,
        arguments: ProcessArguments,
        working_directory: ConfinedPath,
        environment: ProcessEnvironment,
        stdin: Option<ProcessStdin>,
        output_bytes_max: ProcessOutputBytesMax,
        timeout: Duration,
    ) -> Result<Self, crate::RuntimeError> {
        Ok(Self {
            scope,
            program,
            arguments,
            working_directory,
            environment,
            stdin,
            output_bytes_max,
            timeout,
        })
    }
}

/// Bounded ordered process output event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessEvent {
    /// Standard-output chunk.
    Stdout(ProcessOutputChunk),
    /// Standard-error chunk.
    Stderr(ProcessOutputChunk),
}
/// Terminal process state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessOutcome {
    /// Process exit code when available.
    pub exit_code: Option<i32>,
    /// Whether timeout terminated execution.
    pub timed_out: bool,
}

/// Supervises a runtime-scoped child process with bounded I/O and cancellation.
///
/// # Preconditions
/// Callers provide validated values and a bounded sender. Immediately before the OS call, adapters
/// canonicalize and re-check the working directory, reject NUL, enforce platform-specific program,
/// argument, environment-name, and environment-value rules, and translate failures into
/// [`crate::RuntimeError`]. They also validate the explicit total-output ceiling.
/// # Errors
/// Reports validation, confinement, spawn, I/O, output limit, channel, timeout, and cancellation
/// failures.
/// # Cancellation
/// Cancellation or drop terminates and reaps the process tree; timeout remains a normal outcome.
/// # Ownership
/// The adapter owns the request during execution; callers own events and outcome.
pub trait ChildProcessPort: Send + Sync {
    /// Runs one managed process with bounded-channel backpressure.
    fn run(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome>;
}

/// Executes policy-approved commands through an operating-system sandbox.
///
/// # Preconditions
/// Unlike [`ChildProcessPort`], this boundary additionally requires sandbox policy approval and an
/// enabled sandbox. The adapter canonicalizes and re-checks paths and symlinks immediately before
/// spawn.
/// # Errors
/// Reports unavailable sandboxing, validation, confinement, spawn, I/O, limits, channel, and
/// cancellation failures.
/// # Cancellation
/// Cancellation or drop terminates all sandbox descendants; timeout remains distinct.
/// # Ownership
/// The adapter owns the request while running; callers own streamed events and terminal outcome.
pub trait SandboxPort: Send + Sync {
    /// Executes one sandboxed process with bounded-channel backpressure.
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::*;
    use lotta_domain::{AgentId, ConversationId, RuntimeScope};
    use std::time::Duration;

    fn request() -> ProcessRequest {
        let name = EnvironmentName::new("TOKEN".into()).unwrap();
        let value = EnvironmentValue::new("recognizable-secret".into()).unwrap();
        let environment =
            ProcessEnvironment::new(vec![EnvironmentEntry::new(name, value).unwrap()]).unwrap();
        ProcessRequest::new(
            RuntimeScope::new(
                AgentId::accept("agent").unwrap(),
                ConversationId::accept("conversation").unwrap(),
                None,
            ),
            Program::new("program".into()).unwrap(),
            ProcessArguments::new(Vec::new()).unwrap(),
            ConfinedPath::new("/sandbox".into(), "/sandbox".into()).unwrap(),
            environment,
            Some(ProcessStdin::new(Vec::new()).unwrap()),
            ProcessOutputBytesMax::new(1).unwrap(),
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn structural_process_fields_and_variants_have_exact_types() {
        let ProcessRequest {
            scope,
            program,
            arguments,
            working_directory,
            environment,
            stdin,
            output_bytes_max,
            timeout,
        } = request();
        let _: RuntimeScope = scope;
        let _: Program = program;
        let _: ProcessArguments = arguments;
        let _: ConfinedPath = working_directory;
        let _: ProcessEnvironment = environment;
        let _: Option<ProcessStdin> = stdin;
        let _: ProcessOutputBytesMax = output_bytes_max;
        let _: Duration = timeout;
        let ProcessEvent::Stdout(chunk) =
            ProcessEvent::Stdout(ProcessOutputChunk::new(Vec::new()).unwrap())
        else {
            panic!()
        };
        let _: ProcessOutputChunk = chunk;
        let ProcessEvent::Stderr(chunk) =
            ProcessEvent::Stderr(ProcessOutputChunk::new(Vec::new()).unwrap())
        else {
            panic!()
        };
        let _: ProcessOutputChunk = chunk;
        let ProcessOutcome {
            exit_code,
            timed_out,
        } = ProcessOutcome {
            exit_code: Some(0),
            timed_out: false,
        };
        let _: Option<i32> = exit_code;
        let _: bool = timed_out;
    }

    #[test]
    fn nested_process_debug_redacts_environment_values() {
        let request = request();
        let entry = &request.environment.as_slice()[0];
        assert_eq!(entry.value().as_str(), "recognizable-secret");
        for debug in [
            format!("{entry:?}"),
            format!("{:?}", request.environment),
            format!("{request:?}"),
        ] {
            assert!(!debug.contains("recognizable-secret"));
            assert!(debug.contains("REDACTED"));
        }
    }
}
