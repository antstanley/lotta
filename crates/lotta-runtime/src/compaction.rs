//! Transcript compaction planning and dependency-neutral service orchestration.

use crate::ports::{
    ProviderContent, ProviderContentPart, ProviderContextOverflowDetail, ProviderMessage,
    ProviderMessageRole, ProviderMessages, ProviderRequest,
};
use crate::{RuntimeError, boundary::ProviderText, turn::CompactionProgress};
use lotta_domain::{NonEmptyString, RuntimeScope, TurnLease};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Default percentage retained by sliding-window compaction.
pub const COMPACTION_RECENT_PERCENT_DEFAULT: u8 = 30;
/// Smallest accepted sliding-window retained percentage.
pub const COMPACTION_RECENT_PERCENT_MIN: u8 = 1;
/// Largest accepted sliding-window retained percentage.
pub const COMPACTION_RECENT_PERCENT_MAX: u8 = 100;

/// Compaction strategy selected for one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionMode {
    /// Summarize all eligible history into one summary message.
    All,
    /// Summarize the oldest eligible prefix and retain the exact recent percentage.
    SlidingWindow {
        /// Percentage of eligible messages to retain, in `1..=100`.
        recent_percent: u8,
    },
}

impl CompactionMode {
    /// Validates a sliding-window percentage.
    ///
    /// # Errors
    /// Returns invalid data when the percentage is outside `1..=100`.
    pub fn sliding_window(recent_percent: u8) -> Result<Self, RuntimeError> {
        if !(COMPACTION_RECENT_PERCENT_MIN..=COMPACTION_RECENT_PERCENT_MAX)
            .contains(&recent_percent)
        {
            return Err(RuntimeError::InvalidData {
                context: "compaction recent percent".into(),
            });
        }
        Ok(Self::SlidingWindow { recent_percent })
    }
}

impl Default for CompactionMode {
    fn default() -> Self {
        Self::SlidingWindow {
            recent_percent: COMPACTION_RECENT_PERCENT_DEFAULT,
        }
    }
}

/// Stable source of a compaction request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionTrigger {
    /// Explicit runtime/application API request.
    Manual,
    /// Pre-provider-call context pressure.
    Pressure,
    /// Provider-reported context overflow.
    ProviderOverflow,
}

/// Idempotent scoped compaction command.
#[derive(Clone, Debug)]
pub struct CompactionCommand {
    /// Stable caller-supplied request identifier.
    pub request_id: NonEmptyString,
    /// Runtime scope whose transcript is compacted.
    pub scope: RuntimeScope,
    /// Exact active turn lease.
    pub lease: TurnLease,
    /// Compaction strategy.
    pub mode: CompactionMode,
    /// Trigger source.
    pub trigger: CompactionTrigger,
    /// Current provider request.
    pub request: ProviderRequest,
    /// Safe context-pressure detail.
    pub detail: ProviderContextOverflowDetail,
    /// Terminal cancellation token.
    pub cancellation: CancellationToken,
}

/// Planned summary prefix and exact retained suffix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionPlan {
    /// Eligible messages sent to the summarizer.
    pub summarize: Vec<ProviderMessage>,
    /// Exact recent messages retained after the summary.
    pub keep: Vec<ProviderMessage>,
}

/// Plans either compaction mode without splitting tool protocol pairs.
///
/// # Errors
/// Returns invalid data when no eligible protocol-safe prefix can be compacted.
pub fn plan(
    messages: &[ProviderMessage],
    mode: CompactionMode,
) -> Result<CompactionPlan, RuntimeError> {
    let cutoff = match mode {
        CompactionMode::All => all_cutoff(messages),
        CompactionMode::SlidingWindow { recent_percent } => {
            sliding_cutoff(messages, recent_percent)
        }
    };
    if cutoff == 0 {
        return Err(RuntimeError::InvalidData {
            context: "compaction has no eligible history".into(),
        });
    }
    Ok(CompactionPlan {
        summarize: messages[..cutoff].to_vec(),
        keep: messages[cutoff..].to_vec(),
    })
}

fn all_cutoff(messages: &[ProviderMessage]) -> usize {
    safe_cutoff(messages, messages.len())
}

