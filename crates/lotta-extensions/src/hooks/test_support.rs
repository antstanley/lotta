pub(super) use super::{RegisteredHookRuntime, command::*, events::*, loader::*, prompt::*};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_runtime::{
    RuntimeError,
    boundary::{ProcessOutputChunk, ProviderText},
    ports::{
        ModelCapabilityPort, ModelCapabilityRequest, ModelCapabilityResponse, PortFuture,
        ProcessEvent, ProcessOutcome, ProcessRequest, SandboxPort,
    },
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

pub(super) fn owner(value: &str) -> HookOwner {
    HookOwner::new(value.into()).unwrap()
}
pub(super) fn id(value: &str) -> HookId {
    HookId::new(value.into()).unwrap()
}
pub(super) fn payload(event: HookEvent) -> HookPayload {
    HookPayload::new(event, json!({"event_type": event})).unwrap()
}
pub(super) fn command() -> HookConfig {
    HookConfig::Command(CommandHookConfig {
        kind: CommandHookType::Command,
        command: "/bin/true".into(),
        timeout: None,
        quiet: true,
    })
}
pub(super) fn prompt() -> HookConfig {
    HookConfig::Prompt(PromptHookConfig {
        kind: PromptHookType::Prompt,
        prompt: "evaluate $ARGUMENTS".into(),
        model: None,
        timeout: None,
        quiet: true,
    })
}

#[derive(Default)]
pub(super) struct Model {
    pub(super) calls: Mutex<usize>,
}
impl ModelCapabilityPort for Model {
    fn generate(&self, _: ModelCapabilityRequest) -> PortFuture<'_, ModelCapabilityResponse> {
        *self.calls.lock().unwrap() += 1;
        Box::pin(async {
            Ok(ModelCapabilityResponse {
                content: ProviderText::new("{\"ok\":true}".into()).unwrap(),
            })
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SandboxLifecycle {
    pub(super) program: String,
    pub(super) stdin: Vec<u8>,
}

#[derive(Clone)]
pub(super) struct ScriptedSandboxPort {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    outcome: ProcessOutcome,
    lifecycle: Arc<Mutex<Vec<SandboxLifecycle>>>,
}
impl Default for ScriptedSandboxPort {
    fn default() -> Self {
        Self {
            stdout: b"{\"ok\":true}".to_vec(),
            stderr: Vec::new(),
            outcome: ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            },
            lifecycle: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
impl ScriptedSandboxPort {
    pub(super) fn with_json(stdout: serde_json::Value) -> Self {
        Self::with_stdout(serde_json::to_vec(&stdout).unwrap())
    }

    pub(super) fn with_stdout(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            stdout: stdout.into(),
            ..Self::default()
        }
    }

    pub(super) fn blocking(reason: &str) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: reason.as_bytes().to_vec(),
            outcome: ProcessOutcome {
                exit_code: Some(2),
                timed_out: false,
            },
            lifecycle: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn lifecycle(&self) -> Vec<SandboxLifecycle> {
        self.lifecycle.lock().unwrap().clone()
    }
}
impl SandboxPort for ScriptedSandboxPort {
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        let stdout = self.stdout.clone();
        let stderr = self.stderr.clone();
        let outcome = self.outcome;
        let lifecycle = Arc::clone(&self.lifecycle);
        Box::pin(async move {
            lifecycle.lock().unwrap().push(SandboxLifecycle {
                program: request.program.as_str().to_owned(),
                stdin: request
                    .stdin
                    .as_ref()
                    .map_or_else(Vec::new, |stdin| stdin.as_slice().to_vec()),
            });
            if !stdout.is_empty() {
                events
                    .send(ProcessEvent::Stdout(ProcessOutputChunk::new(stdout)?))
                    .await
                    .map_err(|_| RuntimeError::AdapterFailure {
                        code: "channel_closed",
                        context: "scripted sandbox stdout".into(),
                    })?;
            }
            if !stderr.is_empty() {
                events
                    .send(ProcessEvent::Stderr(ProcessOutputChunk::new(stderr)?))
                    .await
                    .map_err(|_| RuntimeError::AdapterFailure {
                        code: "channel_closed",
                        context: "scripted sandbox stderr".into(),
                    })?;
            }
            Ok(outcome)
        })
    }
}

pub(super) fn runtime(
    registrations: Vec<HookRegistration>,
    model: Arc<Model>,
) -> RegisteredHookRuntime {
    runtime_with_sandbox(
        registrations,
        model,
        Arc::new(ScriptedSandboxPort::default()),
    )
}

pub(super) fn runtime_with_sandbox(
    registrations: Vec<HookRegistration>,
    model: Arc<Model>,
    sandbox: Arc<dyn SandboxPort>,
) -> RegisteredHookRuntime {
    let registry = Arc::new(HookRegistry::new());
    let registration_owner = registrations
        .first()
        .map_or_else(|| owner("o"), |registration| registration.owner.clone());
    registry
        .replace_owner(&registration_owner, registrations)
        .unwrap();
    RegisteredHookRuntime::new(
        registry,
        Arc::new(
            CommandHookExecutor::new(
                sandbox,
                RuntimeScope::new(
                    AgentId::accept("test-agent").unwrap(),
                    ConversationId::accept("test-conversation").unwrap(),
                    None,
                ),
                std::env::current_dir().unwrap(),
            )
            .unwrap(),
        ),
        Arc::new(PromptHookExecutor::new(model)),
    )
}
