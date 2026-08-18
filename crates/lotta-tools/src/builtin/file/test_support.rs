use super::FileToolBundle;
use crate::{
    AllowAllPermissions, AllowAllSandbox, OutcomeSink, PipelineError, PipelineRequest,
    SecretResolver, ToolRegistry, ToolsetId, TraceEvent, TraceSink, execute,
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::ports::ToolOutcome;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
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
    pub artifacts: PathBuf,
    pub overflow: PathBuf,
}

impl Fixture {
    pub fn new(label: &str) -> Self {
        let root = (0..32)
            .find_map(|_| {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let candidate = std::env::temp_dir()
                    .join(format!("lotta-task37-{}-{label}-{id}", std::process::id()));
                match fs::create_dir(&candidate) {
                    Ok(()) => Some(candidate),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("create fixture root: {error}"),
                }
            })
            .expect("bounded fixture root retries exhausted");
        for child in ["workspace", "peer", "artifacts", "overflow"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        Self {
            workspace: root.join("workspace"),
            peer: root.join("peer"),
            artifacts: root.join("artifacts"),
            overflow: root.join("overflow"),
            root,
        }
    }

    pub fn write(&self, path: &str, value: impl AsRef<[u8]>) {
        let full = self.workspace.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, value).unwrap();
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
    pub overflow: Mutex<Vec<(String, String)>>,
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
    fn record(&self, _: &str, value: &ToolOutcome) -> Result<(), PipelineError> {
        let target = if self.1 {
            &self.0.persisted
        } else {
            &self.0.emitted
        };
        target.lock().unwrap().push(value.clone());
        Ok(())
    }
}

struct EmptySecrets;
impl SecretResolver for EmptySecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}

struct Overflow(Arc<Records>, PathBuf);
impl crate::clamp::OverflowWriter for Overflow {
    fn write(&self, name: &str, value: &str) -> Result<String, crate::clamp::ClampError> {
        self.0
            .overflow
            .lock()
            .unwrap()
            .push((name.to_owned(), value.to_owned()));
        Ok(self.1.to_string_lossy().into_owned())
    }
}

pub(super) async fn run(
    fixture: &Fixture,
    toolset: ToolsetId,
    model: &str,
    input: Value,
) -> (Result<ToolOutcome, PipelineError>, Arc<Records>) {
    run_with_gate(fixture, toolset, model, input, &AllowAllSandbox).await
}

pub(super) async fn run_with_gate(
    fixture: &Fixture,
    toolset: ToolsetId,
    model: &str,
    input: Value,
    gate: &dyn crate::sandbox::SandboxGate,
) -> (Result<ToolOutcome, PipelineError>, Arc<Records>) {
    let bundle = FileToolBundle::new(&fixture.workspace, &fixture.artifacts).unwrap();
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
        approval_grant: lotta_runtime::ports::ToolApprovalGrant::None,
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: snapshot,
        model_name: model,
        input: BoundedJsonValue::new(input).unwrap(),
        cancellation: CancellationToken::new(),
        hook_runtime: &lotta_runtime::hooks::NoopHookRuntime,
        permissions: &AllowAllPermissions,
        sandbox: gate,
        secrets: &EmptySecrets,
        trace: &Trace(Arc::clone(&records)),
        overflow: &Overflow(Arc::clone(&records), fixture.overflow.join("full.txt")),
        persistence: &Sink(Arc::clone(&records), true),
        emit: &Sink(Arc::clone(&records), false),
    })
    .await;
    (result, records)
}

pub(super) fn success_text(outcome: &ToolOutcome) -> &str {
    match outcome {
        ToolOutcome::Success { content } => content.as_str(),
        other => panic!("unexpected outcome: {other:?}"),
    }
}

pub(super) fn assert_success(records: &Records, outcome: &ToolOutcome) {
    use crate::{PipelineStage as S, PreflightEvent as P, TraceEvent as T};
    let expected = vec![
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
    ];
    assert_eq!(*records.trace.lock().unwrap(), expected);
    assert_eq!(*records.persisted.lock().unwrap(), vec![outcome.clone()]);
    assert_eq!(*records.emitted.lock().unwrap(), vec![outcome.clone()]);
}

pub(super) fn has_residue(root: &Path) -> bool {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(".lotta"))
}
