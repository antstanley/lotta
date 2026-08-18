use super::*;
use lotta_domain::{BoundedJsonValue, BoundedVec};
use lotta_runtime::ports::{
    InternalToolName, ModelFacingToolName, PermissionAction, SecretRedactionPolicy,
    SecretRedactionSpec, ToolApprovalPolicy, ToolDescriptionAsset, ToolExecutionOwner,
    ToolInputSchema, ToolOutputLimit, ToolTimeout,
};
use lotta_tools::ToolRegistration;
use lotta_tools::pipeline::{
    ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use std::future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Statuses;

impl ProductionStatusSink for Statuses {
    fn emit(&self, _: SetupStatus) -> Result<(), SetupError> {
        Ok(())
    }
}

struct Fixture {
    root: PathBuf,
    store: LocalStore,
    agent: Agent,
    conversation: Conversation,
}

impl Fixture {
    async fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lotta-production-{label}-{}-{}",
            std::process::id(),
            ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("create isolated production root");
        for directory in [
            "global-skills",
            "bundled-skills",
            "workspace",
            "isolation",
            "isolation/workspace",
        ] {
            std::fs::create_dir(root.join(directory)).expect("create fixture directory");
        }
        let store = LocalStore::new(StorePaths::new(root.clone()).expect("store paths"));
        let agent = agent("agent-primary");
        let conversation = conversation("agent-primary", "conversation-primary", false);
        AgentStore::save(&store, &agent)
            .await
            .expect("persist agent");
        ConversationStore::save(&store, &conversation)
            .await
            .expect("persist conversation");
        Self {
            root,
            store,
            agent,
            conversation,
        }
    }

    fn ports(&self) -> ProductionSetupPorts {
        let registry = Arc::new(ToolRegistry::new(registrations()).expect("tool registry"));
        let mod_registries = Arc::new(ModRegistries::new(Arc::clone(&registry)));
        ProductionSetupPorts::new(ProductionSetupConfig {
            store_paths: StorePaths::new(self.root.clone()).expect("store paths"),
            skill_roots: SkillRoots {
                project_working_root: self.root.join("workspace"),
                agent_skills_directory: None,
                memory_root: None,
                global_skills_directory: self.root.join("global-skills"),
                bundled_skills_directory: self.root.join("bundled-skills"),
            },
            models: vec![model()],
            default_model: ModelHandle::from_str("openai/gpt-5.4").expect("model handle"),
            server_context_window: 16_384,
            output_tokens: 2_048,
            registry,
            mod_registries,
            hook_registry: Arc::new(HookRegistry::new()),
            hook_runtime: Arc::new(lotta_runtime::hooks::NoopHookRuntime),
            permission_sources: lotta_tools::permissions::scopes::PermissionSourcePaths::new(
                &self.root,
                &self.root,
                &self.root.join("isolation/workspace"),
            ),
            workspace_policy: WorkspacePolicy::new(&lotta_runtime::WorkspaceSandbox::new(
                self.root.join("isolation/workspace"),
                self.root.join("isolation"),
            ))
            .expect("workspace policy"),
            toolset: ToolsetId::Codex,
            allowlist: None,
            status_sink: Arc::new(Statuses),
        })
        .expect("production ports")
    }

    fn input(&self, cwd: PathBuf, fallback: PathBuf, text: &str) -> SetupInput {
        let cwd = if cwd == self.root.join("workspace") {
            self.root.join("isolation/workspace")
        } else {
            cwd
        };
        let fallback = if fallback == self.root.join("workspace") {
            self.root.join("isolation/workspace")
        } else {
            fallback
        };
        SetupInput {
            agent_id: self.agent.id.clone(),
            conversation_id: self.conversation.id.clone(),
            cwd,
            fallback_cwd: fallback,
            user_input: text.to_owned(),
            selected_skills: Vec::new(),
            permission_mode: PermissionMode::Unrestricted,
            cancellation: CancellationToken::new(),
            deadline: Duration::from_secs(10),
        }
    }

    fn conversation_tree(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let directory = self
            .store
            .paths()
            .conversation_dir(&self.agent.id, &self.conversation.id)
            .expect("conversation directory");
        let mut tree = BTreeMap::new();
        snapshot(&directory, &directory, &mut tree);
        tree
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Stub;

impl ToolExecutor for Stub {
    fn execute(&self, _: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        Box::pin(future::pending::<
            Result<RawToolOutcome, lotta_tools::pipeline::ExecutorError>,
        >())
    }
}

fn registration(name: &str) -> ToolRegistration {
    let definition = lotta_runtime::ports::ToolDefinition::new(
        InternalToolName::new(name.to_owned()).expect("internal name"),
        ModelFacingToolName::new(name.to_owned()).expect("model name"),
        ToolInputSchema::new(
            BoundedJsonValue::new(serde_json::json!({"type": "object"})).expect("schema value"),
        )
        .expect("schema"),
        ToolDescriptionAsset::new(format!("{name} description")).expect("description"),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".to_owned()).expect("permission action"),
        ToolTimeout::new(Duration::from_secs(1)).expect("timeout"),
        ToolOutputLimit::new(1_024, 1_024).expect("output limit"),
        SecretRedactionSpec::new(
            BoundedVec::new(Vec::new()).expect("secret fields"),
            SecretRedactionPolicy::Redact,
        )
        .expect("redaction"),
    );
    ToolRegistration {
        definition: Arc::new(definition),
        executor: Arc::new(Stub),
    }
}

fn registrations() -> Vec<ToolRegistration> {
    lotta_tools::names::rows()
        .iter()
        .filter(|row| row.toolset == ToolsetId::Codex)
        .map(|row| row.internal)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(registration)
        .collect()
}

fn agent(id: &str) -> Agent {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": id,
        "description": null,
        "system": "You are a production fixture.",
        "tags": [],
        "model": "openai/gpt-5.4",
        "model_settings": {},
        "hidden": false,
        "compaction_settings": null
    }))
    .expect("valid agent")
}

