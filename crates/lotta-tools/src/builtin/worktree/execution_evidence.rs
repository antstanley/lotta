use super::{
    ConversationWorktreeContext, ProcessLiveness, WorktreeManager, WorktreeOwner,
    WorktreeToolBundle,
};
use crate::registry::ToolRegistry;
use lotta_domain::BoundedJsonValue;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::ProcessOutputChunk;
use lotta_runtime::ports::{
    ChildProcessPort, PortFuture, ProcessEvent, ProcessOutcome, ProcessRequest, ToolOutcome,
};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-task39-worktree-evidence-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join(".letta/worktrees")).expect("managed root");
        Self(path.canonicalize().expect("root"))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct Step {
    cwd: PathBuf,
    argv: Vec<String>,
    stdout: Vec<u8>,
    exit_code: i32,
    effect: Option<Effect>,
}
#[derive(Clone)]
enum Effect {
    Create(PathBuf),
}
#[derive(Default)]
struct ScriptedProcess {
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Vec<(PathBuf, Vec<String>)>>,
}
impl ScriptedProcess {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
    fn assert_drained(&self) {
        assert!(self.steps.lock().expect("steps").is_empty());
    }
}
impl ChildProcessPort for ScriptedProcess {
    fn run(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        let argv = request
            .arguments
            .as_slice()
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect::<Vec<_>>();
        let cwd = request.working_directory.value().to_owned();
        let step = self
            .steps
            .lock()
            .expect("steps")
            .pop_front()
            .expect("script step");
        assert_eq!(request.program.as_str(), "git");
        assert_eq!(cwd, step.cwd);
        assert_eq!(argv, step.argv);
        self.seen.lock().expect("seen").push((cwd, argv));
        Box::pin(async move {
            if let Some(effect) = step.effect {
                match effect {
                    Effect::Create(path) => fs::create_dir_all(path.join(".git")).expect("create"),
                }
            }
            if !step.stdout.is_empty() {
                events
                    .send(ProcessEvent::Stdout(
                        ProcessOutputChunk::new(step.stdout).expect("stdout"),
                    ))
                    .await
                    .map_err(|_| RuntimeError::AdapterFailure {
                        code: "test_process_channel",
                        context: "worktree evidence".into(),
                    })?;
            }
            Ok(ProcessOutcome {
                exit_code: Some(step.exit_code),
                timed_out: false,
            })
        })
    }
}

struct Live;
impl ProcessLiveness for Live {
    fn is_alive(&self, _: u32, _: &str) -> bool {
        true
    }
}
fn owner(id: &str, pid: u32) -> WorktreeOwner {
    WorktreeOwner::new(
        "agent".into(),
        id.into(),
        pid,
        "host".into(),
        format!("nonce-{pid}"),
    )
    .expect("owner")
}
fn manager(
    root: &Root,
    current: &Path,
    process: Arc<ScriptedProcess>,
    owner: WorktreeOwner,
) -> Arc<WorktreeManager> {
    let context = Arc::new(ConversationWorktreeContext::new(&root.0, current).expect("context"));
    Arc::new(
        WorktreeManager::new(&root.0, process, Arc::new(Live), owner, context).expect("manager"),
    )
}

fn list(root: &Path, worktree: &Path, branch: &str) -> Vec<u8> {
    format!(
        "worktree {}\0HEAD a\0branch refs/heads/main\0worktree {}\0HEAD b\0branch refs/heads/{branch}\0",
        root.display(),
        worktree.display()
    )
    .into_bytes()
}
fn step(cwd: &Path, argv: &[&str], stdout: Vec<u8>) -> Step {
    Step {
        cwd: cwd.to_owned(),
        argv: argv.iter().map(|value| (*value).to_owned()).collect(),
        stdout,
        exit_code: 0,
        effect: None,
    }
}

