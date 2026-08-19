//! Runtime-scoped structured events and bounded metrics.

/// Safe structured runtime events and event sinks.
pub mod events;
/// Bounded deterministic runtime metrics.
pub mod metrics;

#[cfg(test)]
include!("tests.rs");

use crate::RuntimeKey;
use events::{ObservedStopReason, RuntimeEvent, RuntimeEventKind, RuntimeEventSink, inert_sink};
use lotta_domain::{NonEmptyString, RunId, TurnLease};
use metrics::{MetricFamily, RuntimeMetrics};
use std::sync::Arc;

/// One shareable observer injected through a runtime component graph.
#[derive(Clone)]
pub struct RuntimeObserver {
    sink: Arc<dyn RuntimeEventSink>,
    metrics: Arc<RuntimeMetrics>,
}

impl Default for RuntimeObserver {
    fn default() -> Self {
        Self {
            sink: inert_sink(),
            metrics: Arc::new(RuntimeMetrics::default()),
        }
    }
}

impl RuntimeObserver {
    /// Creates an observer using the supplied bounded sink and isolated registry.
    #[must_use]
    pub fn new(sink: Arc<dyn RuntimeEventSink>) -> Self {
        Self {
            sink,
            metrics: Arc::new(RuntimeMetrics::default()),
        }
    }

    /// Emits without allowing sink failure to alter a turn outcome.
    pub fn emit(&self, event: RuntimeEvent) {
        let _ignored = self.sink.try_emit(event);
    }

    /// Increments one required counter family.
    pub fn increment(&self, family: MetricFamily) {
        self.metrics.increment(family);
    }

    /// Sets one required gauge family to its authoritative value.
    pub fn set_gauge(&self, family: MetricFamily, value: u64) {
        self.metrics.set_gauge(family, value);
    }

    /// Records one required bounded latency histogram observation.
    pub fn observe_latency(&self, family: MetricFamily, value_ms: u64) {
        self.metrics.observe_latency(family, value_ms);
    }

    /// Records an accepted or queued admission with authoritative gauges.
    pub fn admission(
        &self,
        key: &RuntimeKey,
        connection_id: &NonEmptyString,
        lease: Option<&TurnLease>,
        run_id: Option<&RunId>,
        queue_length: usize,
        active_turns: u64,
    ) {
        let mut event = RuntimeEvent::new(
            if queue_length == 0 {
                RuntimeEventKind::Admission
            } else {
                RuntimeEventKind::Queue
            },
            key,
            connection_id,
            lease.map_or(0, TurnLease::generation),
        )
        .with_queue_length(queue_length);
        if let Some(run_id) = run_id {
            event = event.with_run(run_id);
        }
        self.increment(MetricFamily::Admissions);
        self.set_gauge(MetricFamily::QueueDepth, queue_length as u64);
        self.set_gauge(MetricFamily::ActiveTurns, active_turns);
        self.emit(event);
    }

    /// Records a retry and emits only for an actual retry transition.
    pub fn retry(&self, event: RuntimeEvent) {
        self.increment(MetricFamily::Retries);
        self.emit(event);
    }

    /// Records a completed compaction.
    pub fn compaction(&self, event: RuntimeEvent) {
        self.increment(MetricFamily::Compactions);
        self.emit(event);
    }

    /// Records a provider attempt duration.
    pub fn provider_attempt(&self, event: RuntimeEvent, duration_ms: u64) {
        self.observe_latency(MetricFamily::ProviderLatency, duration_ms);
        self.emit(event.with_duration_ms(duration_ms));
    }

    /// Records a tool execution duration.
    pub fn tool_execution(&self, event: RuntimeEvent, duration_ms: u64) {
        self.observe_latency(MetricFamily::ToolDuration, duration_ms);
        self.emit(event.with_duration_ms(duration_ms));
    }

    /// Records cancellation settlement from an actual elapsed clock duration.
    pub fn cancellation_settled(&self, event: RuntimeEvent, duration_ms: u64) {
        self.observe_latency(MetricFamily::CancellationLatency, duration_ms);
        self.emit(
            event
                .with_stop_reason(ObservedStopReason::Cancelled)
                .with_duration_ms(duration_ms),
        );
    }

    /// Records suppression at the shared lease guard boundary.
    pub fn stale_suppression(&self, event: RuntimeEvent) {
        self.increment(MetricFamily::StaleSuppressions);
        self.emit(event.with_stop_reason(ObservedStopReason::Suppressed));
    }

    /// Records a terminal outcome and clears the active-turn gauge.
    pub fn terminal(&self, event: RuntimeEvent) {
        self.increment(MetricFamily::TerminalOutcomes);
        self.set_gauge(MetricFamily::ActiveTurns, 0);
        self.emit(event);
    }

    /// Returns this observer's isolated metrics registry.
    #[must_use]
    pub fn metrics(&self) -> &RuntimeMetrics {
        &self.metrics
    }
}
