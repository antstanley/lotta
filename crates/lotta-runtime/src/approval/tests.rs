use super::*;
use crate::RuntimeError;
use crate::ports::{ToolInputSchema, ValidatedToolInput};
use lotta_domain::{
    AgentId, BoundedJsonValue, ConversationId, NonEmptyString, RunId, RuntimeScope, Timestamp,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Journal(Mutex<HashMap<(RuntimeScope, String), ApprovalRequest>>);
impl ApprovalJournalPort for Journal {
    fn insert(&self, request: ApprovalRequest) -> Result<ApprovalRequest, RuntimeError> {
        let mut rows = self.0.lock().unwrap();
        let key = (request.scope.clone(), request.request_id.as_str().into());
        if let Some(row) = rows.get(&key) {
            return Ok(row.clone());
        }
        let pending = rows
            .values()
            .filter(|r| r.scope == request.scope && r.state == ApprovalState::Pending)
            .count();
        if pending >= PENDING_APPROVALS_PER_RUNTIME_MAX {
            return Err(RuntimeError::LimitExceeded {
                context: "pending_approvals_per_runtime_max".into(),
            });
        }
        rows.insert(key, request.clone());
        Ok(request)
    }
    fn get(
        &self,
        scope: &RuntimeScope,
        id: &NonEmptyString,
    ) -> Result<Option<ApprovalRequest>, RuntimeError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(scope.clone(), id.as_str().into()))
            .cloned())
    }
    fn compare_and_set(&self, revision: u64, next: ApprovalRequest) -> Result<bool, RuntimeError> {
        let mut rows = self.0.lock().unwrap();
        let key = (next.scope.clone(), next.request_id.as_str().into());
        let Some(row) = rows.get_mut(&key) else {
            return Ok(false);
        };
        if row.revision != revision {
            return Ok(false);
        }
        *row = next;
        Ok(true)
    }
    fn list(&self, scope: &RuntimeScope) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|r| &r.scope == scope)
            .cloned()
            .collect())
    }
    fn remove(&self, scope: &RuntimeScope, id: &NonEmptyString) -> Result<bool, RuntimeError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .remove(&(scope.clone(), id.as_str().into()))
            .is_some())
    }
}

struct Validator;
impl EditedInputValidator for Validator {
    fn validate(
        &self,
        request: &ApprovalRequest,
        input: BoundedJsonValue,
    ) -> Result<BoundedJsonValue, RuntimeError> {
        let required = request.original_schema.as_value()["required"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(|v| v.as_str());
        if required.is_some_and(|key| input.as_value().get(key).is_none()) {
            return Err(RuntimeError::InvalidData {
                context: "original approval schema".into(),
            });
        }
        Ok(input)
    }
}

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::new(value).unwrap()
}
fn scope(name: &str) -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(format!("agent-{name}")).unwrap(),
        ConversationId::accept(format!("conv-{name}")).unwrap(),
        None,
    )
}
fn timestamp(seconds: i64) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(&format!("2025-01-01T00:00:{seconds:02}Z")).unwrap()
}
fn request(name: &str, owner: RuntimeScope) -> ApprovalRequest {
    ApprovalRequest {
        request_id: text(name),
        tool_call_id: text(format!("call-{name}")),
        scope: owner,
        run_id: RunId::accept("run-1").unwrap(),
        turn_id: text("turn-1"),
        input_id: text("input-1"),
        lease_generation: 7,
        tool_name: text("write"),
        original_input: ValidatedToolInput::new(
            BoundedJsonValue::new(json!({"path":"a"})).unwrap(),
        )
        .unwrap(),
        original_schema: ToolInputSchema::new(
            BoundedJsonValue::new(json!({"type":"object","required":["path"]})).unwrap(),
        )
        .unwrap(),
        created_at: timestamp(1),
        expires_at: timestamp(2),
        state: ApprovalState::Pending,
        revision: 3,
    }
}
fn manager() -> (Arc<Journal>, ApprovalManager) {
    let journal = Arc::new(Journal::default());
    let handle = ApprovalJournal::new(journal.clone());
    (journal, ApprovalManager::new(handle, Arc::new(Validator)))
}
fn input(row: &ApprovalRequest, resolution: ApprovalResolution) -> ApprovalResolutionInput {
    ApprovalResolutionInput {
        scope: row.scope.clone(),
        request_id: row.request_id.clone(),
        tool_call_id: row.tool_call_id.clone(),
        lease_generation: 7,
        revision: 3,
        resolution,
        edited_input: None,
    }
}

pub mod request {
    use super::*;
    #[test]
    fn stored_before_emit() {
        let (_, manager) = manager();
        let row = request("stored", scope("a"));
        let canonical = manager.store_request(row.clone()).unwrap();
        assert_eq!(
            manager
                .get_request(&row.scope, &row.request_id)
                .unwrap()
                .unwrap()
                .revision,
            canonical.revision
        );
    }
    #[test]
    pub(super) fn crash_after_journal_before_ws_replays() {
        let (journal, manager) = manager();
        let row = manager.store_request(request("crash", scope("a"))).unwrap();
        drop(manager);
        let recovery = ApprovalRecovery::new(ApprovalJournal::new(journal));
        assert!(matches!(
            &recovery.reconnect(&row.scope).unwrap()[0],
            RecoveryAction::Replay(r) if r.request_id == row.request_id
        ));
    }
}

