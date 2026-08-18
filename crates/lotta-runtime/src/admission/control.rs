use super::{AdmissionOutcome, AdmissionRequest, AdmissionRoute};
use crate::{RuntimeError, RuntimeHandle};

/// Admits control input against a separately published active owner snapshot.
///
/// This preserves canonical control lease semantics without requiring the registry mutex held by
/// the long-running turn controller.
///
/// # Errors
/// Returns a conflict when the supplied owner or route does not match the active snapshot.
pub fn admit_control_snapshot(
    handle: &RuntimeHandle,
    active_handle: &RuntimeHandle,
    active_lease: &lotta_domain::TurnLease,
    request: AdmissionRequest,
) -> Result<AdmissionOutcome, RuntimeError> {
    if handle != active_handle {
        return Err(conflict("approval runtime handle"));
    }
    let AdmissionRoute::Control(lease) = &request.route else {
        return Err(conflict("approval admission route"));
    };
    if lease != active_lease {
        return Err(conflict("approval lease"));
    }
    Ok(AdmissionOutcome::Control(request.item))
}

fn conflict(context: &'static str) -> RuntimeError {
    RuntimeError::Conflict {
        context: context.into(),
    }
}