async fn invoke(bundle: &WorktreeToolBundle, name: &str, input: serde_json::Value) -> ToolOutcome {
    use crate::{AllowAllPermissions, AllowAllSandbox, NoopHooks, PipelineError, PipelineRequest};
    use crate::{OutcomeSink, SecretResolver, TraceEvent, TraceSink};
    struct NoTrace;
    impl TraceSink for NoTrace {
        fn record(&self, _: TraceEvent) {}
    }
    struct Sink;
    impl OutcomeSink for Sink {
        fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), PipelineError> {
            Ok(())
        }
    }
    struct Secrets;
    impl SecretResolver for Secrets {
        fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
            Ok(None)
        }
    }
    struct Overflow;
    impl crate::clamp::OverflowWriter for Overflow {
        fn write(&self, _: &str, _: &str) -> Result<String, crate::clamp::ClampError> {
            Err(crate::clamp::ClampError::OverflowWrite)
        }
    }
    let registration = bundle
        .registrations()
        .iter()
        .find(|item| item.definition.internal_name.as_str() == name)
        .expect("registration")
        .clone();
    let registry = ToolRegistry::new(bundle.registrations().to_vec()).expect("registry");
    let snapshot = registry
        .update(crate::toolset::ToolsetId::None, &[registration], None)
        .expect("snapshot");
    crate::execute(PipelineRequest {
        registry: snapshot,
        model_name: name,
        input: BoundedJsonValue::new(input).expect("input"),
        cancellation: CancellationToken::new(),
        hooks: &NoopHooks,
        permissions: &AllowAllPermissions,
        sandbox: &AllowAllSandbox,
        secrets: &Secrets,
        trace: &NoTrace,
        overflow: &Overflow,
        persistence: &Sink,
        emit: &Sink,
    })
    .await
    .expect("pipeline")
}

