//! Configurable redaction-safe tool fake.

use super::{limit, lock};
use crate::TESTKIT_ITEMS_MAX;
use crate::contract::{ToolContractObservation, ToolContractProbe, ToolContractSnapshot};
use lotta_runtime::RuntimeError;
use lotta_runtime::bounds::TOOL_INPUT_BYTES_MAX;
use lotta_runtime::ports::{
    ParallelSafety, SecretRedactionPolicy, ToolApprovalPolicy, ToolExecutionOwner,
    ToolExecutionRequest, ToolOutcome, ToolOutcomeMessage, ToolPort,
};
use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

const FAKE_TOOL_CALLS_MAX: u32 = 1_024;

/// Exact compact JSON byte count retained in one fixed-width value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolInputBytes(u32);

impl ToolInputBytes {
    /// Returns the exact compact JSON byte count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Redaction-safe observation retained for one fake tool request.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolObservation {
    /// Stable internal tool name.
    pub internal_name: String,
    /// Resolved model-facing name.
    pub model_name: String,
    /// Exact executor owner.
    pub execution_owner: ToolExecutionOwner,
    /// Conservative parallel-safety classification.
    pub parallel_safety: ParallelSafety,
    /// Definition approval policy.
    pub approval_policy: ToolApprovalPolicy,
    /// Permission action label.
    pub permission_action: String,
    /// Definition execution timeout.
    pub timeout: Duration,
    /// Definition output byte ceiling.
    pub output_bytes_max: usize,
    /// Definition output model-character ceiling.
    pub output_model_chars_max: usize,
    /// Secret-field count, without retaining paths or values.
    pub secret_field_count: usize,
    /// Secret redaction policy.
    pub secret_redaction_policy: SecretRedactionPolicy,
    /// Deadline requested by the caller.
    pub deadline: Duration,
    /// Whether cancellation was terminal when the call was observed.
    pub cancelled: bool,
    /// Exact compact JSON byte count, without retaining input.
    pub input_bytes: ToolInputBytes,
}

impl fmt::Debug for ToolObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolObservation")
            .field("internal_name", &self.internal_name)
            .field("model_name", &self.model_name)
            .field("execution_owner", &self.execution_owner)
            .field("parallel_safety", &self.parallel_safety)
            .field("approval_policy", &self.approval_policy)
            .field("permission_action", &self.permission_action)
            .field("timeout", &self.timeout)
            .field("output_bytes_max", &self.output_bytes_max)
            .field("output_model_chars_max", &self.output_model_chars_max)
            .field("secret_field_count", &self.secret_field_count)
            .field("secret_redaction_policy", &self.secret_redaction_policy)
            .field("deadline", &self.deadline)
            .field("cancelled", &self.cancelled)
            .field("input_bytes", &self.input_bytes)
            .finish()
    }
}

#[derive(Debug, Default)]
struct ToolState {
    responses: VecDeque<Result<ToolOutcome, RuntimeError>>,
    observations: Vec<ToolObservation>,
    in_flight: u32,
    max_in_flight: u32,
    completed_calls: u32,
}

/// In-memory tool fake returning configured outcomes or runtime failures in FIFO order.
#[derive(Debug)]
pub struct FakeTool {
    state: Mutex<ToolState>,
    sequential: Arc<Semaphore>,
    pending: Mutex<Option<Arc<Notify>>>,
    pending_admissions: Mutex<u32>,
    entered: Arc<Notify>,
    probe: Mutex<Option<ToolContractProbe>>,
}

impl Default for FakeTool {
    fn default() -> Self {
        Self {
            state: Mutex::new(ToolState::default()),
            sequential: Arc::new(Semaphore::new(1)),
            pending: Mutex::new(None),
            pending_admissions: Mutex::new(0),
            entered: Arc::new(Notify::new()),
            probe: Mutex::new(None),
        }
    }
}

impl FakeTool {
    /// Replaces the bounded response queue and resets observations and counters atomically.
    ///
    /// # Errors
    /// Rejects response lists above the focused testkit item bound.
    pub fn configure(
        &self,
        responses: Vec<Result<ToolOutcome, RuntimeError>>,
    ) -> Result<(), RuntimeError> {
        if responses.len() > TESTKIT_ITEMS_MAX {
            return Err(limit("fake_tool_responses_max"));
        }
        let mut state = lock(&self.state);
        state.responses = responses.into();
        state.observations.clear();
        state.in_flight = 0;
        state.max_in_flight = 0;
        state.completed_calls = 0;
        Ok(())
    }

    /// Connects a testkit-owned contract probe to this adapter wrapper.
    pub fn attach_probe(&self, probe: ToolContractProbe) {
        *lock(&self.probe) = Some(probe);
        self.publish_probe();
    }

    fn publish_probe(&self) {
        let Some(probe) = lock(&self.probe).clone() else {
            return;
        };
        let state = lock(&self.state);
        let pending_calls = *lock(&self.pending_admissions);
        probe.publish(ToolContractSnapshot {
            observations: state.observations.iter().map(Into::into).collect(),
            max_in_flight: state.max_in_flight,
            completed_calls: state.completed_calls,
            pending_calls: pending_calls.max(state.in_flight),
            setup_bounds_verified: false,
        });
    }

