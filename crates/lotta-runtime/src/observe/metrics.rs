//! Bounded, deterministic runtime metrics.

use std::sync::Mutex;

/// Fixed upper histogram bucket boundaries in milliseconds.
pub const LATENCY_BUCKETS_MS: [u64; 8] = [1, 5, 10, 50, 100, 1_000, 10_000, 300_000];

/// The ten stable runtime metric family names in deterministic order.
pub const METRIC_FAMILY_NAMES: [&str; 10] = [
    "runtime_admissions_total",
    "runtime_queue_depth",
    "runtime_active_turns",
    "runtime_cancellation_latency_ms",
    "runtime_retries_total",
    "runtime_compactions_total",
    "runtime_provider_latency_ms",
    "runtime_tool_duration_ms",
    "runtime_stale_suppressions_total",
    "runtime_terminal_outcomes_total",
];

/// Stable bounded metric family selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricFamily {
    /// Admitted inputs.
    Admissions,
    /// Current queue depth observation.
    QueueDepth,
    /// Active-turn gauge observation.
    ActiveTurns,
    /// Cancellation latency observation in milliseconds.
    CancellationLatency,
    /// Provider retries.
    Retries,
    /// Compactions.
    Compactions,
    /// Provider latency observation in milliseconds.
    ProviderLatency,
    /// Tool duration observation in milliseconds.
    ToolDuration,
    /// Stale-lease suppressions.
    StaleSuppressions,
    /// Terminal outcomes.
    TerminalOutcomes,
}

/// Exact semantics for one metric family snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricValue {
    /// Monotonic event count.
    Counter(u64),
    /// Current value, updated up and down.
    Gauge(u64),
    /// Bounded cumulative latency histogram.
    Histogram {
        /// Number of observations.
        count: u64,
        /// Saturating sum in milliseconds.
        sum: u64,
        /// Cumulative counts for [`LATENCY_BUCKETS_MS`].
        buckets: [u64; LATENCY_BUCKETS_MS.len()],
    },
}

/// One deterministic metric snapshot row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricSnapshot {
    /// Stable family name.
    pub name: &'static str,
    /// Family value with exact counter, gauge, or histogram semantics.
    pub value: MetricValue,
}

#[derive(Debug)]
struct MetricState {
    counters: [u64; 5],
    gauges: [u64; 2],
    histograms: [(u64, u64, [u64; LATENCY_BUCKETS_MS.len()]); 3],
}

impl Default for MetricState {
    fn default() -> Self {
        Self {
            counters: [0; 5],
            gauges: [0; 2],
            histograms: [(0, 0, [0; LATENCY_BUCKETS_MS.len()]); 3],
        }
    }
}

/// Shared bounded-cardinality registry for exactly ten required families.
#[derive(Debug, Default)]
pub struct RuntimeMetrics {
    state: Mutex<MetricState>,
}

impl RuntimeMetrics {
    /// Increments a counter family by one.
    pub fn increment(&self, family: MetricFamily) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = counter_index(family) {
            state.counters[index] = state.counters[index].saturating_add(1);
        }
    }

    /// Sets a gauge family to its current authoritative value.
    pub fn set_gauge(&self, family: MetricFamily, value: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = gauge_index(family) {
            state.gauges[index] = value;
        }
    }

    /// Records one bounded latency observation.
    pub fn observe_latency(&self, family: MetricFamily, value_ms: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(index) = histogram_index(family) else {
            return;
        };
        let histogram = &mut state.histograms[index];
        histogram.0 = histogram.0.saturating_add(1);
        histogram.1 = histogram.1.saturating_add(value_ms);
        for (bucket, upper) in histogram.2.iter_mut().zip(LATENCY_BUCKETS_MS) {
            if value_ms <= upper {
                *bucket = bucket.saturating_add(1);
            }
        }
    }

    /// Returns exactly ten rows in stable family order.
    #[must_use]
    pub fn snapshot(&self) -> [MetricSnapshot; 10] {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::array::from_fn(|index| snapshot_row(index, &state))
    }
}

fn snapshot_row(index: usize, state: &MetricState) -> MetricSnapshot {
    let value = if let Some(counter) = counter_index_at(index) {
        MetricValue::Counter(state.counters[counter])
    } else if let Some(gauge) = gauge_index_at(index) {
        MetricValue::Gauge(state.gauges[gauge])
    } else {
        let (count, sum, buckets) = state.histograms[histogram_index_at(index)];
        MetricValue::Histogram {
            count,
            sum,
            buckets,
        }
    };
    MetricSnapshot {
        name: METRIC_FAMILY_NAMES[index],
        value,
    }
}

const fn counter_index(family: MetricFamily) -> Option<usize> {
    match family {
        MetricFamily::Admissions => Some(0),
        MetricFamily::Retries => Some(1),
        MetricFamily::Compactions => Some(2),
        MetricFamily::StaleSuppressions => Some(3),
        MetricFamily::TerminalOutcomes => Some(4),
        _ => None,
    }
}

const fn gauge_index(family: MetricFamily) -> Option<usize> {
    match family {
        MetricFamily::QueueDepth => Some(0),
        MetricFamily::ActiveTurns => Some(1),
        _ => None,
    }
}

const fn histogram_index(family: MetricFamily) -> Option<usize> {
    match family {
        MetricFamily::CancellationLatency => Some(0),
        MetricFamily::ProviderLatency => Some(1),
        MetricFamily::ToolDuration => Some(2),
        _ => None,
    }
}

const fn counter_index_at(index: usize) -> Option<usize> {
    match index {
        0 => Some(0),
        4 => Some(1),
        5 => Some(2),
        8 => Some(3),
        9 => Some(4),
        _ => None,
    }
}

const fn gauge_index_at(index: usize) -> Option<usize> {
    match index {
        1 => Some(0),
        2 => Some(1),
        _ => None,
    }
}

const fn histogram_index_at(index: usize) -> usize {
    match index {
        3 => 0,
        6 => 1,
        7 => 2,
        _ => unreachable!(),
    }
}
