use super::{
    ProviderStartPort, SetupFailure, SetupInput, SetupOrchestrator, SetupPorts, SetupStatus,
    TurnPorts, TurnRunOutcome,
};
use crate::{ListenerRuntime, RuntimeError, RuntimeHandle};
use lotta_domain::TurnLease;

/// Setup boundary accepted by the production turn caller.
pub trait TurnSetup: Send + Sync {
    /// Performs concrete setup and durable input admission.
    fn prepare(&self, input: SetupInput) -> crate::ports::PortFuture<'_, super::SetupOutput>;
}

impl TurnSetup for SetupOrchestrator<'_> {
    fn prepare(&self, input: SetupInput) -> crate::ports::PortFuture<'_, super::SetupOutput> {
        Box::pin(async move {
            self.prepare(input)
                .await
                .map_err(|error| setup_failure(&error))
        })
    }
}

/// Runs setup, emits dispatch lifecycle status, then enters the Task 19 turn loop.
///
/// # Errors
/// Returns typed setup or turn-loop failures.
#[allow(
    clippy::too_many_arguments,
    reason = "explicit production port boundary"
)]
pub async fn run_turn_with_setup<'ports>(
    runtime: &mut ListenerRuntime,
    handle: RuntimeHandle,
    lease: TurnLease,
    setup: &dyn TurnSetup,
    setup_ports: &dyn SetupPorts,
    input: SetupInput,
    turn_ports: impl for<'catalog> FnOnce(
        &'catalog super::TurnToolCatalog,
        &'ports dyn crate::ports::ProviderPort,
        &'ports dyn crate::ports::ToolPort,
        &'ports dyn super::TurnEffectPort,
    ) -> TurnPorts<'ports, 'catalog>,
    provider: &'ports dyn crate::ports::ProviderPort,
    tools: &'ports dyn crate::ports::ToolPort,
    effects: &'ports dyn super::TurnEffectPort,
) -> Result<TurnRunOutcome, RuntimeError> {
    let prepared = setup.prepare(input).await?;
    debug_assert_eq!(prepared.status, SetupStatus::Sending);
    let status = DispatchStatus { ports: setup_ports };
    let ports = turn_ports(&prepared.tools, provider, tools, effects).with_provider_start(&status);
    let result = super::run_turn(runtime, handle, lease, prepared.request, ports).await;
    if let Err(error) = &result {
        setup_ports
            .record_interrupted(
                &prepared.admission,
                &super::SetupError::Adapter(error.to_string()),
            )
            .await?;
    }
    result
}

struct DispatchStatus<'a> {
    ports: &'a dyn SetupPorts,
}

impl ProviderStartPort for DispatchStatus<'_> {
    fn provider_start(&self) -> crate::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            self.ports
                .emit_status(SetupStatus::Sending)
                .map_err(|error| setup_error(&error))?;
            Ok(())
        })
    }

    fn provider_waiting(&self) -> crate::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            self.ports
                .emit_status(SetupStatus::Waiting)
                .map_err(|error| setup_error(&error))
        })
    }
}

fn setup_failure(error: &SetupFailure) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "turn_setup",
        context: format!("{error:?}"),
    }
}

fn setup_error(error: &super::SetupError) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "turn_setup_status",
        context: error.to_string(),
    }
}