    /// Makes each admitted execution report a deterministic pending boundary.
    pub fn hold_pending(&self) {
        *lock(&self.pending_admissions) = 1;
        self.publish_probe();
    }

    /// Releases all calls currently waiting at the pending gate.
    pub fn release_pending(&self) {
        if let Some(gate) = lock(&self.pending).take() {
            gate.notify_waiters();
        }
    }

    /// Stops gating future calls without waking calls already pending at the gate.
    pub fn clear_pending(&self) {
        lock(&self.pending).take();
    }

    /// Waits until an execution has acquired sequential admission and reached the pending gate.
    pub async fn wait_until_pending(&self) {
        loop {
            if *lock(&self.pending_admissions) > 0 {
                return;
            }
            let notified = self.entered.notified();
            if *lock(&self.pending_admissions) > 0 {
                return;
            }
            notified.await;
        }
    }

    /// Returns bounded, redaction-safe request observations.
    #[must_use]
    pub fn observations(&self) -> Vec<ToolObservation> {
        lock(&self.state).observations.clone()
    }

    /// Returns the greatest number of simultaneously admitted calls.
    #[must_use]
    pub fn max_in_flight(&self) -> u32 {
        lock(&self.state).max_in_flight
    }

    /// Returns the bounded count of terminal calls, including cancellation and adapter errors.
    #[must_use]
    pub fn completed_calls(&self) -> u32 {
        lock(&self.state).completed_calls
    }

    #[cfg(test)]
    fn seed_counters(&self, in_flight: u32, completed: u32, admissions: u32) {
        let mut state = lock(&self.state);
        state.in_flight = in_flight;
        state.completed_calls = completed;
        *lock(&self.pending_admissions) = admissions;
    }
}

struct BoundedJsonCounter {
    count: usize,
}

impl Write for BoundedJsonCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .count
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("tool input byte count overflow"))?;
        if next > TOOL_INPUT_BYTES_MAX.value {
            return Err(io::Error::other("tool input byte count limit"));
        }
        self.count = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn input_bytes(value: &serde_json::Value) -> Result<ToolInputBytes, RuntimeError> {
    let mut counter = BoundedJsonCounter { count: 0 };
    serde_json::to_writer(&mut counter, value).map_err(|_| limit(TOOL_INPUT_BYTES_MAX.name))?;
    let count = u32::try_from(counter.count).map_err(|_| limit(TOOL_INPUT_BYTES_MAX.name))?;
    Ok(ToolInputBytes(count))
}

fn observe(request: &ToolExecutionRequest) -> Result<ToolObservation, RuntimeError> {
    let definition = &request.definition;
    Ok(ToolObservation {
        internal_name: definition.internal_name.as_str().into(),
        model_name: definition.model_name.as_str().into(),
        execution_owner: definition.execution_owner,
        parallel_safety: definition.parallel_safety.clone(),
        approval_policy: definition.approval_policy,
        permission_action: definition.permission_action.as_str().into(),
        timeout: definition.timeout.get(),
        output_bytes_max: definition.output_limit.bytes_max(),
        output_model_chars_max: definition.output_limit.model_chars_max(),
        secret_field_count: definition.secret_redaction.fields().len(),
        secret_redaction_policy: definition.secret_redaction.policy(),
        deadline: request.deadline.get(),
        cancelled: request.cancellation.is_cancelled(),
        input_bytes: input_bytes(request.input.as_value())?,
    })
}

fn interrupted() -> Result<ToolOutcome, RuntimeError> {
    Ok(ToolOutcome::Interrupted {
        message: ToolOutcomeMessage::new("cancelled".into())?,
    })
}

impl From<&ToolObservation> for ToolContractObservation {
    fn from(value: &ToolObservation) -> Self {
        Self {
            internal_name: value.internal_name.clone(),
            model_name: value.model_name.clone(),
            execution_owner: value.execution_owner,
            parallel_safety: value.parallel_safety.clone(),
            approval_policy: value.approval_policy,
            permission_action: value.permission_action.clone(),
            timeout: value.timeout,
            output_bytes_max: value.output_bytes_max,
            output_model_chars_max: value.output_model_chars_max,
            secret_field_count: value.secret_field_count,
            deadline: value.deadline,
            cancelled: value.cancelled,
            input_bytes: value.input_bytes.get(),
        }
    }
}

impl FakeTool {
    fn begin_call(&self, observation: ToolObservation) -> Result<(), RuntimeError> {
        let mut state = lock(&self.state);
        if state.observations.len() >= TESTKIT_ITEMS_MAX {
            return Err(limit("fake_tool_observations_max"));
        }
        let in_flight = state
            .in_flight
            .checked_add(1)
            .filter(|value| *value <= FAKE_TOOL_CALLS_MAX)
            .ok_or_else(|| limit("fake_tool_in_flight_max"))?;
        state.observations.push(observation);
        state.in_flight = in_flight;
        state.max_in_flight = state.max_in_flight.max(in_flight);
        Ok(())
    }