fn conversation(agent_id: &str, id: &str, archived: bool) -> Conversation {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "agent_id": agent_id,
        "archived": archived,
        "created_at": "2026-08-14T00:00:00Z",
        "updated_at": "2026-08-14T00:00:00Z",
        "last_message_at": null,
        "summary": null,
        "in_context_message_ids": [],
        "model": null,
        "model_settings": null,
        "context_window_limit": null,
        "hidden": false,
        "tags": []
    }))
    .expect("valid conversation")
}

fn model() -> ModelDescriptor {
    ModelDescriptor {
        handle: NonEmptyString::new("openai/gpt-5.4").expect("model"),
        provider_id: NonEmptyString::new("openai").expect("provider"),
        available: true,
        context_window: Some(32_768),
        model_settings: None,
    }
}

fn snapshot(root: &Path, directory: &Path, tree: &mut BTreeMap<PathBuf, Vec<u8>>) {
    if !directory.exists() {
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .expect("read snapshot directory")
        .map(|entry| entry.expect("snapshot entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            snapshot(root, &path, tree);
        } else {
            tree.insert(
                path.strip_prefix(root)
                    .expect("relative path")
                    .to_path_buf(),
                std::fs::read(path).expect("snapshot bytes"),
            );
        }
    }
}

fn transcript_bytes(fixture: &Fixture) -> Vec<u8> {
    let path = fixture
        .store
        .paths()
        .conversation_dir(&fixture.agent.id, &fixture.conversation.id)
        .expect("conversation directory")
        .join("messages.jsonl");
    std::fs::read(path).expect("transcript bytes")
}

#[tokio::test]
async fn production_resolve_conversation_many_agents_no_deadlock() {
    let fixture = Fixture::new("resolve-many").await;
    for index in 0..96 {
        let value = agent(&format!("agent-{index:03}"));
        AgentStore::save(&fixture.store, &value)
            .await
            .expect("persist many agents");
    }
    let owner = agent("agent-owner");
    let global = conversation("agent-owner", "conversation-global", false);
    AgentStore::save(&fixture.store, &owner)
        .await
        .expect("persist owner");
    ConversationStore::save(&fixture.store, &global)
        .await
        .expect("persist global conversation");
    let ports = fixture.ports();
    let missing_requested = AgentId::accept("agent-missing-requested").expect("agent id");
    let resolved = tokio::time::timeout(
        Duration::from_secs(5),
        SetupPorts::resolve_conversation(
            &ports,
            &missing_requested,
            &global.id,
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("resolution did not deadlock")
    .expect("resolve globally");
    assert_eq!(resolved.agent_id, owner.id);
}

#[tokio::test]
async fn production_fresh_conversation_initializes_and_admits() {
    let fixture = Fixture::new("fresh-admit").await;
    let fresh = conversation("agent-primary", "conversation-fresh", false);
    ConversationStore::save(&fixture.store, &fresh)
        .await
        .expect("persist fresh conversation");
    let ports = fixture.ports();
    let receipt = SetupPorts::admit_input(
        &ports,
        &fixture.agent.id,
        &fresh.id,
        "fresh production input",
        None,
    )
    .await
    .expect("admit fresh input");
    let bytes = std::fs::read(
        fixture
            .store
            .paths()
            .conversation_dir(&fixture.agent.id, &fresh.id)
            .expect("conversation directory")
            .join("messages.jsonl"),
    )
    .expect("fresh transcript");
    let text = String::from_utf8(bytes).expect("utf8 transcript");
    assert!(text.contains(&receipt.input_id));
    assert!(text.contains("fresh production input"));
    assert!(text.contains("\"type\":\"session\""));
}

#[tokio::test]
async fn production_toolset_preserves_model_names_and_allowlist() {
    let fixture = Fixture::new("toolset").await;
    let ports = fixture.ports();
    let resolved = SetupPorts::resolve_model(&ports, &fixture.agent, &fixture.conversation)
        .expect("resolve model");
    assert_eq!(resolved.toolset, "codex");
    let mut allowlisted = resolved.clone();
    allowlisted.allowlist = Some(vec![
        "Agent".to_owned(),
        "ViewImage".to_owned(),
        "ApplyPatch".to_owned(),
        "UpdatePlan".to_owned(),
    ]);
    let catalog = SetupPorts::merge_tools(&ports, &allowlisted, Vec::new()).expect("merge tools");
    let names: BTreeSet<_> = catalog
        .definitions()
        .map(|definition| definition.model_name.as_str().to_owned())
        .collect();
    assert_eq!(
        names,
        ["Agent", "ApplyPatch", "UpdatePlan", "ViewImage"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

#[tokio::test]
async fn production_deleted_cwd_reminder_is_human_and_once_across_instances() {
    let fixture = Fixture::new("deleted-reminder").await;
    let fallback = fixture.root.join("isolation/workspace");
    let deleted = fallback.join("deleted-before-turn");
    let ports = fixture.ports();
    let output = SetupOrchestrator::new(&ports)
        .prepare(fixture.input(deleted.clone(), fallback.clone(), "remember cwd"))
        .await
        .expect("prepare deleted cwd");
    let prompt = output.request.system_prompt.expect("compiled prompt");
    let prompt = prompt.as_str();
    assert!(prompt.contains(&deleted.display().to_string()));
    assert!(prompt.contains(&fallback.display().to_string()));
    assert!(prompt.contains("was deleted"));
    for forbidden in ["pid", "nonce", "token"] {
        assert!(!prompt.to_ascii_lowercase().contains(forbidden));
    }
    let second = fixture.ports();
    let output = SetupOrchestrator::new(&second)
        .prepare(fixture.input(deleted.clone(), fallback, "second input"))
        .await
        .expect("prepare second instance");
    let prompt = output.request.system_prompt.expect("second prompt");
    assert!(!prompt.as_str().contains(&deleted.display().to_string()));
}

#[tokio::test]
async fn production_stale_claim_before_append_is_reclaimed() {
    let fixture = Fixture::new("stale-claim").await;
    let fallback = fixture.root.join("isolation/workspace");
    let deleted = fallback.join("stale-deleted");
    let early = fixture.ports().with_clock(|| 1_000);
    SetupPorts::apply_scope(
        &early,
        &fallback,
        PermissionMode::Unrestricted,
        &CancellationToken::new(),
    )
    .await
    .expect("apply scope");
    let first = SetupPorts::claim_cwd_reminder(
        &early,
        &fixture.agent.id,
        &fixture.conversation.id,
        &deleted,
        &CancellationToken::new(),
    )
    .await
    .expect("claim reminder")
    .expect("first claim");
    let late = fixture
        .ports()
        .with_clock(|| (REMINDER_PENDING_TTL_SECONDS + 2) * 1_000);
    SetupPorts::apply_scope(
        &late,
        &fallback,
        PermissionMode::Unrestricted,
        &CancellationToken::new(),
    )
    .await
    .expect("apply second scope");
    let reclaimed = SetupPorts::claim_cwd_reminder(
        &late,
        &fixture.agent.id,
        &fixture.conversation.id,
        &deleted,
        &CancellationToken::new(),
    )
    .await
    .expect("reclaim reminder")
    .expect("stale claim reclaimed");
    assert_ne!(first.token(), reclaimed.token());
    assert_ne!(first.input_id(), reclaimed.input_id());
}

#[tokio::test]
async fn production_crash_after_append_recovers_consumed() {
    let fixture = Fixture::new("crash-recovery").await;
    let fallback = fixture.root.join("isolation/workspace");
    let deleted = fallback.join("crash-deleted");
    let ports = fixture.ports();
    SetupPorts::apply_scope(
        &ports,
        &fallback,
        PermissionMode::Unrestricted,
        &CancellationToken::new(),
    )
    .await
    .expect("apply scope");
    let claim = SetupPorts::claim_cwd_reminder(
        &ports,
        &fixture.agent.id,
        &fixture.conversation.id,
        &deleted,
        &CancellationToken::new(),
    )
    .await
    .expect("claim reminder")
    .expect("claim exists");
    let now = Timestamp::from_utc(chrono::Utc::now());
    let entry = TranscriptEntry::Message(MessageEntry {
        entry_type: MessageEntryType::Message,
        id: NonEmptyString::new(claim.input_id().to_owned()).expect("entry id"),
        parent_id: None,
        timestamp: now,
        message: LocalMessage {
            id: MessageId::accept(claim.input_id()).expect("message id"),
            role: LocalMessageRole::User,
            content: Some(
                BoundedJsonValue::new(serde_json::json!("crash input")).expect("content"),
            ),
            timestamp: chrono::Utc::now().timestamp_millis() as f64,
            metadata: None,
            extras: Default::default(),
        },
    });
    append_or_initialize(
        &fixture.store,
        &fixture.agent.id,
        &fixture.conversation.id,
        &entry,
        now,
    )
    .await
    .expect("append before simulated crash");
    let before = transcript_bytes(&fixture);
    let recovered = fixture.ports().with_clock(|| 7_000);
    SetupPorts::apply_scope(
        &recovered,
        &fallback,
        PermissionMode::Unrestricted,
        &CancellationToken::new(),
    )
    .await
    .expect("apply recovered scope");
    let next = SetupPorts::claim_cwd_reminder(
        &recovered,
        &fixture.agent.id,
        &fixture.conversation.id,
        &deleted,
        &CancellationToken::new(),
    )
    .await
    .expect("recover consumed claim");
    assert!(next.is_none());
    assert_eq!(before, transcript_bytes(&fixture));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_concurrent_claim_exactly_one() {
    for repetition in 0..50 {
        let fixture = Fixture::new(&format!("concurrent-claim-{repetition}")).await;
        let fallback = fixture.root.join("isolation/workspace");
        let deleted = fallback.join("concurrent-deleted");
        let first = fixture.ports().with_clock(|| 1_000);
        let second = fixture.ports().with_clock(|| 1_000);
        for ports in [&first, &second] {
            SetupPorts::apply_scope(
                ports,
                &fallback,
                PermissionMode::Unrestricted,
                &CancellationToken::new(),
            )
            .await
            .expect("apply scope");
        }
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let agent = fixture.agent.id.clone();
        let conversation = fixture.conversation.id.clone();
        let deleted_first = deleted.clone();
        let first_barrier = Arc::clone(&barrier);
        let first_task = tokio::spawn(async move {
            first_barrier.wait().await;
            SetupPorts::claim_cwd_reminder(
                &first,
                &agent,
                &conversation,
                &deleted_first,
                &CancellationToken::new(),
            )
            .await
        });
        let agent = fixture.agent.id.clone();
        let conversation = fixture.conversation.id.clone();
        let second_barrier = Arc::clone(&barrier);
        let second_task = tokio::spawn(async move {
            second_barrier.wait().await;
            SetupPorts::claim_cwd_reminder(
                &second,
                &agent,
                &conversation,
                &deleted,
                &CancellationToken::new(),
            )
            .await
        });
        barrier.wait().await;
        let claims = [
            first_task.await.expect("first task").expect("first claim"),
            second_task
                .await
                .expect("second task")
                .expect("second claim"),
        ];
        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    }
}

#[tokio::test]
async fn production_cross_agent_archived_leave_store_unchanged() {
    let fixture = Fixture::new("invalid-conversation").await;
    let other = agent("agent-other");
    let cross = conversation("agent-other", "conversation-cross", false);
    let archived = conversation("agent-primary", "conversation-archived", true);
    AgentStore::save(&fixture.store, &other)
        .await
        .expect("persist other agent");
    ConversationStore::save(&fixture.store, &cross)
        .await
        .expect("persist cross conversation");
    ConversationStore::save(&fixture.store, &archived)
        .await
        .expect("persist archived conversation");
    let ports = fixture.ports();
    let before = fixture.conversation_tree();
    let mut cross_input = fixture.input(
        fixture.root.join("workspace"),
        fixture.root.join("workspace"),
        "cross agent input",
    );
    cross_input.agent_id = other.id.clone();
    cross_input.conversation_id = fixture.conversation.id.clone();
    assert!(matches!(
        SetupOrchestrator::new(&ports).prepare(cross_input).await,
        Err(lotta_runtime::turn::SetupFailure::PreAdmission(
            SetupError::CrossAgent
        ))
    ));
    let mut archived_input = fixture.input(
        fixture.root.join("workspace"),
        fixture.root.join("workspace"),
        "archived input",
    );
    archived_input.conversation_id = archived.id;
    assert!(matches!(
        SetupOrchestrator::new(&ports).prepare(archived_input).await,
        Err(lotta_runtime::turn::SetupFailure::PreAdmission(
            SetupError::ArchivedOrInvalid
        ))
    ));
    assert_eq!(before, fixture.conversation_tree());
}

#[tokio::test]
async fn production_scope_rejects_escape_before_memfs() {
    let fixture = Fixture::new("scope-escape").await;
    let outside = fixture.root.join("outside");
    std::fs::create_dir(&outside).expect("outside directory");
    let ports = fixture.ports();
    let before = fixture.root.join("memfs").exists();
    let result = SetupOrchestrator::new(&ports)
        .prepare(fixture.input(
            outside.clone(),
            fixture.root.join("isolation/workspace"),
            "escape",
        ))
        .await;
    assert!(matches!(
        result,
        Err(lotta_runtime::turn::SetupFailure::PreAdmission(
            SetupError::Cwd(CwdFailure::SymlinkOrEscape)
        ))
    ));
    assert_eq!(before, fixture.root.join("memfs").exists());
}

#[tokio::test]
async fn production_strict_vs_unrestricted_actual_catalog() {
    let fixture = Fixture::new("permission-catalog").await;
    let unrestricted = fixture.ports();
    SetupPorts::apply_scope(
        &unrestricted,
        &fixture.root.join("isolation/workspace"),
        PermissionMode::Unrestricted,
        &CancellationToken::new(),
    )
    .await
    .expect("unrestricted scope");
    let unrestricted_candidates = SetupPorts::tool_candidates(
        &unrestricted,
        &ExtensionSnapshot::default(),
        &CancellationToken::new(),
    )
    .expect("unrestricted candidates");
    let strict = fixture.ports();
    SetupPorts::apply_scope(
        &strict,
        &fixture.root.join("isolation/workspace"),
        PermissionMode::Strict,
        &CancellationToken::new(),
    )
    .await
    .expect("strict scope");
    let strict_candidates = SetupPorts::tool_candidates(
        &strict,
        &ExtensionSnapshot::default(),
        &CancellationToken::new(),
    )
    .expect("strict candidates");
    assert!(
        unrestricted_candidates
            .iter()
            .filter(|candidate| candidate.authorized)
            .count()
            >= strict_candidates
                .iter()
                .filter(|candidate| candidate.authorized)
                .count()
    );
}

#[tokio::test]
async fn production_strict_path_command_is_not_catalog_authorized() {
    let fixture = Fixture::new("permission-path-command").await;
    let ports = fixture.ports();
    SetupPorts::apply_scope(
        &ports,
        &fixture.root.join("isolation/workspace"),
        PermissionMode::Strict,
        &CancellationToken::new(),
    )
    .await
    .expect("strict scope");
    let candidate = ToolCandidate {
        source: SetupToolSource::ControllerExternal,
        definition: (*registration("Bash").definition).clone(),
        model_name: "Bash".to_owned(),
        authorized: true,
    };
    let scope = ports.scope.lock().expect("scope lock");
    let actual = authorize_candidate(scope.as_ref().expect("scope"), candidate)
        .expect("permission decision");
    assert!(!actual.authorized);
}