#[tokio::test]
async fn executor_enter_a_blocks_b_exit_a_then_enter_b() {
    let root = Root::new();
    let path = root.0.join(".letta/worktrees/existing");
    fs::create_dir_all(path.join(".git")).expect("worktree");
    let path = path.canonicalize().expect("canonical");
    let mut steps = Vec::new();
    for _ in 0..3 {
        steps.push(step(
            &root.0,
            &["rev-parse", "--show-toplevel"],
            format!("{}\n", root.0.display()).into_bytes(),
        ));
        steps.push(step(
            &root.0,
            &["worktree", "list", "--porcelain", "-z"],
            list(&root.0, &path, "topic"),
        ));
        steps.push(step(
            &root.0,
            &["worktree", "list", "--porcelain", "-z"],
            list(&root.0, &path, "topic"),
        ));
    }
    let process = Arc::new(ScriptedProcess::new(steps));
    let a = WorktreeToolBundle::new(manager(&root, &root.0, process.clone(), owner("a", 1)))
        .expect("a");
    let b = WorktreeToolBundle::new(manager(&root, &root.0, process.clone(), owner("b", 2)))
        .expect("b");
    assert!(matches!(
        invoke(&a, "EnterWorktree", serde_json::json!({"path":path})).await,
        ToolOutcome::Success { .. }
    ));
    assert!(matches!(
        invoke(&b, "EnterWorktree", serde_json::json!({"path":path})).await,
        ToolOutcome::ToolDefinedError { .. }
    ));
    assert!(matches!(
        invoke(&a, "ExitWorktree", serde_json::json!({"action":"keep"})).await,
        ToolOutcome::Success { .. }
    ));
    assert!(matches!(
        invoke(&b, "EnterWorktree", serde_json::json!({"path":path})).await,
        ToolOutcome::Success { .. }
    ));
    process.assert_drained();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn exit_keep_remove_and_discard_are_atomic() {
    let root = Root::new();
    let keep = root.0.join(".letta/worktrees/keep");
    fs::create_dir_all(keep.join(".git")).expect("keep");
    let keep = keep.canonicalize().expect("keep canonical");
    let keep_process = Arc::new(ScriptedProcess::new(Vec::new()));
    let keep_bundle = WorktreeToolBundle::new(manager(
        &root,
        &keep,
        keep_process.clone(),
        owner("keep", 20),
    ))
    .expect("keep bundle");
    keep_bundle
        .manager()
        .acquire(&keep, false)
        .expect("keep lock");
    assert!(matches!(
        invoke(
            &keep_bundle,
            "ExitWorktree",
            serde_json::json!({"action":"keep"})
        )
        .await,
        ToolOutcome::Success { .. }
    ));
    assert!(keep.exists());
    assert_eq!(
        keep_bundle.manager().context().current_cwd().expect("cwd"),
        root.0
    );
    keep_process.assert_drained();

    let remove = root.0.join(".letta/worktrees/remove");
    fs::create_dir_all(remove.join(".git")).expect("remove");
    let remove = remove.canonicalize().expect("remove canonical");
    let branch = "remove-branch";
    let remove_step = step(
        &root.0,
        &["worktree", "remove", remove.to_str().expect("path")],
        Vec::new(),
    );
    let remove_process = Arc::new(ScriptedProcess::new(vec![
        step(
            &root.0,
            &["worktree", "list", "--porcelain", "-z"],
            list(&root.0, &remove, branch),
        ),
        step(&remove, &["status", "--porcelain"], Vec::new()),
        step(
            &root.0,
            &["rev-list", "--count", &format!("HEAD..{branch}")],
            b"0\n".to_vec(),
        ),
        remove_step,
        step(&root.0, &["branch", "-d", branch], Vec::new()),
    ]));
    let remove_bundle = WorktreeToolBundle::new(manager(
        &root,
        &remove,
        remove_process.clone(),
        owner("remove", 21),
    ))
    .expect("remove bundle");
    remove_bundle
        .manager()
        .acquire(&remove, false)
        .expect("remove lock");
    assert!(matches!(
        invoke(
            &remove_bundle,
            "ExitWorktree",
            serde_json::json!({"action":"remove"}),
        )
        .await,
        ToolOutcome::Success { .. }
    ));
    assert!(remove.exists());
    assert_eq!(
        remove_bundle
            .manager()
            .context()
            .current_cwd()
            .expect("cwd"),
        root.0
    );
    remove_process.assert_drained();

    let discard = root.0.join(".letta/worktrees/discard");
    fs::create_dir_all(discard.join(".git")).expect("discard");
    let discard = discard.canonicalize().expect("discard canonical");
    let branch = "discard-branch";
    let discard_step = step(
        &root.0,
        &[
            "worktree",
            "remove",
            "--force",
            discard.to_str().expect("path"),
        ],
        Vec::new(),
    );
    let discard_process = Arc::new(ScriptedProcess::new(vec![
        step(
            &root.0,
            &["worktree", "list", "--porcelain", "-z"],
            list(&root.0, &discard, branch),
        ),
        discard_step,
        step(&root.0, &["branch", "-D", branch], Vec::new()),
    ]));
    let discard_bundle = WorktreeToolBundle::new(manager(
        &root,
        &discard,
        discard_process.clone(),
        owner("discard", 22),
    ))
    .expect("discard bundle");
    discard_bundle
        .manager()
        .acquire(&discard, false)
        .expect("discard lock");
    assert!(matches!(
        invoke(
            &discard_bundle,
            "ExitWorktree",
            serde_json::json!({"action":"remove","discard_changes":true}),
        )
        .await,
        ToolOutcome::Success { .. }
    ));
    assert!(discard.exists());
    assert_eq!(
        discard_bundle
            .manager()
            .context()
            .current_cwd()
            .expect("cwd"),
        root.0
    );
    discard_process.assert_drained();
}

#[tokio::test]
async fn exit_refuses_dirty_unmerged_and_remove_failure_without_state_change() {
    for (label, status, ahead, remove_failure) in [
        ("dirty", b" M file\n".to_vec(), None, false),
        ("unmerged", Vec::new(), Some(b"1\n".to_vec()), false),
        ("failure", Vec::new(), Some(b"0\n".to_vec()), true),
    ] {
        let root = Root::new();
        let path = root.0.join(format!(".letta/worktrees/{label}"));
        fs::create_dir_all(path.join(".git")).expect("worktree");
        let path = path.canonicalize().expect("canonical");
        let branch = format!("{label}-branch");
        let mut steps = vec![
            step(
                &root.0,
                &["worktree", "list", "--porcelain", "-z"],
                list(&root.0, &path, &branch),
            ),
            step(&path, &["status", "--porcelain"], status),
        ];
        if let Some(ahead) = ahead {
            steps.push(step(
                &root.0,
                &["rev-list", "--count", &format!("HEAD..{branch}")],
                ahead,
            ));
        }
        if remove_failure {
            let mut failure = step(
                &root.0,
                &["worktree", "remove", path.to_str().expect("path")],
                Vec::new(),
            );
            failure.exit_code = 1;
            steps.push(failure);
        }
        let process = Arc::new(ScriptedProcess::new(steps));
        let bundle =
            WorktreeToolBundle::new(manager(&root, &path, process.clone(), owner(label, 30)))
                .expect("bundle");
        bundle.manager().acquire(&path, false).expect("lock");
        assert!(matches!(
            invoke(
                &bundle,
                "ExitWorktree",
                serde_json::json!({"action":"remove"})
            )
            .await,
            ToolOutcome::ToolDefinedError { .. }
        ));
        assert!(path.exists());
        assert_eq!(bundle.manager().context().current_cwd().expect("cwd"), path);
        assert!(path.join(".git/letta-enter.lock").exists());
        process.assert_drained();
    }
}

#[tokio::test]
async fn enter_create_failure_leaves_no_worktree_lock_or_cwd_change() {
    let root = Root::new();
    let path = root.0.join(".letta/worktrees/fail-nonce40");
    let branch = "letta/fail-nonce40";
    let mut failure = step(
        &root.0,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            branch,
            path.to_str().expect("path"),
            "main",
        ],
        Vec::new(),
    );
    failure.exit_code = 1;
    let process = Arc::new(ScriptedProcess::new(vec![
        step(
            &root.0,
            &["rev-parse", "--show-toplevel"],
            format!("{}\n", root.0.display()).into_bytes(),
        ),
        failure,
    ]));
    let bundle = WorktreeToolBundle::new(manager(
        &root,
        &root.0,
        process.clone(),
        WorktreeOwner::new(
            "agent".into(),
            "create-failure".into(),
            40,
            "host".into(),
            "nonce40".into(),
        )
        .expect("owner"),
    ))
    .expect("bundle");
    assert!(matches!(
        invoke(
            &bundle,
            "EnterWorktree",
            serde_json::json!({"name":"fail","base_ref":"main","refresh_base":false}),
        )
        .await,
        ToolOutcome::ToolDefinedError { .. }
    ));
    assert!(!path.exists());
    assert_eq!(
        bundle.manager().context().current_cwd().expect("cwd"),
        root.0
    );
    process.assert_drained();
}