    async fn await_gate(&self, request: &ToolExecutionRequest) -> Result<bool, RuntimeError> {
        let gate = lock(&self.pending).clone();
        let scripted_pending = *lock(&self.pending_admissions) > 0;
        if request.cancellation.is_cancelled() {
            if scripted_pending {
                *lock(&self.pending_admissions) = 0;
            }
            return Ok(true);
        }
        let Some(gate) = gate else { return Ok(false) };
        self.update_pending(true)?;
        self.entered.notify_waiters();
        let cancelled = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => true,
            () = gate.notified() => false,
        };
        self.update_pending(false)?;
        Ok(cancelled)
    }

    fn update_pending(&self, entering: bool) -> Result<(), RuntimeError> {
        let mut admissions = lock(&self.pending_admissions);
        *admissions = if entering {
            admissions
                .checked_add(1)
                .filter(|value| *value <= FAKE_TOOL_CALLS_MAX)
                .ok_or_else(|| limit("fake_tool_pending_admissions_max"))?
        } else {
            admissions
                .checked_sub(1)
                .ok_or_else(|| limit("fake_tool_pending_admissions_underflow"))?
        };
        Ok(())
    }

    fn finish_call(
        &self,
        consume_response: bool,
    ) -> Result<Option<Result<ToolOutcome, RuntimeError>>, RuntimeError> {
        let mut state = lock(&self.state);
        let result = consume_response.then(|| {
            state.responses.pop_front().unwrap_or_else(|| {
                Err(RuntimeError::AdapterFailure {
                    code: "testkit_tool_script_exhausted",
                    context: "fake tool".into(),
                })
            })
        });
        state.in_flight = state
            .in_flight
            .checked_sub(1)
            .ok_or_else(|| limit("fake_tool_in_flight_underflow"))?;
        state.completed_calls = state
            .completed_calls
            .checked_add(1)
            .filter(|value| *value <= FAKE_TOOL_CALLS_MAX)
            .ok_or_else(|| limit("fake_tool_completed_calls_max"))?;
        Ok(result)
    }
}

impl ToolPort for FakeTool {
    fn execute(
        &self,
        request: ToolExecutionRequest,
    ) -> lotta_runtime::ports::PortFuture<'_, ToolOutcome> {
        Box::pin(async move {
            let permit =
                self.sequential
                    .acquire()
                    .await
                    .map_err(|_| RuntimeError::AdapterFailure {
                        code: "testkit_tool_semaphore_closed",
                        context: "fake tool".into(),
                    })?;
            self.begin_call(observe(&request)?)?;
            self.publish_probe();
            let cancelled = self.await_gate(&request).await?;
            let result = if cancelled {
                self.finish_call(false)?;
                interrupted()
            } else {
                self.finish_call(true)?
                    .expect("consuming call has a scripted result")
            };
            self.publish_probe();
            drop(permit);
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_error(code: &'static str) -> Result<ToolOutcome, RuntimeError> {
        Err(RuntimeError::AdapterFailure {
            code,
            context: "tool fake test".into(),
        })
    }

    #[tokio::test]
    async fn counters_reject_boundaries_without_mutation() {
        let request = crate::tests::tool_request();
        let tool = FakeTool::default();
        tool.configure(vec![adapter_error("unused")])
            .expect("setup");
        tool.seed_counters(FAKE_TOOL_CALLS_MAX, 7, 0);
        assert!(matches!(
            tool.execute(request.clone()).await,
            Err(RuntimeError::LimitExceeded { ref context })
                if context == "fake_tool_in_flight_max"
        ));
        assert_eq!(tool.max_in_flight(), 0);
        assert_eq!(tool.completed_calls(), 7);

        tool.configure(vec![adapter_error("terminal")])
            .expect("setup");
        tool.seed_counters(0, FAKE_TOOL_CALLS_MAX, 0);
        assert!(matches!(
            tool.execute(request).await,
            Err(RuntimeError::LimitExceeded { ref context })
                if context == "fake_tool_completed_calls_max"
        ));
        assert_eq!(tool.completed_calls(), FAKE_TOOL_CALLS_MAX);
    }

    #[tokio::test]
    async fn configuration_replacement_capacity_and_exhaustion() {
        let tool = FakeTool::default();
        tool.configure(Vec::new()).expect("empty setup");
        assert!(tool.observations().is_empty());
        let oversized = (0..=TESTKIT_ITEMS_MAX)
            .map(|_| adapter_error("bounded"))
            .collect();
        assert!(tool.configure(oversized).is_err());
        tool.configure(vec![adapter_error("first")])
            .expect("first setup");
        tool.configure(vec![adapter_error("replacement")])
            .expect("replacement setup");
        let request = crate::tests::tool_request();
        assert!(matches!(
            tool.execute(request.clone()).await,
            Err(RuntimeError::AdapterFailure {
                code: "replacement",
                ..
            })
        ));
        assert!(matches!(
            tool.execute(request).await,
            Err(RuntimeError::AdapterFailure {
                code: "testkit_tool_script_exhausted",
                ..
            })
        ));
    }
}
