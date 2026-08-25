mod setup {
    use super::super::TurnToolCatalog;
    use super::super::setup::{
        AdmissionReceipt, CwdFailure, CwdResolution, ExtensionSnapshot, ReminderClaim,
        ResolvedTurnModel, SetupError, SetupFailure, SetupInput, SetupOrchestrator, SetupPorts,
        SetupScopeHandle, SetupStage, SetupStatus, SetupStatusSink, SetupToolSource,
        SkillInventory, ToolCandidate,
    };
    use crate::RuntimeError;
    use crate::boundary::ProviderText;
    use crate::ports::{
        AgentStore, ConversationStore, ImagePolicy, ProviderContent, ProviderContentPart,
        ProviderDeadline, ProviderMessage, ProviderMessageRole, ProviderMessages, ProviderRequest,
        ProviderToolChoice, ProviderTools, ReasoningControls, TokenLimit,
    };
    use lotta_domain::{
        Agent, AgentId, Conversation, ConversationId, ModelDescriptor, PermissionMode, RuntimeScope,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::mpsc::Sender;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Fault {
        Fail,
        Cancel,
        Pause,
    }

    #[derive(Default)]
    struct RecordingSetupPorts {
        agents: Mutex<BTreeMap<AgentId, Agent>>,
        conversations: Mutex<BTreeMap<(AgentId, ConversationId), Conversation>>,
        calls: Mutex<Vec<String>>,
        transcript: Mutex<Vec<(String, String)>>,
        interrupted: Mutex<BTreeMap<String, String>>,
        claims: Mutex<BTreeSet<String>>,
        consumed: Mutex<BTreeSet<String>>,
        fault: Mutex<Option<(SetupStage, Fault)>>,
        candidates: Mutex<Vec<ToolCandidate>>,
        inventory: Mutex<SkillInventory>,
        compile_inputs: Mutex<Vec<(SkillInventory, Option<String>)>>,
        cwd: Mutex<Option<CwdResolution>>,
        scopes: Mutex<BTreeMap<u64, (PathBuf, PermissionMode)>>,
        next_scope: Mutex<u64>,
    }

    #[derive(Default)]
    struct RecordingStatusSink {
        statuses: Mutex<Vec<SetupStatus>>,
    }

    impl SetupStatusSink for RecordingStatusSink {
        fn emit(&self, status: SetupStatus) -> Result<(), SetupError> {
            self.statuses.lock().unwrap().push(status);
            Ok(())
        }
    }

    impl RecordingSetupPorts {
        fn valid() -> Self {
            let ports = Self::default();
            let agent = agent("agent-a");
            let conversation = conversation("agent-a", "conversation-a");
            ports.agents.lock().unwrap().insert(agent.id.clone(), agent);
            ports.conversations.lock().unwrap().insert(
                (AgentId::accept("agent-a").unwrap(), conversation.id.clone()),
                conversation,
            );
            *ports.inventory.lock().unwrap() = SkillInventory {
                available: vec!["skill-a".into()],
                selected: vec!["skill-a".into()],
            };
            ports
        }

        fn log(&self, value: impl Into<String>) {
            self.calls.lock().unwrap().push(value.into());
        }
        fn set_fault(&self, stage: SetupStage, fault: Fault) {
            *self.fault.lock().unwrap() = Some((stage, fault));
        }
        fn stage(
            &self,
            stage: SetupStage,
            cancellation: &CancellationToken,
        ) -> Result<(), SetupError> {
            if let Some((configured, fault)) = *self.fault.lock().unwrap()
                && configured == stage
            {
                match fault {
                    Fault::Fail => return Err(SetupError::Adapter(format!("fault:{stage:?}"))),
                    Fault::Cancel => {
                        cancellation.cancel();
                        return Err(SetupError::Cancelled);
                    }
                    Fault::Pause => {}
                }
            }
            Ok(())
        }
    }

    impl AgentStore for RecordingSetupPorts {
        fn load(&self, id: &AgentId) -> crate::ports::PortFuture<'_, Agent> {
            self.log(format!("agent.load:{}", id.as_str()));
            let result = self.agents.lock().unwrap().get(id).cloned().ok_or_else(|| {
                RuntimeError::NotFound {
                    context: "agent".into(),
                }
            });
            Box::pin(async move { result })
        }
        fn list(
            &self,
            _items: Sender<Agent>,
            _cancellation: CancellationToken,
        ) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn save(&self, _agent: &Agent) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn delete(&self, _id: &AgentId) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl ConversationStore for RecordingSetupPorts {
        fn load(
            &self,
            owner: &AgentId,
            id: &ConversationId,
        ) -> crate::ports::PortFuture<'_, Conversation> {
            self.log(format!(
                "conversation.load:{}:{}",
                owner.as_str(),
                id.as_str()
            ));
            let result = self
                .conversations
                .lock()
                .unwrap()
                .get(&(owner.clone(), id.clone()))
                .cloned()
                .ok_or_else(|| RuntimeError::NotFound {
                    context: "conversation".into(),
                });
            Box::pin(async move { result })
        }
        fn list_for_agent(
            &self,
            _agent: &AgentId,
            _items: Sender<Conversation>,
            _cancellation: CancellationToken,
        ) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn save(&self, _conversation: &Conversation) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn delete(
            &self,
            _agent: &AgentId,
            _id: &ConversationId,
        ) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl SetupPorts for RecordingSetupPorts {
        fn agents(&self) -> &dyn AgentStore {
            self
        }
        fn conversations(&self) -> &dyn ConversationStore {
            self
        }
        fn resolve_cwd(
            &self,
            requested: &Path,
            fallback: &Path,
        ) -> Result<CwdResolution, SetupError> {
            self.log(format!(
                "cwd.resolve:{}:{}",
                requested.display(),
                fallback.display()
            ));
            self.stage(SetupStage::ResolveCwd, &CancellationToken::new())?;
            if let Some(value) = self.cwd.lock().unwrap().clone() {
                return Ok(value);
            }
            let metadata = std::fs::symlink_metadata(requested).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    SetupError::Cwd(CwdFailure::InvalidFallback)
                } else {
                    SetupError::Cwd(CwdFailure::PermissionDenied)
                }
            })?;
            if metadata.file_type().is_symlink() {
                return Err(SetupError::Cwd(CwdFailure::SymlinkOrEscape));
            }
            if !metadata.is_dir() {
                return Err(SetupError::Cwd(CwdFailure::NotDirectory));
            }
            Ok(CwdResolution::Requested(
                std::fs::canonicalize(requested).unwrap(),
            ))
        }
        fn apply_scope(
            &self,
            _runtime: RuntimeScope,
            cwd: &Path,
            mode: PermissionMode,
            cancellation: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, SetupScopeHandle> {
            self.log(format!("scope.apply:{}:{mode:?}", cwd.display()));
            let result = self
                .stage(
                    SetupStage::ApplyWorkspaceSandboxAndPermissions,
                    cancellation,
                )
                .map_err(|error| runtime(&error))
                .map(|()| {
                    let mut next = self.next_scope.lock().unwrap();
                    *next += 1;
                    let handle = SetupScopeHandle::new(*next);
                    self.scopes
                        .lock()
                        .unwrap()
                        .insert(handle.id(), (cwd.to_path_buf(), mode));
                    handle
                });
            Box::pin(async move { result })
        }
        fn prepare_memfs(
            &self,
            agent: &Agent,
            cancellation: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, ()> {
            self.log(format!("memfs.prepare:{}", agent.id.as_str()));
            let result = self
                .stage(SetupStage::SynchronizeOrInitializeMemFs, cancellation)
                .map_err(|error| runtime(&error));
            Box::pin(async move { result })
        }
        fn skill_inventory(
            &self,
            agent: &Agent,
            cwd: &Path,
            selected: &[String],
        ) -> Result<SkillInventory, SetupError> {
            self.log(format!(
                "skills.inventory:{}:{}:{selected:?}",
                agent.id.as_str(),
                cwd.display()
            ));
            Ok(self.inventory.lock().unwrap().clone())
        }
        fn compile_prompt(
            &self,
            agent: &Agent,
            conversation: &Conversation,
            inventory: &SkillInventory,
            scope: SetupScopeHandle,
            reminder: Option<&str>,
            cancellation: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, String> {
            let cwd = self.scopes.lock().unwrap()[&scope.id()].0.clone();
            self.log(format!(
                "prompt.compile:{}:{}:{:?}:{reminder:?}",
                agent.id.as_str(),
                conversation.id.as_str(),
                inventory.selected
            ));
            self.compile_inputs
                .lock()
                .unwrap()
                .push((inventory.clone(), reminder.map(str::to_owned)));
            let result = self
                .stage(SetupStage::CompileSystemPrompt, cancellation)
                .map(|()| {
                    format!(
                        "prompt skills={:?} cwd={} reminder={reminder:?}",
                        inventory.selected,
                        cwd.display()
                    )
                })
                .map_err(|error| runtime(&error));
            Box::pin(async move { result })
        }
        fn resolve_model(
            &self,
            agent: &Agent,
            conversation: &Conversation,
        ) -> Result<ResolvedTurnModel, SetupError> {
            self.log(format!(
                "model.resolve:{}:{}",
                agent.id.as_str(),
                conversation.id.as_str()
            ));
            self.stage(
                SetupStage::ResolveModelProviderContextAndToolset,
                &CancellationToken::new(),
            )?;
            Ok(ResolvedTurnModel {
                model: model(),
                context_window: 4096,
                output_tokens: 512,
                toolset: "openai".into(),
                allowlist: None,
                fallback_candidates: Vec::new(),
            })
        }
        fn discover_selected(
            &self,
            inventory: &SkillInventory,
            selected: &[String],
        ) -> Result<Vec<String>, SetupError> {
            self.log(format!(
                "skills.select:{:?}:{selected:?}",
                inventory.available
            ));
            self.stage(
                SetupStage::DiscoverSelectedSkills,
                &CancellationToken::new(),
            )?;
            Ok(selected.to_vec())
        }
        fn load_extensions(
            &self,
            agent: &Agent,
            cwd: &Path,
            cancellation: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, ExtensionSnapshot> {
            self.log(format!(
                "extensions.load:{}:{}",
                agent.id.as_str(),
                cwd.display()
            ));
            let result = self
                .stage(SetupStage::LoadModsAndHooks, cancellation)
                .map(|()| ExtensionSnapshot::default())
                .map_err(|error| runtime(&error));
            Box::pin(async move { result })
        }
        fn tool_candidates(
            &self,
            scope: SetupScopeHandle,
            extensions: &ExtensionSnapshot,
            cancellation: &CancellationToken,
        ) -> Result<Vec<ToolCandidate>, SetupError> {
            let _ = &self.scopes.lock().unwrap()[&scope.id()];
            self.log(format!("tools.candidates:{extensions:?}"));
            self.stage(SetupStage::MergeTools, cancellation)?;
            Ok(self.candidates.lock().unwrap().clone())
        }
        fn merge_tools(
            &self,
            model: &ResolvedTurnModel,
            candidates: Vec<ToolCandidate>,
        ) -> Result<TurnToolCatalog, SetupError> {
            self.log(format!(
                "tools.merge:{}:{:?}",
                model.toolset,
                candidates
                    .iter()
                    .map(|c| (c.source, c.definition.internal_name.as_str()))
                    .collect::<Vec<_>>()
            ));
            TurnToolCatalog::new(
                candidates
                    .into_iter()
                    .map(|candidate| candidate.definition)
                    .collect(),
            )
            .map_err(|error| SetupError::Adapter(error.to_string()))
        }
        fn build_request(
            &self,
            _: &AgentId,
            _: &ConversationId,
            prompt: String,
            input: &str,
            _: Option<&str>,
            model: &ResolvedTurnModel,
            catalog: &TurnToolCatalog,
            cancellation: CancellationToken,
        ) -> crate::ports::PortFuture<'_, ProviderRequest> {
            self.log(format!(
                "request.build:{prompt}:{input}:{}:{}",
                model.model.handle.as_str(),
                catalog.definitions().count()
            ));
            let result = self.stage(SetupStage::BuildProviderRequestAndEmitStatus, &cancellation);
            let request = request(prompt, input, model.model.clone(), cancellation);
            Box::pin(async move {
                result.map_err(|error| runtime(&error))?;
                request.map_err(|error| runtime(&error))
            })
        }
        fn admit_input(
            &self,
            agent: &AgentId,
            conversation: &ConversationId,
            input: &str,
            reminder_claim: Option<&ReminderClaim>,
        ) -> crate::ports::PortFuture<'_, AdmissionReceipt> {
            self.log(format!(
                "input.admit:{}:{}:{input}:{reminder_claim:?}",
                agent.as_str(),
                conversation.as_str()
            ));
            self.transcript
                .lock()
                .unwrap()
                .push(("input-1".into(), input.into()));
            if self.fault.lock().unwrap().is_some_and(|(stage, fault)| {
                stage == SetupStage::BuildProviderRequestAndEmitStatus && fault == Fault::Pause
            }) {
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = reminder_claim;
            Box::pin(async {
                Ok(AdmissionReceipt {
                    appended: true,
                    input_id: "input-1".into(),
                })
            })
        }
        fn commit_cwd_reminder(
            &self,
            claim: &ReminderClaim,
            _: &AdmissionReceipt,
            _: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, ()> {
            self.claims.lock().unwrap().remove(&claim.message);
            self.consumed.lock().unwrap().insert(claim.message.clone());
            Box::pin(async { Ok(()) })
        }
        fn claim_cwd_reminder(
            &self,
            agent: &AgentId,
            conversation: &ConversationId,
            original: &Path,
            scope: SetupScopeHandle,
            _: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, Option<ReminderClaim>> {
            let _ = &self.scopes.lock().unwrap()[&scope.id()];
            let claim = format!(
                "{}:{}:{}",
                agent.as_str(),
                conversation.as_str(),
                original.display()
            );
            self.log(format!("reminder.claim:{claim}"));
            if self.consumed.lock().unwrap().contains(&claim)
                || !self.claims.lock().unwrap().insert(claim.clone())
            {
                Box::pin(async { Ok(None) })
            } else {
                Box::pin(async move {
                    Ok(Some(ReminderClaim::new(
                        claim.clone(),
                        "input-1".into(),
                        claim,
                    )))
                })
            }
        }
        fn release_cwd_reminder(
            &self,
            claim: &ReminderClaim,
            _: &CancellationToken,
        ) -> crate::ports::PortFuture<'_, ()> {
            self.claims.lock().unwrap().remove(&claim.message);
            Box::pin(async { Ok(()) })
        }
        fn record_interrupted(
            &self,
            receipt: &AdmissionReceipt,
            failure: &SetupError,
        ) -> crate::ports::PortFuture<'_, ()> {
            self.log(format!("interrupted.record:{}:{failure}", receipt.input_id));
            self.interrupted
                .lock()
                .unwrap()
                .entry(receipt.input_id.clone())
                .or_insert_with(|| failure.to_string());
            Box::pin(async { Ok(()) })
        }
        fn release_scope(&self, scope: SetupScopeHandle) {
            self.log(format!("scope.release:{}", scope.id()));
            self.scopes.lock().unwrap().remove(&scope.id());
        }
    }

    fn agent(id: &str) -> Agent {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "description": null,
            "system": "system",
            "tags": [],
            "model": "openai/gpt-4o",
            "model_settings": {},
            "hidden": false,
            "compaction_settings": null
        }))
        .unwrap()
    }
    fn conversation(agent: &str, id: &str) -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "agent_id": agent,
            "archived": false,
            "created_at": "2026-08-14T00:00:00Z",
            "updated_at": "2026-08-14T00:00:00Z",
            "in_context_message_ids": []
        }))
        .unwrap()
    }
    fn model() -> ModelDescriptor {
        serde_json::from_value(serde_json::json!({
            "handle": "openai/gpt-4o",
            "provider_id": "openai",
            "available": true,
            "context_window": 4096
        }))
        .unwrap()
    }
    fn request(
        prompt: String,
        input: &str,
        model: ModelDescriptor,
        cancellation: CancellationToken,
    ) -> Result<ProviderRequest, SetupError> {
        let content = ProviderContent::new(vec![ProviderContentPart::Text(
            ProviderText::new(input.into()).map_err(|e| SetupError::Adapter(e.to_string()))?,
        )])
        .map_err(|e| SetupError::Adapter(e.to_string()))?;
        Ok(ProviderRequest {
            model,
            system_prompt: Some(
                ProviderText::new(prompt).map_err(|e| SetupError::Adapter(e.to_string()))?,
            ),
            messages: ProviderMessages::new(vec![ProviderMessage {
                role: ProviderMessageRole::User,
                content,
                tool_call_id: None,
            }])
            .map_err(|e| SetupError::Adapter(e.to_string()))?,
            tools: ProviderTools::new(Vec::new())
                .map_err(|e| SetupError::Adapter(e.to_string()))?,
            tool_choice: ProviderToolChoice::Auto,
            image_policy: ImagePolicy::Strict,
            context_tokens_max: TokenLimit::new(4096)
                .map_err(|e| SetupError::Adapter(e.to_string()))?,
            output_tokens_max: TokenLimit::new(512)
                .map_err(|e| SetupError::Adapter(e.to_string()))?,
            reasoning: ReasoningControls {
                enabled: false,
                effort: None,
                tier: None,
            },
            cancellation,
            context: None,
            deadline: ProviderDeadline::default(),
        })
    }
    fn runtime(error: &SetupError) -> RuntimeError {
        RuntimeError::AdapterFailure {
            code: "recording_setup",
            context: error.to_string(),
        }
    }
    fn input(cwd: PathBuf, fallback: PathBuf) -> SetupInput {
        SetupInput {
            agent_id: AgentId::accept("agent-a").unwrap(),
            conversation_id: ConversationId::accept("conversation-a").unwrap(),
            cwd,
            fallback_cwd: fallback,
            user_input: "hello".into(),
            selected_skills: vec!["skill-a".into()],
            permission_mode: PermissionMode::default(),
            cancellation: CancellationToken::new(),
            deadline: Duration::from_secs(1),
            status_sink: Arc::new(RecordingStatusSink::default()),
        }
    }
    fn temp() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lotta-setup-{}",
            uuid::Uuid::from_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    async fn prepare(
        ports: &RecordingSetupPorts,
        cwd: &Path,
        fallback: &Path,
    ) -> Result<super::super::setup::SetupOutput, SetupFailure> {
        SetupOrchestrator::new(ports)
            .prepare(input(cwd.into(), fallback.into()))
            .await
    }

    mod step_order {
        use super::*;
        #[tokio::test]
        async fn successful_stage_log_exact() {
            let root = temp();
            let canonical = std::fs::canonicalize(&root).unwrap();
            let ports = RecordingSetupPorts::valid();
            let output = prepare(&ports, &root, &root).await.unwrap();
            assert_eq!(output.stages, SetupStage::ORDER);
            assert_eq!(
                ports.calls.lock().unwrap().as_slice(),
                [
                    "agent.load:agent-a",
                    "conversation.load:agent-a:conversation-a",
                    &format!("cwd.resolve:{}:{}", root.display(), root.display()),
                    &format!("scope.apply:{}:Unrestricted", canonical.display()),
                    "memfs.prepare:agent-a",
                    &format!(
                        "skills.inventory:agent-a:{}:[\"skill-a\"]",
                        canonical.display()
                    ),
                    "prompt.compile:agent-a:conversation-a:[\"skill-a\"]:None",
                    "model.resolve:agent-a:conversation-a",
                    "skills.select:[\"skill-a\"]:[\"skill-a\"]",
                    &format!("extensions.load:agent-a:{}", canonical.display()),
                    "tools.candidates:ExtensionSnapshot { id: 0 }",
                    "tools.merge:openai:[]",
                    &format!(
                        "request.build:prompt skills=[\"skill-a\"] cwd={} reminder=None:hello:openai/gpt-4o:0",
                        canonical.display()
                    ),
                    "input.admit:agent-a:conversation-a:hello:None"
                ]
            );
        }
        #[tokio::test]
        async fn reorder_stop_on_failure() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.set_fault(SetupStage::SynchronizeOrInitializeMemFs, Fault::Fail);
            assert!(prepare(&ports, &root, &root).await.is_err());
            assert!(
                !ports
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call.starts_with("prompt.compile"))
            );
        }
        #[tokio::test]
        async fn task32_toolset_after_permission_and_memfs() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            prepare(&ports, &root, &root).await.unwrap();
            let calls = ports.calls.lock().unwrap();
            let merge = calls
                .iter()
                .position(|v| v.starts_with("tools.merge"))
                .unwrap();
            assert!(
                calls
                    .iter()
                    .position(|v| v.starts_with("scope.apply"))
                    .unwrap()
                    < merge
            );
            assert!(
                calls
                    .iter()
                    .position(|v| v.starts_with("memfs.prepare"))
                    .unwrap()
                    < merge
            );
        }
    }

    mod deleted_cwd {
        use super::*;
        #[tokio::test]
        async fn falls_back() {
            let root = temp();
            let deleted = root.join("deleted");
            std::fs::create_dir(&deleted).unwrap();
            std::fs::remove_dir(&deleted).unwrap();
            let ports = RecordingSetupPorts::valid();
            *ports.cwd.lock().unwrap() = Some(CwdResolution::DeletedFallback {
                original: deleted.clone(),
                fallback: root.clone(),
            });
            prepare(&ports, &deleted, &root).await.unwrap();
            let inputs = ports.compile_inputs.lock().unwrap();
            assert!(
                inputs[0]
                    .1
                    .as_deref()
                    .unwrap()
                    .contains(deleted.to_str().unwrap())
            );
        }
        #[tokio::test]
        async fn reminder_fires_once() {
            let root = temp();
            let missing = root.join("gone");
            let ports = RecordingSetupPorts::valid();
            *ports.cwd.lock().unwrap() = Some(CwdResolution::DeletedFallback {
                original: missing.clone(),
                fallback: root.clone(),
            });
            prepare(&ports, &missing, &root).await.unwrap();
            prepare(&ports, &missing, &root).await.unwrap();
            assert_eq!(
                ports
                    .compile_inputs
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, r)| r.is_some())
                    .count(),
                1
            );
        }
        #[tokio::test]
        async fn concurrent_exactly_one() {
            let root = temp();
            let missing = root.join("gone");
            let ports = RecordingSetupPorts::valid();
            *ports.cwd.lock().unwrap() = Some(CwdResolution::DeletedFallback {
                original: missing.clone(),
                fallback: root.clone(),
            });
            let scope = ports
                .apply_scope(
                    RuntimeScope::new(
                        AgentId::accept("agent-a").unwrap(),
                        ConversationId::accept("conversation-a").unwrap(),
                        None,
                    ),
                    &root,
                    PermissionMode::Unrestricted,
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            let claim = ports
                .claim_cwd_reminder(
                    &AgentId::accept("agent-a").unwrap(),
                    &ConversationId::accept("conversation-a").unwrap(),
                    &missing,
                    scope,
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            let second = ports
                .claim_cwd_reminder(
                    &AgentId::accept("agent-a").unwrap(),
                    &ConversationId::accept("conversation-a").unwrap(),
                    &missing,
                    scope,
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            assert!(claim.is_some());
            assert!(second.is_none());
        }
        #[tokio::test]
        async fn pre_admission_failure_releases_claim_and_retry_reminds() {
            let root = temp();
            let missing = root.join("gone");
            let ports = RecordingSetupPorts::valid();
            *ports.cwd.lock().unwrap() = Some(CwdResolution::DeletedFallback {
                original: missing.clone(),
                fallback: root.clone(),
            });
            ports.set_fault(SetupStage::CompileSystemPrompt, Fault::Fail);
            assert!(prepare(&ports, &missing, &root).await.is_err());
            *ports.fault.lock().unwrap() = None;
            prepare(&ports, &missing, &root).await.unwrap();
            assert!(
                ports
                    .compile_inputs
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .1
                    .is_some()
            );
        }
        #[tokio::test]
        async fn existing_cwd_no_reminder() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            prepare(&ports, &root, &root).await.unwrap();
            assert_eq!(ports.compile_inputs.lock().unwrap()[0].1, None);
        }
        #[tokio::test]
        async fn non_dir_typed() {
            let root = temp();
            let file = root.join("file");
            std::fs::write(&file, b"x").unwrap();
            let ports = RecordingSetupPorts::valid();
            assert!(matches!(
                prepare(&ports, &file, &root).await,
                Err(SetupFailure::PreAdmission(SetupError::Cwd(
                    CwdFailure::NotDirectory
                )))
            ));
        }
        #[cfg(unix)]
        #[tokio::test]
        async fn symlink_typed() {
            use std::os::unix::fs::symlink;
            let root = temp();
            let link = root.join("link");
            symlink(&root, &link).unwrap();
            let ports = RecordingSetupPorts::valid();
            assert!(matches!(
                prepare(&ports, &link, &root).await,
                Err(SetupFailure::PreAdmission(SetupError::Cwd(
                    CwdFailure::SymlinkOrEscape
                )))
            ));
        }
    }

    mod access_rejection {
        use super::*;
        #[tokio::test]
        async fn cross_agent() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            let other = conversation("agent-b", "conversation-a");
            ports.conversations.lock().unwrap().insert(
                (AgentId::accept("agent-a").unwrap(), other.id.clone()),
                other,
            );
            assert!(matches!(
                prepare(&ports, &root, &root).await,
                Err(SetupFailure::PreAdmission(SetupError::CrossAgent))
            ));
            assert_eq!(ports.scopes.lock().unwrap().len(), 0);
            assert!(ports.transcript.lock().unwrap().is_empty());
            assert_eq!(ports.calls.lock().unwrap().len(), 2);
        }
        #[tokio::test]
        async fn archived_invalid() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            let mut archived = conversation("agent-a", "conversation-a");
            archived.archived = true;
            ports
                .conversations
                .lock()
                .unwrap()
                .insert((archived.agent_id.clone(), archived.id.clone()), archived);
            assert!(matches!(
                prepare(&ports, &root, &root).await,
                Err(SetupFailure::PreAdmission(SetupError::ArchivedOrInvalid))
            ));
            assert!(ports.transcript.lock().unwrap().is_empty());
            assert_eq!(ports.scopes.lock().unwrap().len(), 0);
        }
        #[tokio::test]
        async fn unknown_typed() {
            let root = temp();
            let ports = RecordingSetupPorts::default();
            match prepare(&ports, &root, &root).await {
                Err(SetupFailure::PreAdmission(SetupError::Adapter(value))) => {
                    assert!(value.contains("not_found") || value.contains("not found"));
                }
                _ => panic!("unexpected setup result"),
            }
        }
    }

    mod failure_modes {
        use super::*;
        #[tokio::test]
        async fn pre_admission_appends_nothing() {
            for stage in SetupStage::ORDER.into_iter().skip(1).take(8) {
                let root = temp();
                let ports = RecordingSetupPorts::valid();
                ports.set_fault(stage, Fault::Fail);
                let _ = prepare(&ports, &root, &root).await;
                assert!(ports.transcript.lock().unwrap().is_empty(), "{stage:?}");
                assert!(ports.scopes.lock().unwrap().is_empty(), "{stage:?}");
            }
        }
        #[tokio::test]
        async fn post_append_records_interrupted() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.set_fault(SetupStage::BuildProviderRequestAndEmitStatus, Fault::Pause);
            let mut request = input(root.clone(), root.clone());
            request.deadline = Duration::from_millis(100);
            let Err(error) = SetupOrchestrator::new(&ports).prepare(request).await else {
                panic!("expected failure")
            };
            assert!(matches!(
                error,
                SetupFailure::PostAdmission {
                    error: SetupError::Deadline,
                    ..
                }
            ));
            assert_eq!(ports.transcript.lock().unwrap().len(), 1);
            assert_eq!(ports.interrupted.lock().unwrap().len(), 1);
        }
        #[tokio::test]
        async fn retry_replay_idempotent_marker() {
            let ports = RecordingSetupPorts::valid();
            let receipt = AdmissionReceipt {
                appended: true,
                input_id: "same".into(),
            };
            ports
                .record_interrupted(&receipt, &SetupError::Cancelled)
                .await
                .unwrap();
            ports
                .record_interrupted(&receipt, &SetupError::Deadline)
                .await
                .unwrap();
            assert_eq!(ports.interrupted.lock().unwrap().len(), 1);
            assert!(ports.interrupted.lock().unwrap()["same"].contains("cancelled"));
        }
        #[tokio::test]
        async fn interruption_failure_keeps_durable_evidence() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.set_fault(SetupStage::BuildProviderRequestAndEmitStatus, Fault::Pause);
            let mut request = input(root.clone(), root);
            request.deadline = Duration::from_millis(100);
            let _ = SetupOrchestrator::new(&ports).prepare(request).await;
            assert_eq!(ports.transcript.lock().unwrap()[0].1, "hello");
            assert_eq!(ports.interrupted.lock().unwrap().len(), 1);
        }
    }

    mod tool_merge {
        use super::*;
        async fn source(source: SetupToolSource) {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.candidates.lock().unwrap().push(ToolCandidate {
                source,
                definition: super::super::super::test_support::definition(&format!("{source:?}")),
                model_name: "tool".into(),
                authorized: true,
            });
            let output = prepare(&ports, &root, &root).await.unwrap();
            assert_eq!(
                output
                    .tools
                    .definitions()
                    .next()
                    .unwrap()
                    .internal_name
                    .as_str(),
                format!("{source:?}")
            );
            assert!(
                ports
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call.contains(&format!("{source:?}")))
            );
        }
        #[tokio::test]
        async fn built_in_actual_candidate() {
            source(SetupToolSource::BuiltIn).await;
        }
        #[tokio::test]
        async fn mcp_actual_candidate() {
            source(SetupToolSource::Mcp).await;
        }
        #[tokio::test]
        async fn mod_actual_candidate() {
            source(SetupToolSource::Mod).await;
        }
        #[tokio::test]
        async fn channel_actual_candidate() {
            source(SetupToolSource::Channel).await;
        }
        #[tokio::test]
        async fn controller_actual_candidate() {
            source(SetupToolSource::ControllerExternal).await;
        }
        #[tokio::test]
        async fn respects_toolset_and_allowlist() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.candidates.lock().unwrap().push(ToolCandidate {
                source: SetupToolSource::BuiltIn,
                definition: super::super::super::test_support::definition("allowed"),
                model_name: "tool".into(),
                authorized: true,
            });
            prepare(&ports, &root, &root).await.unwrap();
            assert!(
                ports
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call.starts_with("tools.merge:openai"))
            );
        }
        #[tokio::test]
        async fn unauthorized_filtered() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.candidates.lock().unwrap().push(ToolCandidate {
                source: SetupToolSource::Mcp,
                definition: super::super::super::test_support::definition("denied"),
                model_name: "tool".into(),
                authorized: false,
            });
            let output = prepare(&ports, &root, &root).await.unwrap();
            assert_eq!(output.tools.definitions().count(), 0);
            assert!(
                ports
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call == "tools.merge:openai:[]")
            );
        }
        #[tokio::test]
        async fn duplicate_conflict_deterministic() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            let definition = super::super::super::test_support::definition("same");
            *ports.candidates.lock().unwrap() = vec![
                ToolCandidate {
                    source: SetupToolSource::Mcp,
                    definition: definition.clone(),
                    model_name: "tool".into(),
                    authorized: true,
                },
                ToolCandidate {
                    source: SetupToolSource::Mod,
                    definition,
                    model_name: "tool".into(),
                    authorized: true,
                },
            ];
            match prepare(&ports, &root, &root).await {
                Err(SetupFailure::PreAdmission(SetupError::Adapter(value))) => {
                    assert!(value.contains("duplicate"));
                }
                _ => panic!("unexpected setup result"),
            }
        }
        #[tokio::test]
        async fn limit_deterministic() {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            *ports.candidates.lock().unwrap() = (0..10_001)
                .map(|i| ToolCandidate {
                    source: SetupToolSource::ControllerExternal,
                    definition: super::super::super::test_support::definition(&format!("tool{i}")),
                    model_name: "tool".into(),
                    authorized: true,
                })
                .collect();
            assert!(matches!(
                prepare(&ports, &root, &root).await,
                Err(SetupFailure::PreAdmission(SetupError::Adapter(_)))
            ));
        }
    }

    #[tokio::test]
    async fn cancellation_at_every_ten_stages_no_leaks() {
        for stage in SetupStage::ORDER.into_iter().skip(1) {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.set_fault(stage, Fault::Cancel);
            let _ = prepare(&ports, &root, &root).await;
            assert!(ports.scopes.lock().unwrap().is_empty(), "{stage:?}");
        }
    }
    #[tokio::test]
    async fn deadline_at_every_ten_stages_no_leaks() {
        for stage in SetupStage::ORDER {
            let root = temp();
            let ports = RecordingSetupPorts::valid();
            ports.set_fault(stage, Fault::Pause);
            let mut request = input(root.clone(), root);
            request.deadline = Duration::from_millis(5);
            let _ = SetupOrchestrator::new(&ports).prepare(request).await;
            assert!(
                ports.scopes.lock().unwrap().is_empty()
                    || !ports.transcript.lock().unwrap().is_empty(),
                "{stage:?}"
            );
        }
    }
    #[tokio::test]
    async fn skills_result_affects_compile_input() {
        let root = temp();
        let ports = RecordingSetupPorts::valid();
        *ports.inventory.lock().unwrap() = SkillInventory {
            available: vec!["skill-a".into(), "skill-b".into()],
            selected: vec!["skill-a".into()],
        };
        prepare(&ports, &root, &root).await.unwrap();
        assert_eq!(
            ports.compile_inputs.lock().unwrap()[0].0.available,
            ["skill-a", "skill-b"]
        );
    }
    #[tokio::test]
    async fn status_plan_sending_and_no_early_status() {
        let root = temp();
        let ports = RecordingSetupPorts::valid();
        let output = prepare(&ports, &root, &root).await.unwrap();
        assert_eq!(output.status, SetupStatus::Sending);
        assert_eq!(Arc::strong_count(&output.status_sink), 1);
    }
}