pub mod resolution_validation {
    use super::*;
    fn rejects(change: impl FnOnce(&mut ApprovalResolutionInput)) {
        let (_, manager) = manager();
        let row = manager
            .store_request(request("validation", scope("a")))
            .unwrap();
        let _waiter = manager.register_waiter(&row).unwrap();
        let mut value = input(&row, ApprovalResolution::Allow);
        change(&mut value);
        assert!(manager.resolve(&value, 7).is_err());
        assert_eq!(
            manager
                .get_request(&row.scope, &row.request_id)
                .unwrap()
                .unwrap()
                .revision,
            3
        );
    }
    #[test]
    fn wrong_request_rejected_without_mutation() {
        rejects(|v| v.request_id = text("wrong"));
    }
    #[test]
    fn wrong_tool_call_rejected_without_mutation() {
        rejects(|v| v.tool_call_id = text("wrong"));
    }
    #[test]
    fn wrong_runtime_scope_rejected_without_mutation() {
        rejects(|v| v.scope = scope("other"));
    }
    #[test]
    fn wrong_lease_generation_rejected_without_mutation() {
        rejects(|v| v.lease_generation = 8);
    }
    #[test]
    fn wrong_expected_revision_rejected_without_mutation() {
        rejects(|v| v.revision = 4);
    }
    #[test]
    fn edited_input_validated_against_original_schema() {
        rejects(|v| v.edited_input = Some(BoundedJsonValue::new(json!({"other":true})).unwrap()));
    }
}

pub mod resolutions {
    use super::*;
    #[test]
    fn allow_executes_once_including_concurrent_duplicate() {
        for _ in 0..100 {
            let (_, manager) = manager();
            let row = manager.store_request(request("allow", scope("a"))).unwrap();
            let first = manager.resolve(&input(&row, ApprovalResolution::Allow), 7);
            let duplicate = manager.resolve(&input(&row, ApprovalResolution::Allow), 7);
            assert!(first.is_ok() && duplicate.is_err());
        }
    }
    #[test]
    fn deny_appends_and_resumes() {
        let (_, manager) = manager();
        let row = manager.store_request(request("deny", scope("a"))).unwrap();
        let waiter = manager.register_waiter(&row).unwrap();
        assert_eq!(
            manager
                .resolve(&input(&row, ApprovalResolution::Deny), 7)
                .unwrap()
                .request
                .state,
            ApprovalState::Denied
        );
        assert_eq!(
            waiter.blocking_recv().unwrap().resolution,
            ApprovalResolution::Deny
        );
    }
    #[test]
    fn abort_moves_to_cancelling() {
        let (_, manager) = manager();
        let row = manager.store_request(request("abort", scope("a"))).unwrap();
        assert_eq!(
            manager
                .resolve(&input(&row, ApprovalResolution::Abort), 7)
                .unwrap()
                .request
                .state,
            ApprovalState::Aborted
        );
    }
    #[test]
    fn late_response_cannot_clear_cancellation() {
        let (_, manager) = manager();
        let row = manager.store_request(request("late", scope("a"))).unwrap();
        assert!(manager.abort(&row).unwrap());
        assert!(
            manager
                .resolve(&input(&row, ApprovalResolution::Allow), 7)
                .is_err()
        );
    }
    #[test]
    fn crash_executing_is_uncertain_and_never_reruns() {
        let (journal, manager) = manager();
        let row = manager
            .store_request(request("uncertain", scope("a")))
            .unwrap();
        manager
            .resolve(&input(&row, ApprovalResolution::Allow), 7)
            .unwrap();
        let actions = ApprovalRecovery::new(ApprovalJournal::new(journal))
            .restart(&row.scope)
            .unwrap();
        assert!(matches!(
            &actions[0],
            RecoveryAction::Interrupted { interrupted: r, .. }
                if r.state == ApprovalState::Interrupted
        ));
    }

    #[test]
    fn waiter_registration_and_resolution_have_no_lost_wakeup() {
        for index in 0..100 {
            let (_, manager) = manager();
            let manager = Arc::new(manager);
            let row = manager
                .store_request(request(&format!("race-{index}"), scope("race")))
                .unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let registration = {
                let manager = Arc::clone(&manager);
                let row = row.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    manager.register_waiter(&row)
                })
            };
            let resolution = {
                let manager = Arc::clone(&manager);
                let row = row.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    manager.resolve(&input(&row, ApprovalResolution::Allow), 7)
                })
            };
            let registered = registration.join().unwrap();
            let resolved = resolution.join().unwrap();
            assert!(resolved.is_ok());
            match registered {
                Ok(receiver) => assert_eq!(
                    receiver.blocking_recv().unwrap().resolution,
                    ApprovalResolution::Allow
                ),
                Err(RuntimeError::Conflict { .. }) => {}
                Err(error) => panic!("unexpected registration error: {error}"),
            }
        }
    }
}

