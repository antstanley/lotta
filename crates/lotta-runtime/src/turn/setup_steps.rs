use super::setup::{
    AdmissionReceipt, CwdResolution, SetupError, SetupFailure, SetupInput, SetupOrchestrator,
    SetupOutput, SetupStage, SetupStatus,
};
use lotta_domain::{Agent, Conversation};

struct ClaimedContext<'a> {
    agent: &'a Agent,
    conversation: &'a Conversation,
    cwd: &'a CwdResolution,
    inventory: &'a super::setup::SkillInventory,
    reminder: Option<&'a super::setup::ReminderClaim>,
}

impl SetupOrchestrator<'_> {
    pub(super) async fn prepare_inner(
        &self,
        input: SetupInput,
    ) -> Result<SetupOutput, SetupFailure> {
        let mut stages = Vec::with_capacity(SetupStage::ORDER.len());
        let (agent, conversation) = self.resolve_records(&input).await?;
        stages.push(SetupStage::ResolveAgentAndConversation);

        self.check_ready(&input)?;
        let cwd = self
            .ports
            .resolve_cwd(&input.cwd, &input.fallback_cwd)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::ResolveCwd);

        self.check_ready(&input)?;
        let runtime = lotta_domain::RuntimeScope::new(
            input.agent_id.clone(),
            input.conversation_id.clone(),
            None,
        );
        let scope = self
            .ports
            .apply_scope(
                runtime,
                cwd.effective(),
                input.permission_mode,
                &input.cancellation,
            )
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::ApplyWorkspaceSandboxAndPermissions);

        let result = self
            .finish_pre_admission(&input, &agent, &conversation, &cwd, scope, &mut stages)
            .await;
        if result.is_err() {
            self.ports.release_scope(scope);
        }
        result
    }

    async fn finish_pre_admission(
        &self,
        input: &SetupInput,
        agent: &Agent,
        conversation: &Conversation,
        cwd: &CwdResolution,
        scope: super::setup::SetupScopeHandle,
        stages: &mut Vec<SetupStage>,
    ) -> Result<SetupOutput, SetupFailure> {
        self.check_ready(input)?;
        self.ports
            .prepare_memfs(agent, &input.cancellation)
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::SynchronizeOrInitializeMemFs);

        self.check_ready(input)?;
        let inventory = self
            .ports
            .skill_inventory(agent, cwd.effective(), &input.selected_skills)
            .map_err(SetupFailure::pre)?;
        let reminder = self.reminder_claim(input, cwd, scope).await?;
        let context = ClaimedContext {
            agent,
            conversation,
            cwd,
            inventory: &inventory,
            reminder: reminder.as_ref(),
        };
        let result = self.finish_claimed(input, context, scope, stages).await;
        if result.is_err() {
            let _ = self
                .release_reminder(reminder.as_ref(), &input.cancellation)
                .await;
        }
        result
    }

    async fn finish_claimed(
        &self,
        input: &SetupInput,
        context: ClaimedContext<'_>,
        scope: super::setup::SetupScopeHandle,
        stages: &mut Vec<SetupStage>,
    ) -> Result<SetupOutput, SetupFailure> {
        let prompt = self.compile(input, &context, scope, stages).await?;
        let (model, extensions) = self.model_and_extensions(input, &context, stages).await?;
        let tools = self.tools(input, scope, &model, &extensions, stages)?;
        self.admit_and_finish(
            input,
            prompt,
            model,
            tools,
            extensions,
            context.reminder.cloned(),
            scope,
            stages,
        )
        .await
    }

    async fn compile(
        &self,
        input: &SetupInput,
        context: &ClaimedContext<'_>,
        scope: super::setup::SetupScopeHandle,
        stages: &mut Vec<SetupStage>,
    ) -> Result<String, SetupFailure> {
        let prompt = self
            .ports
            .compile_prompt(
                context.agent,
                context.conversation,
                context.inventory,
                scope,
                context.reminder.map(|claim| claim.message.as_str()),
                &input.cancellation,
            )
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::CompileSystemPrompt);
        Ok(prompt)
    }

    async fn model_and_extensions(
        &self,
        input: &SetupInput,
        context: &ClaimedContext<'_>,
        stages: &mut Vec<SetupStage>,
    ) -> Result<
        (
            super::setup::ResolvedTurnModel,
            super::setup::ExtensionSnapshot,
        ),
        SetupFailure,
    > {
        self.check_ready(input)?;
        let model = self
            .ports
            .resolve_model(context.agent, context.conversation)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::ResolveModelProviderContextAndToolset);
        self.check_ready(input)?;
        let selected = self
            .ports
            .discover_selected(context.inventory, &input.selected_skills)
            .map_err(SetupFailure::pre)?;
        if selected != context.inventory.selected {
            return Err(SetupFailure::pre(SetupError::SkillSelection));
        }
        stages.push(SetupStage::DiscoverSelectedSkills);
        self.check_ready(input)?;
        let extensions = self
            .ports
            .load_extensions(context.agent, context.cwd.effective(), &input.cancellation)
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::LoadModsAndHooks);
        Ok((model, extensions))
    }

    fn tools(
        &self,
        input: &SetupInput,
        scope: super::setup::SetupScopeHandle,
        model: &super::setup::ResolvedTurnModel,
        extensions: &super::setup::ExtensionSnapshot,
        stages: &mut Vec<SetupStage>,
    ) -> Result<super::TurnToolCatalog, SetupFailure> {
        self.check_ready(input)?;
        let candidates = self
            .ports
            .tool_candidates(scope, extensions, &input.cancellation)
            .map_err(SetupFailure::pre)?;
        let authorized = candidates
            .into_iter()
            .filter(|candidate| candidate.authorized)
            .collect();
        let tools = self
            .ports
            .merge_tools(model, authorized)
            .map_err(SetupFailure::pre)?;
        stages.push(SetupStage::MergeTools);
        Ok(tools)
    }

    async fn release_reminder(
        &self,
        claim: Option<&super::setup::ReminderClaim>,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), SetupFailure> {
        if let Some(claim) = claim {
            self.ports
                .release_cwd_reminder(claim, cancellation)
                .await
                .map_err(SetupError::from)
                .map_err(SetupFailure::pre)?;
        }
        Ok(())
    }

    async fn resolve_records(
        &self,
        input: &SetupInput,
    ) -> Result<(Agent, Conversation), SetupFailure> {
        self.check_ready(input)?;
        let agent = self
            .ports
            .agents()
            .load(&input.agent_id)
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        self.check_ready(input)?;
        let conversation = self
            .ports
            .resolve_conversation(&input.agent_id, &input.conversation_id, &input.cancellation)
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        if conversation.agent_id != input.agent_id {
            return Err(SetupFailure::pre(SetupError::CrossAgent));
        }
        if conversation.archived || matches!(conversation.archived_at, Some(Some(_))) {
            return Err(SetupFailure::pre(SetupError::ArchivedOrInvalid));
        }
        Ok((agent, conversation))
    }

    async fn commit_admission(
        &self,
        input: &SetupInput,
        reminder: Option<&super::setup::ReminderClaim>,
    ) -> Result<super::setup::AdmissionReceipt, SetupFailure> {
        self.check_ready(input)?;
        let receipt = self
            .ports
            .admit_input(
                &input.agent_id,
                &input.conversation_id,
                &input.user_input,
                reminder,
            )
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        if !receipt.appended {
            return self
                .post_admission(
                    receipt,
                    SetupError::Adapter("admission did not append input".into()),
                )
                .await;
        }
        if let Some(claim) = reminder
            && let Err(error) = self
                .ports
                .commit_cwd_reminder(claim, &receipt, &input.cancellation)
                .await
                .map_err(SetupError::from)
        {
            return self.post_admission(receipt, error).await;
        }
        self.check_post_admission(input, &receipt).await?;
        Ok(receipt)
    }

    #[allow(clippy::too_many_arguments, reason = "explicit setup artifacts")]
    async fn admit_and_finish(
        &self,
        input: &SetupInput,
        prompt: String,
        model: super::setup::ResolvedTurnModel,
        tools: super::TurnToolCatalog,
        extensions: super::setup::ExtensionSnapshot,
        reminder: Option<super::setup::ReminderClaim>,
        scope: super::setup::SetupScopeHandle,
        stages: &mut Vec<SetupStage>,
    ) -> Result<SetupOutput, SetupFailure> {
        self.check_ready(input)?;
        let request = self
            .ports
            .build_request(
                &input.agent_id,
                &input.conversation_id,
                prompt.clone(),
                &input.user_input,
                reminder.as_ref().map(super::setup::ReminderClaim::input_id),
                &model,
                &tools,
                input.cancellation.clone(),
            )
            .await
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        request
            .validate_bytes()
            .map_err(SetupError::from)
            .map_err(SetupFailure::pre)?;
        let receipt = self.commit_admission(input, reminder.as_ref()).await?;
        stages.push(SetupStage::BuildProviderRequestAndEmitStatus);
        Ok(SetupOutput {
            request,
            fallback_candidates: model.fallback_candidates.clone(),
            model,
            prompt,
            input: input.user_input.clone(),
            tools,
            stages: std::mem::take(stages),
            admission: receipt,
            extensions,
            scope,
            status_sink: std::sync::Arc::clone(&input.status_sink),
            status: SetupStatus::Sending,
        })
    }

    async fn reminder_claim(
        &self,
        input: &SetupInput,
        cwd: &CwdResolution,
        scope: super::setup::SetupScopeHandle,
    ) -> Result<Option<super::setup::ReminderClaim>, SetupFailure> {
        match cwd {
            CwdResolution::Requested(_) => Ok(None),
            CwdResolution::DeletedFallback { original, .. } => self
                .ports
                .claim_cwd_reminder(
                    &input.agent_id,
                    &input.conversation_id,
                    original,
                    scope,
                    &input.cancellation,
                )
                .await
                .map_err(SetupError::from)
                .map_err(SetupFailure::pre),
        }
    }

    async fn check_post_admission(
        &self,
        input: &SetupInput,
        receipt: &AdmissionReceipt,
    ) -> Result<(), SetupFailure> {
        if input.cancellation.is_cancelled() {
            return self
                .post_admission(receipt.clone(), SetupError::Cancelled)
                .await;
        }
        if self.check_deadline().is_err() {
            return self
                .post_admission(receipt.clone(), SetupError::Deadline)
                .await;
        }
        Ok(())
    }

    async fn post_admission<T>(
        &self,
        receipt: AdmissionReceipt,
        error: SetupError,
    ) -> Result<T, SetupFailure> {
        let record_error = self.ports.record_interrupted(&receipt, &error).await.err();
        if let Some(record) = record_error {
            tracing::error!(
                %record,
                input_id = %receipt.input_id,
                "failed to record interrupted turn"
            );
        }
        Err(SetupFailure::PostAdmission { receipt, error })
    }

    fn check_ready(&self, input: &SetupInput) -> Result<(), SetupFailure> {
        if input.cancellation.is_cancelled() {
            Err(SetupFailure::pre(SetupError::Cancelled))
        } else {
            self.check_deadline()
        }
    }
}