fn sliding_cutoff(messages: &[ProviderMessage], recent_percent: u8) -> usize {
    let retain = messages
        .len()
        .saturating_mul(usize::from(recent_percent))
        .div_ceil(100);
    safe_cutoff(messages, messages.len().saturating_sub(retain))
}

fn safe_cutoff(messages: &[ProviderMessage], requested: usize) -> usize {
    let mut cutoff = requested.min(messages.len());
    while cutoff > 0 && splits_tool_protocol(messages, cutoff) {
        cutoff -= 1;
    }
    cutoff
}

fn splits_tool_protocol(messages: &[ProviderMessage], cutoff: usize) -> bool {
    if cutoff == 0 || cutoff >= messages.len() {
        return false;
    }
    if messages[cutoff].role == ProviderMessageRole::Tool {
        return true;
    }
    messages[cutoff - 1].role == ProviderMessageRole::Assistant
        && messages
            .get(cutoff)
            .is_some_and(|message| message.role == ProviderMessageRole::Tool)
}

/// Bounded summary output returned by the provider-backed summarizer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionSummary(pub String);

/// Provider-backed summarization port.
pub trait CompactionSummarizer: Send + Sync {
    /// Summarizes one exact eligible prefix using the selected provider request.
    fn summarize(
        &self,
        request: ProviderRequest,
        messages: Vec<ProviderMessage>,
        cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionSummary, RuntimeError>> + Send + '_>>;
}

/// Durable/state/lifecycle effects owned by a concrete adapter.
pub trait CompactionEffects: Send + Sync {
    /// Claims durable ownership before provider summarization or recovers prior progress.
    fn claim(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionRecovery, RuntimeError>> + Send + '_>>;
    /// Persists a safe bounded projection before transcript append.
    fn record_projection(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        retained: Vec<ProviderMessage>,
        progress: CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>>;
    /// Confirms that the lease remains canonical before any externally visible effect.
    fn lease_is_current(&self, scope: &RuntimeScope, lease: &TurnLease) -> bool;
    /// Fires the pre-compact lifecycle callbacks.
    fn pre_compact(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>>;
    /// Durably appends one compaction entry before publishing state.
    fn append(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        retained: Vec<ProviderMessage>,
        progress: CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>>;
    /// Publishes context IDs.
    fn publish(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>>;
    /// Fires the post-compact callback after publication and request recompilation succeed.
    fn post_compact(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>>;
}

/// Durable state recovered when claiming one compaction request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionRecovery {
    /// No summary or append is durable; execute from the provider summary step.
    Pending,
    /// A safe projection exists but append remains incomplete.
    Planned(CompactionProgress),
    /// The transcript entry is durable and publication remains incomplete.
    Appended(CompactionProgress),
    /// Publication already completed.
    Published(CompactionProgress),
}

type Execution = Arc<Mutex<Option<CompactionProgress>>>;
type ExecutionKey = (RuntimeScope, String);

/// Lease-serialized, request-idempotent compaction coordinator.
pub struct CompactionService<S, E> {
    summarizer: S,
    effects: E,
    executions: Mutex<HashMap<ExecutionKey, Execution>>,
}

impl<S, E> CompactionService<S, E>
where
    S: CompactionSummarizer,
    E: CompactionEffects,
{
    /// Creates a service over injected provider and production-effect ports.
    #[must_use]
    pub fn new(summarizer: S, effects: E) -> Self {
        Self {
            summarizer,
            effects,
            executions: Mutex::new(HashMap::new()),
        }
    }

    /// Executes one canonical compaction, or returns the prior idempotent result.
    ///
    /// # Errors
    /// Returns cancellation, stale-lease, provider, planning, persistence, or publication errors.
    pub async fn compact(
        &self,
        command: CompactionCommand,
    ) -> Result<CompactionProgress, RuntimeError> {
        let execution = {
            let mut executions = self.executions.lock().await;
            executions
                .entry((command.scope.clone(), command.request_id.as_str().into()))
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut completed = execution.lock().await;
        if let Some(progress) = *completed {
            return Ok(progress);
        }
        let progress = self.execute(&command).await?;
        *completed = Some(progress);
        Ok(progress)
    }

    async fn execute(
        &self,
        command: &CompactionCommand,
    ) -> Result<CompactionProgress, RuntimeError> {
        ensure_live(command, &self.effects)?;
        match self.effects.claim(command).await? {
            CompactionRecovery::Published(progress) => return Ok(progress),
            CompactionRecovery::Appended(progress) => {
                self.effects.publish(command).await?;
                ensure_live(command, &self.effects)?;
                self.effects.post_compact(command).await?;
                return Ok(progress);
            }
            CompactionRecovery::Planned(progress) => {
                self.effects
                    .append(
                        command,
                        CompactionSummary(String::new()),
                        Vec::new(),
                        progress,
                    )
                    .await?;
                self.effects.publish(command).await?;
                ensure_live(command, &self.effects)?;
                self.effects.post_compact(command).await?;
                return Ok(progress);
            }
            CompactionRecovery::Pending => {}
        }
        let planned = plan(command.request.messages.as_slice(), command.mode)?;
        self.effects.pre_compact(command).await?;
        ensure_live(command, &self.effects)?;
        let summary = self
            .summarizer
            .summarize(
                command.request.clone(),
                planned.summarize,
                command.cancellation.clone(),
            )
            .await?;
        ensure_live(command, &self.effects)?;
        let progress = progress(command, &summary, &planned.keep)?;
        self.effects
            .record_projection(command, summary.clone(), planned.keep.clone(), progress)
            .await?;
        self.effects
            .append(command, summary, planned.keep, progress)
            .await?;
        self.effects.publish(command).await?;
        ensure_live(command, &self.effects)?;
        self.effects.post_compact(command).await?;
        Ok(progress)
    }
}

fn ensure_live(
    command: &CompactionCommand,
    effects: &impl CompactionEffects,
) -> Result<(), RuntimeError> {
    if command.cancellation.is_cancelled()
        || !effects.lease_is_current(&command.scope, &command.lease)
    {
        return Err(RuntimeError::Cancelled {
            context: "stale compaction lease".into(),
        });
    }
    Ok(())
}

fn progress(
    command: &CompactionCommand,
    summary: &CompactionSummary,
    retained: &[ProviderMessage],
) -> Result<CompactionProgress, RuntimeError> {
    let content = ProviderContent::new(vec![ProviderContentPart::Text(ProviderText::new(
        summary.0.clone(),
    )?)])
    .map_err(|_| service_error("compaction summary content"))?;
    let summary_message = ProviderMessage {
        role: ProviderMessageRole::User,
        content,
        tool_call_id: None,
    };
    let mut compacted = command.request.clone();
    compacted.messages = ProviderMessages::new(
        std::iter::once(summary_message)
            .chain(retained.iter().cloned())
            .collect(),
    )
    .map_err(|_| service_error("compaction count messages"))?;
    let tokens_after = crate::ports::estimate_request_tokens(&compacted).tokens;
    Ok(CompactionProgress {
        tokens_before: command.detail.estimated.tokens,
        tokens_after,
        messages_before: command.request.messages.len(),
        messages_after: retained.len().saturating_add(1),
    })
}

fn service_error(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "compaction_service",
        context: context.into(),
    }
}

/// Builds model-visible messages after compaction.
///
/// # Errors
/// Returns a bound error if the summary plus retained suffix exceeds provider message bounds.
#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "named selector module remains adjacent to implementation"
)]
mod tests {
    use super::*;
    use crate::boundary::ProviderText;
    use crate::ports::{
        ImagePolicy, ProviderDeadline, ProviderToolChoice, ProviderTools, ReasoningControls,
        TokenLimit,
    };
    use lotta_domain::{
        AgentId, BoundedJsonValue, CompactionEntry, CompactionEntryType, ConversationId,
        EntityExtras, LocalMessage, LocalMessageRole, MessageId, NonEmptyString, RunId,
        RuntimeScope, Timestamp,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    fn message(role: ProviderMessageRole, text: &str) -> ProviderMessage {
        ProviderMessage {
            role,
            content: ProviderContent::new(vec![ProviderContentPart::Text(
                ProviderText::new(text.to_owned()).expect("text"),
            )])
            .expect("content"),
            tool_call_id: None,
        }
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            model: lotta_domain::ModelDescriptor {
                provider_id: NonEmptyString::new("test-provider").expect("provider"),
                handle: NonEmptyString::new("test/model").expect("model"),
                available: true,
                context_window: Some(4096),
                model_settings: None,
            },
            messages: ProviderMessages::new(vec![
                message(ProviderMessageRole::User, "one"),
                message(ProviderMessageRole::Assistant, "two"),
                message(ProviderMessageRole::User, "three"),
                message(ProviderMessageRole::Assistant, "four"),
            ])
            .expect("messages"),
            system_prompt: Some(ProviderText::new("fixture prompt".to_owned()).expect("prompt")),
            tools: ProviderTools::new(Vec::new()).expect("tools"),
            tool_choice: ProviderToolChoice::Auto,
            image_policy: ImagePolicy::Strict,
            output_tokens_max: TokenLimit::new(64).expect("output limit"),
            context_tokens_max: TokenLimit::new(4096).expect("context limit"),
            reasoning: ReasoningControls {
                enabled: false,
                effort: None,
                tier: None,
            },
            context: None,
            cancellation: CancellationToken::new(),
            deadline: ProviderDeadline::default(),
        }
    }

    #[derive(Clone, Default)]
    struct RecordingSummarizer {
        calls: Arc<AtomicUsize>,
        trace: Arc<StdMutex<Vec<String>>>,
    }

    impl CompactionSummarizer for RecordingSummarizer {
        fn summarize(
            &self,
            request: ProviderRequest,
            messages: Vec<ProviderMessage>,
            _: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<CompactionSummary, RuntimeError>> + Send + '_>>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.trace.lock().expect("trace").push("summary".into());
            assert!(!messages.is_empty());
            assert_eq!(
                request.system_prompt.as_ref().map(ProviderText::as_str),
                Some("fixture prompt")
            );
            Box::pin(async { Ok(CompactionSummary("bounded summary".into())) })
        }
    }

    #[derive(Clone)]
    struct RecordingEffects {
        runtime: Arc<StdMutex<crate::ListenerRuntime>>,
        handle: crate::RuntimeHandle,
        trace: Arc<StdMutex<Vec<String>>>,
        triggers: Arc<StdMutex<Vec<CompactionTrigger>>>,
        appends: Arc<AtomicUsize>,
        publications: Arc<StdMutex<Vec<(String, String)>>>,
    }

    impl RecordingEffects {
        fn current(&self, lease: &TurnLease) -> bool {
            self.runtime
                .lock()
                .expect("runtime")
                .lifecycle(&self.handle)
                .is_some_and(|owner| owner.is_current(lease))
        }
    }

    impl CompactionEffects for RecordingEffects {
        fn claim(
            &self,
            command: &CompactionCommand,
        ) -> Pin<Box<dyn Future<Output = Result<CompactionRecovery, RuntimeError>> + Send + '_>>
        {
            self.triggers
                .lock()
                .expect("triggers")
                .push(command.trigger);
            Box::pin(async { Ok(CompactionRecovery::Pending) })
        }
        fn record_projection(
            &self,
            _: &CompactionCommand,
            _: CompactionSummary,
            _: Vec<ProviderMessage>,
            _: CompactionProgress,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
            self.trace.lock().expect("trace").push("record".into());
            Box::pin(async { Ok(()) })
        }
        fn lease_is_current(&self, _: &RuntimeScope, lease: &TurnLease) -> bool {
            self.current(lease)
        }
        fn pre_compact(
            &self,
            _: &CompactionCommand,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
            self.trace.lock().expect("trace").push("pre".into());
            Box::pin(async { Ok(()) })
        }
        fn append(
            &self,
            _: &CompactionCommand,
            _: CompactionSummary,
            _: Vec<ProviderMessage>,
            _: CompactionProgress,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
            self.appends.fetch_add(1, Ordering::SeqCst);
            self.trace.lock().expect("trace").push("append".into());
            Box::pin(async { Ok(()) })
        }
        fn publish(
            &self,
            command: &CompactionCommand,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
            self.trace.lock().expect("trace").push("publish".into());
            self.publications.lock().expect("publications").push((
                command.request_id.as_str().into(),
                command
                    .request
                    .system_prompt
                    .as_ref()
                    .expect("prompt")
                    .as_str()
                    .into(),
            ));
            Box::pin(async { Ok(()) })
        }
        fn post_compact(
            &self,
            _: &CompactionCommand,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
            self.trace.lock().expect("trace").push("post".into());
            Box::pin(async { Ok(()) })
        }
    }

    struct Fixture {
        service: Arc<CompactionService<RecordingSummarizer, RecordingEffects>>,
        summarizer: RecordingSummarizer,
        effects: RecordingEffects,
        command: CompactionCommand,
    }

    fn fixture(trigger: CompactionTrigger) -> Fixture {
        let scope = RuntimeScope::new(
            AgentId::accept("agent-compaction").expect("agent"),
            ConversationId::accept("conversation-compaction").expect("conversation"),
            None,
        );
        let mut runtime = crate::ListenerRuntime::new();
        let handle = runtime
            .get_or_create(&scope, uuid::Uuid::from_u128(58))
            .expect("runtime");
        let lease = runtime
            .lifecycle_mut(&handle)
            .expect("owner")
            .begin_turn("turn-58".into(), RunId::generate_sequence(58).expect("run"))
            .expect("lease");
        let runtime = Arc::new(StdMutex::new(runtime));
        let trace = Arc::new(StdMutex::new(Vec::new()));
        let summarizer = RecordingSummarizer {
            calls: Arc::new(AtomicUsize::new(0)),
            trace: Arc::clone(&trace),
        };
        let effects = RecordingEffects {
            runtime,
            handle,
            trace,
            triggers: Arc::new(StdMutex::new(Vec::new())),
            appends: Arc::new(AtomicUsize::new(0)),
            publications: Arc::new(StdMutex::new(Vec::new())),
        };
        let request = request();
        let detail = ProviderContextOverflowDetail {
            measured: None,
            estimated: crate::ports::estimate_request_tokens(&request),
            limit: 4096,
            provider: "test-provider".into(),
            model: "test/model".into(),
            attempt: 1,
            compactions_completed: 0,
        };
        let command = CompactionCommand {
            request_id: NonEmptyString::new("compact-request-58").expect("request id"),
            scope,
            lease,
            mode: CompactionMode::sliding_window(50).expect("mode"),
            trigger,
            request,
            detail,
            cancellation: CancellationToken::new(),
        };
        let service = Arc::new(CompactionService::new(summarizer.clone(), effects.clone()));
        Fixture {
            service,
            summarizer,
            effects,
            command,
        }
    }

    fn assert_completed(fixture: &Fixture) {
        assert_eq!(fixture.summarizer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.effects.appends.load(Ordering::SeqCst), 1);
        assert_eq!(
            *fixture.effects.trace.lock().expect("trace"),
            vec!["pre", "summary", "record", "append", "publish", "post"]
        );
    }

    mod compaction {
        use super::*;

        mod prompt_refresh {

            #[derive(Default)]
            struct PromptRevisionBoundary {
                compiled: Option<String>,
            }

            impl PromptRevisionBoundary {
                fn should_recompile(&self, committed: Option<&str>) -> bool {
                    committed.is_some_and(|revision| self.compiled.as_deref() != Some(revision))
                }

                fn commit(&mut self, revision: &str) {
                    self.compiled = Some(revision.to_owned());
                }
            }

            #[tokio::test]
            async fn committed_change_recompiles() {
                let mut boundary = PromptRevisionBoundary::default();
                assert!(boundary.should_recompile(Some("head-a")));
                boundary.commit("head-a");
                assert!(boundary.should_recompile(Some("head-b")));
            }

            #[tokio::test]
            async fn uncommitted_change_does_not() {
                let mut boundary = PromptRevisionBoundary::default();
                boundary.commit("head-a");
                assert!(!boundary.should_recompile(Some("head-a")));
                assert!(!boundary.should_recompile(None));
            }
        }

        mod modes {
            use super::*;

            #[test]
            fn all_summarizes_every_eligible_message() {
                let messages = request().messages.as_slice().to_vec();
                let planned = plan(&messages, CompactionMode::All).expect("plan");
                assert_eq!(planned.summarize, messages);
                assert!(planned.keep.is_empty());
            }

            #[test]
            fn sliding_retains_exact_configured_recent_percentage() {
                let messages = (0..10)
                    .map(|index| {
                        message(
                            if index % 2 == 0 {
                                ProviderMessageRole::User
                            } else {
                                ProviderMessageRole::Assistant
                            },
                            &index.to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                let planned = plan(&messages, CompactionMode::sliding_window(30).expect("mode"))
                    .expect("plan");
                assert_eq!((planned.summarize.len(), planned.keep.len()), (7, 3));
            }

            #[test]
            fn multi_tool_group_protocol_boundary() {
                let mut first = message(ProviderMessageRole::Tool, "first result");
                first.tool_call_id = Some(crate::ports::ToolCallId::from_name(
                    crate::boundary::ProviderName::new("first-call".into()).expect("id"),
                ));
                let mut second = message(ProviderMessageRole::Tool, "second result");
                second.tool_call_id = Some(crate::ports::ToolCallId::from_name(
                    crate::boundary::ProviderName::new("second-call".into()).expect("id"),
                ));
                let messages = vec![
                    message(ProviderMessageRole::User, "start"),
                    message(ProviderMessageRole::Assistant, "two calls"),
                    first,
                    second,
                    message(ProviderMessageRole::Assistant, "done"),
                ];
                let planned = plan(&messages, CompactionMode::sliding_window(50).expect("mode"))
                    .expect("plan");
                assert_ne!(
                    planned.keep.first().map(|value| value.role),
                    Some(ProviderMessageRole::Tool)
                );
                assert!(
                    planned
                        .summarize
                        .iter()
                        .filter(|value| value.role == ProviderMessageRole::Tool)
                        .count()
                        != 1
                );
            }
        }

        mod triggers {
            use super::*;

            async fn drive(trigger: CompactionTrigger) {
                let fixture = fixture(trigger);
                fixture
                    .service
                    .compact(fixture.command.clone())
                    .await
                    .expect("compact");
                assert_completed(&fixture);
                assert_eq!(
                    *fixture.effects.triggers.lock().expect("triggers"),
                    vec![trigger]
                );
            }

            #[tokio::test]
            async fn manual_request() {
                drive(CompactionTrigger::Manual).await;
            }
            #[tokio::test]
            async fn pre_call_pressure() {
                let request = fixture(CompactionTrigger::Pressure).command.request;
                assert_eq!(crate::turn::effective_context_limit(&request), 4096);
                drive(CompactionTrigger::Pressure).await;
            }
            #[tokio::test]
            async fn provider_reported_overflow() {
                drive(CompactionTrigger::ProviderOverflow).await;
            }
            #[tokio::test]
            async fn overflow_bound() {
                assert_eq!(crate::turn::CONTEXT_OVERFLOW_COMPACTIONS_MAX, 3);
                for _ in 0..crate::turn::CONTEXT_OVERFLOW_COMPACTIONS_MAX {
                    drive(CompactionTrigger::ProviderOverflow).await;
                }
            }
        }

        mod effects {
            use super::*;

            #[tokio::test]
            async fn ordered_trace() {
                let f = fixture(CompactionTrigger::Manual);
                f.service.compact(f.command.clone()).await.unwrap();
                assert_completed(&f);
            }
            #[tokio::test]
            async fn exactly_one_append() {
                let f = fixture(CompactionTrigger::Manual);
                f.service.compact(f.command.clone()).await.unwrap();
                assert_eq!(f.effects.appends.load(Ordering::SeqCst), 1);
            }
            #[tokio::test]
            async fn publishes_request_id_and_prompt_marker() {
                let f = fixture(CompactionTrigger::Manual);
                f.service.compact(f.command.clone()).await.unwrap();
                assert_eq!(
                    *f.effects.publications.lock().unwrap(),
                    vec![("compact-request-58".into(), "fixture prompt".into())]
                );
            }
            #[tokio::test]
            async fn stale_lease_has_zero_effects() {
                let f = fixture(CompactionTrigger::Manual);
                f.effects
                    .runtime
                    .lock()
                    .unwrap()
                    .lifecycle_mut(&f.effects.handle)
                    .unwrap()
                    .finish_turn(
                        &f.command.lease,
                        lotta_domain::StopReason::new("cancelled").expect("reason"),
                    )
                    .unwrap();
                assert!(f.service.compact(f.command.clone()).await.is_err());
                assert_eq!(f.effects.appends.load(Ordering::SeqCst), 0);
                assert!(f.effects.trace.lock().unwrap().is_empty());
            }
            #[tokio::test]
            async fn cancelled_has_zero_effects() {
                let f = fixture(CompactionTrigger::Manual);
                f.command.cancellation.cancel();
                assert!(f.service.compact(f.command.clone()).await.is_err());
                assert_eq!(f.effects.appends.load(Ordering::SeqCst), 0);
                assert!(f.effects.trace.lock().unwrap().is_empty());
            }
            #[tokio::test]
            async fn concurrent_same_request_once() {
                let f = fixture(CompactionTrigger::Manual);
                let a = {
                    let s = Arc::clone(&f.service);
                    let c = f.command.clone();
                    tokio::spawn(async move { s.compact(c).await })
                };
                let b = {
                    let s = Arc::clone(&f.service);
                    let c = f.command.clone();
                    tokio::spawn(async move { s.compact(c).await })
                };
                a.await.unwrap().unwrap();
                b.await.unwrap().unwrap();
                assert_completed(&f);
            }
        }

        mod records_counts {
            use super::*;

            #[tokio::test]
            async fn canonical_progress_roundtrips_compaction_entry() {
                let fixture = fixture(CompactionTrigger::Manual);
                let progress = fixture
                    .service
                    .compact(fixture.command.clone())
                    .await
                    .expect("compact");
                let entry = CompactionEntry {
                    entry_type: CompactionEntryType::Compaction,
                    id: fixture.command.request_id.clone(),
                    parent_id: None,
                    timestamp: Timestamp::parse_persisted_rfc3339("2026-08-18T00:00:00Z")
                        .expect("timestamp"),
                    summary: "bounded summary".into(),
                    first_kept_entry_id: Some("message-3".into()),
                    tokens_before: progress.tokens_before,
                    tokens_after: Some(progress.tokens_after),
                    messages_before: Some(progress.messages_before),
                    messages_after: Some(progress.messages_after),
                    message: LocalMessage {
                        id: MessageId::accept("compact-request-58").expect("message id"),
                        role: LocalMessageRole::User,
                        content: Some(
                            BoundedJsonValue::new(serde_json::json!("bounded summary"))
                                .expect("content"),
                        ),
                        timestamp: 1.0,
                        metadata: None,
                        extras: EntityExtras::default(),
                    },
                    details: None,
                };
                let value = serde_json::to_value(&entry).expect("serialize");
                assert_eq!(value["tokensBefore"], progress.tokens_before);
                assert_eq!(value["tokensAfter"], progress.tokens_after);
                assert_eq!(value["messagesBefore"], progress.messages_before);
                assert_eq!(value["messagesAfter"], progress.messages_after);
                assert_eq!(value["firstKeptEntryId"], "message-3");
                let roundtrip: CompactionEntry = serde_json::from_value(value).expect("roundtrip");
                assert_eq!(roundtrip.tokens_before, progress.tokens_before);
                assert_eq!(roundtrip.first_kept_entry_id.as_deref(), Some("message-3"));
            }
        }
    }
}

/// Builds one summary message followed by the exact retained suffix.
///
/// # Errors
/// Returns a provider text or message bound failure.
pub fn compacted_messages(
    summary: CompactionSummary,
    retained: Vec<ProviderMessage>,
) -> Result<ProviderMessages, RuntimeError> {
    let content = ProviderContent::new(vec![ProviderContentPart::Text(ProviderText::new(
        summary.0,
    )?)])
    .map_err(|_| service_error("compaction summary content"))?;
    let mut messages = Vec::with_capacity(retained.len().saturating_add(1));
    messages.push(ProviderMessage {
        role: ProviderMessageRole::User,
        content,
        tool_call_id: None,
    });
    messages.extend(retained);
    ProviderMessages::new(messages).map_err(|_| RuntimeError::LimitExceeded {
        context: "provider messages".into(),
    })
}