#[tokio::test]
async fn enter_creation_runs_provisioning_and_exact_git_argv() {
    let root = Root::new();
    fs::write(root.0.join(".worktreeinclude"), ".env\n").expect("include");
    fs::write(root.0.join(".env"), "TOKEN=test").expect("env");
    fs::create_dir_all(root.0.join(".husky/_")).expect("hooks");
    fs::write(root.0.join(".husky/_/pre-commit"), "hook").expect("hook");
    fs::create_dir_all(root.0.join(".letta")).expect("settings dir");
    fs::write(
        root.0.join(".letta/settings.json"),
        r#"{"worktree":{"copyLocalSettings":true,"linkHooks":true}}"#,
    )
    .expect("config");
    fs::write(root.0.join(".letta/settings.local.json"), "{}").expect("settings");
    fs::create_dir(root.0.join("node_modules")).expect("deps");
    let branch = "letta/demo-nonce9";
    let path = root.0.join(".letta/worktrees/demo-nonce9");
    let mut add = step(
        &root.0,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            branch,
            path.to_str().expect("path"),
            "main",
        ],
        Vec::new(),
    );
    add.effect = Some(Effect::Create(path.clone()));
    let process = Arc::new(ScriptedProcess::new(vec![
        step(
            &root.0,
            &["rev-parse", "--show-toplevel"],
            format!("{}\n", root.0.display()).into_bytes(),
        ),
        add,
        step(
            &root.0,
            &["config", "--get", "core.hooksPath"],
            b".husky/_\n".to_vec(),
        ),
    ]));
    let bundle = WorktreeToolBundle::new(manager(
        &root,
        &root.0,
        process.clone(),
        WorktreeOwner::new(
            "agent".into(),
            "create".into(),
            9,
            "host".into(),
            "nonce9".into(),
        )
        .expect("owner"),
    ))
    .expect("bundle");
    assert!(matches!(
        invoke(
            &bundle,
            "EnterWorktree",
            serde_json::json!({
                "name":"demo","base_ref":"main","refresh_base":false,
                "symlink_dependencies":true
            }),
        )
        .await,
        ToolOutcome::Success { .. }
    ));
    assert_eq!(
        fs::read_to_string(path.join(".env")).expect("env"),
        "TOKEN=test"
    );
    assert_eq!(
        fs::read_to_string(path.join(".letta/settings.local.json")).expect("settings"),
        "{}"
    );
    assert!(
        fs::symlink_metadata(path.join(".husky/_"))
            .expect("hooks")
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(path.join("node_modules"))
            .expect("deps")
            .file_type()
            .is_symlink()
    );
    process.assert_drained();
}
