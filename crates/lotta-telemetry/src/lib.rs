//! Structured tracing and metrics adapters.
//!
//! This crate implements telemetry ports. It must not own domain behavior or depend on transport
//! adapters and unrelated concrete adapters.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use lotta_runtime::observe::events::{EventSinkError, RuntimeEvent, RuntimeEventSink};

/// Existing tracing-backed adapter for safe runtime events.
#[derive(Debug, Default)]
pub struct TracingRuntimeEventSink;

impl RuntimeEventSink for TracingRuntimeEventSink {
    fn try_emit(&self, event: RuntimeEvent) -> Result<(), EventSinkError> {
        tracing::info!(runtime_event = ?event, "runtime observation");
        Ok(())
    }
}