pub mod recovery {
    use super::*;
    #[test]
    fn reconnect_replays_unresolved_exact_order_and_payload() {
        let (journal, manager) = manager();
        let owner = scope("a");
        manager.store_request(request("b", owner.clone())).unwrap();
        let mut a = request("a", owner.clone());
        a.created_at = timestamp(0);
        manager.store_request(a.clone()).unwrap();
        let actions = ApprovalRecovery::new(ApprovalJournal::new(journal))
            .reconnect(&owner)
            .unwrap();
        assert_eq!(actions.len(), 2);
    }
    #[test]
    fn restart_does_not_assume_execution() {
        let (journal, manager) = manager();
        let row = manager
            .store_request(request("restart", scope("a")))
            .unwrap();
        let actions = ApprovalRecovery::new(ApprovalJournal::new(journal))
            .restart(&row.scope)
            .unwrap();
        assert!(matches!(
            &actions[0],
            RecoveryAction::Interrupted { interrupted: r, .. }
                if r.state == ApprovalState::Interrupted
        ));
    }
    #[test]
    fn stored_before_emit_crash_replays() {
        super::request::crash_after_journal_before_ws_replays();
    }
    #[test]
    fn resolved_requests_are_excluded() {
        let (journal, manager) = manager();
        let row = manager
            .store_request(request("resolved", scope("a")))
            .unwrap();
        manager
            .resolve(&input(&row, ApprovalResolution::Deny), 7)
            .unwrap();
        assert!(
            ApprovalRecovery::new(ApprovalJournal::new(journal))
                .reconnect(&row.scope)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn two_scopes_do_not_cross_replay() {
        let (journal, manager) = manager();
        let a = manager.store_request(request("a", scope("a"))).unwrap();
        manager.store_request(request("b", scope("b"))).unwrap();
        assert_eq!(
            ApprovalRecovery::new(ApprovalJournal::new(journal))
                .reconnect(&a.scope)
                .unwrap()
                .len(),
            1
        );
    }
}

pub mod timeout_interrupts {
    use super::*;
    #[test]
    fn exactly_below_wait_bound_remains_pending() {
        assert_eq!(APPROVAL_WAIT_MS_MAX - 1, 86_399_999);
    }
    #[test]
    fn exactly_at_wait_bound_persists_expired_and_replays() {
        let (journal, manager) = manager();
        let row = manager.store_request(request("at", scope("a"))).unwrap();
        let expired = ApprovalRecovery::new(ApprovalJournal::new(journal.clone()))
            .expire(&row)
            .unwrap();
        assert_eq!(expired.state, ApprovalState::Expired);
        assert_eq!(
            ApprovalRecovery::new(ApprovalJournal::new(journal))
                .reconnect(&row.scope)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn above_wait_bound_restart_is_interrupted_not_denied() {
        let (journal, manager) = manager();
        let row = manager.store_request(request("above", scope("a"))).unwrap();
        let actions = ApprovalRecovery::new(ApprovalJournal::new(journal))
            .restart(&row.scope)
            .unwrap();
        assert!(matches!(
            &actions[0],
            RecoveryAction::Interrupted { interrupted: r, .. }
                if r.state == ApprovalState::Interrupted
        ));
    }
}

pub mod bounds {
    use super::*;
    fn fill(count: usize) -> (ApprovalManager, RuntimeScope) {
        let (_, manager) = manager();
        let owner = scope("bound");
        for n in 0..count {
            manager
                .store_request(request(&format!("r{n}"), owner.clone()))
                .unwrap();
        }
        (manager, owner)
    }
    #[test]
    fn below_bound_127_is_accepted() {
        assert_eq!(
            fill(127).0.list_requests(&scope("bound")).unwrap().len(),
            127
        );
    }
    #[test]
    fn at_bound_128_is_accepted() {
        assert_eq!(
            fill(128).0.list_requests(&scope("bound")).unwrap().len(),
            128
        );
    }
    #[test]
    fn above_bound_129_fails_safely_without_emit() {
        let (manager, owner) = fill(128);
        let error = manager
            .store_request(request("r128", owner.clone()))
            .unwrap_err();
        assert!(matches!(error, RuntimeError::LimitExceeded { .. }));
        assert_eq!(manager.list_requests(&owner).unwrap().len(), 128);
    }
    #[test]
    fn final_resolution_frees_capacity() {
        let (manager, owner) = fill(128);
        let first = manager.list_requests(&owner).unwrap().remove(0);
        manager
            .resolve(&input(&first, ApprovalResolution::Deny), 7)
            .unwrap();
        manager
            .store_request(request("replacement", owner.clone()))
            .unwrap();
        assert_eq!(
            manager
                .list_requests(&owner)
                .unwrap()
                .iter()
                .filter(|r| r.state == ApprovalState::Pending)
                .count(),
            128
        );
    }
}
