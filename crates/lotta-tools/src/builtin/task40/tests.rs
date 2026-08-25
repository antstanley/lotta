use super::*;
use crate::{
    pipeline::{RawToolExecutionRequest, RawToolOutcome},
    toolset::ToolsetId,
};
use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, RuntimeScope};
use lotta_runtime::{
    RuntimeError,
    ports::{
        InteractiveSandboxPort, ModelFacingToolName, PortFuture, ProcessEvent, ProcessOutcome,
        ProcessRequest, ProcessSessionFuture, SandboxPort, ValidatedToolInput,
    },
};
use serde_json::json;
use std::{future, path::Path};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

struct SkillPort;
impl RegisteredSkillPort for SkillPort {
    fn load(&self, _: &str) -> skill::SkillLoadFuture<'_> {
        Box::pin(future::ready(Ok(skill::RegisteredSkillContent {
            instructions: "instructions".into(),
            companions: Vec::new(),
        })))
    }
}

struct LspPort;
impl lsp::LanguageServerPort for LspPort {
    fn diagnostics(
        &self,
        _: &'_ Path,
        _: CancellationToken,
        _: std::time::Duration,
    ) -> lsp::DiagnosticsFuture<'_> {
        Box::pin(future::ready(Ok(Vec::new())))
    }
}

struct Sandbox;
impl InteractiveSandboxPort for Sandbox {
    fn start_session(&self, _: ProcessRequest, _: CancellationToken) -> ProcessSessionFuture<'_> {
        Box::pin(async {
            Err(RuntimeError::Unsupported {
                context: "task40 scripted sandbox".into(),
            })
        })
    }
}
impl SandboxPort for Sandbox {
    fn execute(
        &self,
        _: ProcessRequest,
        _: Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        Box::pin(async {
            Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            })
        })
    }
}

#[tokio::test]
async fn registry_aliases_share_state_and_every_nonblocking_tool_executes() {
    let root = temp_root();
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let source = root.join("sample.rs");
    std::fs::write(&source, "fn main() {}").unwrap();
    let planning = Arc::new(PlanningPort::new());
    let tasks = Arc::new(TaskLifecyclePort::new());
    let skills: Arc<dyn RegisteredSkillPort> = Arc::new(SkillPort);
    let (interaction, _receiver) = InteractionPort::new();
    let server: Arc<dyn lsp::LanguageServerPort> = Arc::new(LspPort);
    let lsp = Arc::new(LanguageServerRegistry::new(root.clone(), [("rs".into(), server)]).unwrap());
    let shell = ShellToolBundle::new(&root, scope(), Arc::new(Sandbox)).unwrap();
    let bundle = Task40ToolBundle::new(
        Arc::clone(&planning),
        tasks,
        scope(),
        skills,
        interaction,
        lsp,
        &shell,
    )
    .unwrap();
    assert_eq!(named(bundle.registrations(), "TaskOutput"), 1);
    assert_eq!(named(bundle.registrations(), "TaskStop"), 1);

    let registry = ToolRegistry::new(bundle.registrations().to_vec()).unwrap();
    let pascal = registry
        .update(ToolsetId::Codex, &[], Some(&["update_plan"]))
        .unwrap()
        .by_model("UpdatePlan")
        .unwrap()
        .clone();
    let snake = registry
        .update(ToolsetId::CodexSnake, &[], Some(&["update_plan"]))
        .unwrap()
        .by_model("update_plan")
        .unwrap()
        .clone();
    assert!(Arc::ptr_eq(&pascal.executor, &snake.executor));
    execute(
        &pascal,
        json!({"plan":[{"step":"first","status":"pending"}]}),
    )
    .await;
    execute(
        &snake,
        json!({"plan":[{"step":"second","status":"in_progress"}]}),
    )
    .await;
    assert_eq!(planning.plan().unwrap()[0].step, "second");

    for (name, input) in [
        (
            "write_todos",
            json!({"todos":[{"content":"do","status":"pending","activeForm":"doing"}]}),
        ),
        (
            "TaskCreate",
            json!({"subject":"subject","description":"description"}),
        ),
        ("TaskList", json!({})),
        ("Skill", json!({"skill":"demo"})),
        ("ReadLSP", json!({"file_path":source})),
        (
            "TaskOutput",
            json!({"task_id":"missing","block":false,"timeout":0}),
        ),
        ("TaskStop", json!({"task_id":"missing"})),
    ] {
        let registration = bundle
            .registrations()
            .iter()
            .find(|item| item.definition.internal_name.as_str() == name)
            .unwrap();
        let tool = crate::registry::RegisteredTool {
            definition: Arc::clone(&registration.definition),
            model_name: ModelFacingToolName::new(name.into()).unwrap(),
            executor: Arc::clone(&registration.executor),
        };
        let _ = execute_any(&tool, input).await;
    }
    shell.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

async fn execute(tool: &crate::registry::RegisteredTool, input: serde_json::Value) {
    assert!(matches!(
        execute_any(tool, input).await,
        RawToolOutcome::Success(_)
    ));
}

async fn execute_any(
    tool: &crate::registry::RegisteredTool,
    input: serde_json::Value,
) -> RawToolOutcome {
    tool.executor
        .execute(RawToolExecutionRequest {
            tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
                lotta_runtime::boundary::ProviderName::new("raw-call".to_owned()).unwrap(),
            ),
            input: ValidatedToolInput::new(BoundedJsonValue::new(input).unwrap()).unwrap(),
            cancellation: CancellationToken::new(),
            deadline: tool.definition.timeout,
            definition: Arc::clone(&tool.definition),
            model_name: tool.model_name.clone(),
            secrets: crate::pipeline::test_empty_secret_delivery(),
        })
        .await
        .unwrap()
}

fn named(registrations: &[ToolRegistration], name: &str) -> usize {
    registrations
        .iter()
        .filter(|item| item.definition.internal_name.as_str() == name)
        .count()
}

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent").unwrap(),
        ConversationId::accept("conversation").unwrap(),
        None,
    )
}

fn temp_root() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "lotta-task40-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
