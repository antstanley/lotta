use super::{ShellSandbox, ShellToolBundle};
use crate::clamp::OverflowWriter;
use crate::{
    AllowAllPermissions, AllowAllSandbox, NoopHooks, OutcomeSink, PipelineError, PipelineRequest,
    SecretResolver, ToolRegistry, ToolsetId, TraceEvent, TraceSink, execute,
};
use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, RuntimeScope};
use lotta_runtime::ports::ToolOutcome;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Fixture {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub peer: PathBuf,
    pub overflow: PathBuf,
}

impl Fixture {
    pub fn new(label: &str) -> Self {
        let root = (0..32)
            .find_map(|_| {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let candidate = std::env::temp_dir()
                    .join(format!("lotta-task38-{}-{label}-{id}", std::process::id()));
                match fs::create_dir(&candidate) {
                    Ok(()) => Some(candidate),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("create fixture root: {error}"),
                }
            })
            .expect("bounded fixture root retries exhausted");
        for child in ["workspace", "peer", "overflow"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        Self {
            workspace: root.join("workspace"),
            peer: root.join("peer"),
            overflow: root.join("overflow"),
            root,
        }
    }

    pub fn write_peer(&self, name: &str, value: &str) {
        fs::write(self.peer.join(name), value).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
pub(super) struct Records {
    pub trace: Mutex<Vec<TraceEvent>>,
    pub persisted: Mutex<Vec<ToolOutcome>>,
    pub emitted: Mutex<Vec<ToolOutcome>>,
}

struct Trace(Arc<Records>);
impl TraceSink for Trace {
    fn record(&self, event: TraceEvent) {
        self.0.trace.lock().unwrap().push(event);
    }
}

struct Sink(Arc<Records>, bool);
impl OutcomeSink for Sink {
    fn record(&self, _: &str, outcome: &ToolOutcome) -> Result<(), PipelineError> {
        let target = if self.1 {
            &self.0.persisted
        } else {
            &self.0.emitted
        };
        target.lock().unwrap().push(outcome.clone());
        Ok(())
    }
}

struct EmptySecrets;
impl SecretResolver for EmptySecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}

struct Overflow(PathBuf);
impl crate::clamp::OverflowWriter for Overflow {
    fn write(&self, _: &str, value: &str) -> Result<String, crate::clamp::ClampError> {
        fs::write(&self.0, value).map_err(|_| crate::clamp::ClampError::OverflowWrite)?;
        Ok(self.0.to_string_lossy().into_owned())
    }
}

pub(super) fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent").unwrap(),
        ConversationId::accept("conversation").unwrap(),
        None,
    )
}

pub(super) async fn run(
    fixture: &Fixture,
    sandbox: Arc<dyn ShellSandbox>,
    toolset: ToolsetId,
    model: &str,
    input: Value,
) -> (
    Result<ToolOutcome, PipelineError>,
    Arc<Records>,
    ShellToolBundle,
) {
    run_with(
        fixture,
        sandbox,
        toolset,
        model,
        input,
        CancellationToken::new(),
    )
    .await
}

pub(super) async fn run_with(
    fixture: &Fixture,
    sandbox: Arc<dyn ShellSandbox>,
    toolset: ToolsetId,
    model: &str,
    input: Value,
    cancellation: CancellationToken,
) -> (
    Result<ToolOutcome, PipelineError>,
    Arc<Records>,
    ShellToolBundle,
) {
    let bundle = ShellToolBundle::new(&fixture.workspace, scope(), sandbox).unwrap();
    let overflow = Overflow(fixture.overflow.join("full.txt"));
    let (result, records) =
        execute_bundle_with_overflow(&bundle, toolset, model, input, cancellation, &overflow).await;
    (result, records, bundle)
}

pub(super) async fn execute_bundle(
    fixture: &Fixture,
    bundle: &ShellToolBundle,
    toolset: ToolsetId,
    model: &str,
    input: Value,
    cancellation: CancellationToken,
) -> (Result<ToolOutcome, PipelineError>, Arc<Records>) {
    let overflow = Overflow(fixture.overflow.join("full.txt"));
    execute_bundle_with_overflow(bundle, toolset, model, input, cancellation, &overflow).await
}

pub(super) async fn execute_bundle_with_overflow(
    bundle: &ShellToolBundle,
    toolset: ToolsetId,
    model: &str,
    input: Value,
    cancellation: CancellationToken,
    overflow: &dyn OverflowWriter,
) -> (Result<ToolOutcome, PipelineError>, Arc<Records>) {
    let registrations = bundle.registrations().to_vec();
    let registry = ToolRegistry::new(registrations.clone()).unwrap();
    let snapshot = if toolset == ToolsetId::None {
        let registration = registrations
            .iter()
            .find(|item| item.definition.internal_name.as_str() == model)
            .unwrap()
            .clone();
        registry.update(toolset, &[registration], None).unwrap()
    } else {
        registry.update(toolset, &[], Some(&[model])).unwrap()
    };
    let records = Arc::new(Records::default());
    let result = execute(PipelineRequest {
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: snapshot,
        model_name: model,
        input: BoundedJsonValue::new(input).unwrap(),
        cancellation,
        hooks: &NoopHooks,
        permissions: &AllowAllPermissions,
        sandbox: &AllowAllSandbox,
        secrets: &EmptySecrets,
        trace: &Trace(Arc::clone(&records)),
        overflow,
        persistence: &Sink(Arc::clone(&records), true),
        emit: &Sink(Arc::clone(&records), false),
    })
    .await;
    (result, records)
}

pub(super) fn assert_complete(records: &Records, outcome: &ToolOutcome) {
    use crate::{PipelineStage as S, PreflightEvent as P, TraceEvent as T};
    assert_eq!(
        *records.trace.lock().unwrap(),
        vec![
            T::Preflight(P::NameResolution),
            T::Preflight(P::SchemaValidation),
            T::Stage(S::PreHook),
            T::Stage(S::Permission),
            T::Stage(S::Sandbox),
            T::Stage(S::SecretSubstitution),
            T::Stage(S::Executor),
            T::Stage(S::PostHook),
            T::Stage(S::Scrub),
            T::Stage(S::Clamp),
            T::Stage(S::Persist),
            T::Stage(S::Emit),
        ]
    );
    assert_eq!(*records.persisted.lock().unwrap(), vec![outcome.clone()]);
    assert_eq!(*records.emitted.lock().unwrap(), vec![outcome.clone()]);
}

pub(super) fn success_text(outcome: &ToolOutcome) -> &str {
    match outcome {
        ToolOutcome::Success { content } => content.as_str(),
        other => panic!("unexpected outcome: {other:?}"),
    }
}
