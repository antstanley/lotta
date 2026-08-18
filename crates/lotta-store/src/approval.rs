//! Atomic bounded local approval journal.

use lotta_domain::{NonEmptyString, RuntimeScope};
use lotta_runtime::RuntimeError;
use lotta_runtime::approval::{
    APPROVAL_TERMINAL_REPLAY_MAX, ApprovalJournalPort, ApprovalRequest, ApprovalState,
    PENDING_APPROVALS_PER_RUNTIME_MAX,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;

const APPROVAL_JOURNAL_BYTES_MAX: usize = 8 * 1_024 * 1_024;

#[derive(Default, Deserialize, Serialize)]
struct JournalFile {
    requests: Vec<ApprovalRequest>,
}

impl crate::LocalStore {
    /// Returns a runtime approval journal backed by this store.
    #[must_use]
    pub fn approval_journal(&self) -> lotta_runtime::ApprovalJournal {
        lotta_runtime::ApprovalJournal::new(std::sync::Arc::new(self.clone()))
    }

    fn approval_path(&self) -> PathBuf {
        self.paths().runtime().join("approvals.json")
    }

    fn read_approvals(&self) -> Result<JournalFile, RuntimeError> {
        let path = self.approval_path();
        crate::confinement::validate_existing(self.paths().root(), &path)
            .map_err(|_| invalid("approval journal path"))?;
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(JournalFile::default());
            }
            Err(_) => return Err(adapter("approval journal metadata")),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid("approval journal file"));
        }
        #[cfg(unix)]
        reject_permissive_mode(&metadata)?;
        if metadata.len() > APPROVAL_JOURNAL_BYTES_MAX as u64 {
            return Err(limit("approval_journal_bytes_max"));
        }
        let file = std::fs::File::open(&path).map_err(|_| adapter("approval journal open"))?;
        let capacity =
            usize::try_from(metadata.len()).map_err(|_| limit("approval_journal_bytes_max"))?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take((APPROVAL_JOURNAL_BYTES_MAX + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| adapter("approval journal read"))?;
        if bytes.len() > APPROVAL_JOURNAL_BYTES_MAX {
            return Err(limit("approval_journal_bytes_max"));
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid("approval journal"))
    }

    fn write_approvals(
        &self,
        file: &mut JournalFile,
        lock: &crate::LottaStorageLock,
    ) -> Result<(), RuntimeError> {
        compact(file);
        let bytes = serde_json::to_vec(file).map_err(|_| invalid("approval journal encoding"))?;
        if bytes.len() > APPROVAL_JOURNAL_BYTES_MAX {
            return Err(limit("approval_journal_bytes_max"));
        }
        crate::atomic::atomic_write_locked(
            &self.approval_path(),
            &bytes,
            crate::WriteMode::ProviderAuth,
            lock,
        )
        .map_err(|_| adapter("approval journal write"))
    }

    fn transaction(&self) -> Result<crate::LottaStorageLock, RuntimeError> {
        crate::LottaStorageLock::try_acquire(self.paths().root())
            .map_err(|_| adapter("approval journal lock"))
    }
}

impl ApprovalJournalPort for crate::LocalStore {
    fn insert(&self, request: ApprovalRequest) -> Result<ApprovalRequest, RuntimeError> {
        let lock = self.transaction()?;
        let mut file = self.read_approvals()?;
        if let Some(existing) = exact(&file.requests, &request.scope, &request.request_id) {
            return same_request(existing, &request);
        }
        let pending = file
            .requests
            .iter()
            .filter(|value| {
                value.scope == request.scope
                    && matches!(
                        value.state,
                        ApprovalState::Pending | ApprovalState::Executing
                    )
            })
            .count();
        if pending >= PENDING_APPROVALS_PER_RUNTIME_MAX {
            return Err(limit("pending_approvals_per_runtime_max"));
        }
        file.requests
            .try_reserve(1)
            .map_err(|_| limit("approval journal records"))?;
        file.requests.push(request.clone());
        self.write_approvals(&mut file, &lock)?;
        Ok(request)
    }

    fn get(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<Option<ApprovalRequest>, RuntimeError> {
        self.read_approvals()
            .map(|file| exact(&file.requests, scope, request_id).cloned())
    }

    fn compare_and_set(
        &self,
        expected_revision: u64,
        next: ApprovalRequest,
    ) -> Result<bool, RuntimeError> {
        let lock = self.transaction()?;
        let mut file = self.read_approvals()?;
        let Some(current) = file
            .requests
            .iter_mut()
            .find(|value| value.scope == next.scope && value.request_id == next.request_id)
        else {
            return Ok(false);
        };
        if current.revision != expected_revision {
            return Ok(false);
        }
        *current = next;
        self.write_approvals(&mut file, &lock)?;
        Ok(true)
    }

    fn list(&self, scope: &RuntimeScope) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        self.read_approvals().map(|file| {
            file.requests
                .into_iter()
                .filter(|value| &value.scope == scope)
                .collect()
        })
    }

    fn remove(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<bool, RuntimeError> {
        let lock = self.transaction()?;
        let mut file = self.read_approvals()?;
        let before = file.requests.len();
        file.requests
            .retain(|request| &request.scope != scope || &request.request_id != request_id);
        if before == file.requests.len() {
            return Ok(false);
        }
        self.write_approvals(&mut file, &lock)?;
        Ok(true)
    }
}

fn compact(file: &mut JournalFile) {
    file.requests.retain(|request| {
        !matches!(
            request.state,
            ApprovalState::Allowed | ApprovalState::Denied | ApprovalState::Aborted
        )
    });
    let mut by_scope = std::collections::HashMap::<RuntimeScope, Vec<_>>::new();
    for (index, request) in file.requests.iter().enumerate().filter(|(_, request)| {
        matches!(
            request.state,
            ApprovalState::Expired | ApprovalState::Interrupted
        )
    }) {
        by_scope.entry(request.scope.clone()).or_default().push((
            index,
            request.created_at,
            request.request_id.as_str().to_owned(),
        ));
    }
    let mut discarded = Vec::new();
    for mut terminal in by_scope.into_values() {
        terminal.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.2.cmp(&right.2)));
        let remove = terminal.len().saturating_sub(APPROVAL_TERMINAL_REPLAY_MAX);
        discarded.extend(terminal.into_iter().take(remove).map(|(index, _, _)| index));
    }
    discarded.sort_unstable();
    let mut index = 0usize;
    file.requests.retain(|_| {
        let keep = discarded.binary_search(&index).is_err();
        index += 1;
        keep
    });
}

fn exact<'a>(
    requests: &'a [ApprovalRequest],
    scope: &RuntimeScope,
    request_id: &NonEmptyString,
) -> Option<&'a ApprovalRequest> {
    requests
        .iter()
        .find(|value| &value.scope == scope && &value.request_id == request_id)
}

fn same_request(
    existing: &ApprovalRequest,
    request: &ApprovalRequest,
) -> Result<ApprovalRequest, RuntimeError> {
    if existing.tool_call_id == request.tool_call_id
        && existing.lease_generation == request.lease_generation
        && existing.revision == request.revision
    {
        Ok(existing.clone())
    } else {
        Err(RuntimeError::Conflict {
            context: "approval request collision".into(),
        })
    }
}

#[cfg(unix)]
fn reject_permissive_mode(metadata: &std::fs::Metadata) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(invalid("approval journal mode"));
    }
    Ok(())
}

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn limit(context: &'static str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: context.into(),
    }
}
fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "approval_journal",
        context: context.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, RunId, Timestamp};
    use lotta_runtime::approval::{ApprovalManager, EditedInputValidator};
    use lotta_runtime::ports::{ToolInputSchema, ValidatedToolInput};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

    struct Validator;
    impl EditedInputValidator for Validator {
        fn validate(
            &self,
            _: &ApprovalRequest,
            input: BoundedJsonValue,
        ) -> Result<BoundedJsonValue, RuntimeError> {
            Ok(input)
        }
    }

    fn scope(name: &str) -> RuntimeScope {
        RuntimeScope::new(
            AgentId::accept(format!("agent-{name}")).unwrap(),
            ConversationId::accept(format!("conv-{name}")).unwrap(),
            None,
        )
    }

    fn request(name: &str, scope: RuntimeScope) -> ApprovalRequest {
        let text = |value: String| NonEmptyString::new(value).unwrap();
        ApprovalRequest {
            request_id: text(name.to_owned()),
            tool_call_id: text(format!("call-{name}")),
            scope,
            run_id: RunId::accept("run-1").unwrap(),
            turn_id: text("turn-1".into()),
            input_id: text("input-1".into()),
            lease_generation: 7,
            tool_name: text("write".into()),
            original_input: ValidatedToolInput::new(
                BoundedJsonValue::new(json!({"path":"a"})).unwrap(),
            )
            .unwrap(),
            original_schema: ToolInputSchema::new(
                BoundedJsonValue::new(json!({"type":"object"})).unwrap(),
            )
            .unwrap(),
            created_at: Timestamp::parse_persisted_rfc3339("2025-01-01T00:00:01Z").unwrap(),
            expires_at: Timestamp::parse_persisted_rfc3339("2025-01-02T00:00:01Z").unwrap(),
            state: ApprovalState::Pending,
            revision: 0,
        }
    }

    fn manager(name: &str) -> (std::path::PathBuf, Arc<ApprovalManager>) {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("lotta-approval-store-{name}-{id}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = crate::StorePaths::new(root.clone()).unwrap();
        let journal = crate::LocalStore::new(paths).approval_journal();
        (
            root,
            Arc::new(ApprovalManager::new(journal, Arc::new(Validator))),
        )
    }

    #[test]
    fn real_journal_insert_transition_list_and_secure_mode() {
        let (root, manager) = manager("roundtrip");
        let row = request("one", scope("roundtrip"));
        manager.store_request(row.clone()).unwrap();
        assert!(manager.expire(&row).unwrap());
        let rows = manager.list_requests(&row.scope).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, ApprovalState::Expired);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let runtime_mode = std::fs::metadata(root.join("runtime"))
                .unwrap()
                .permissions()
                .mode();
            let file_mode = std::fs::metadata(root.join("runtime/approvals.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(runtime_mode & 0o077, 0);
            assert_eq!(file_mode & 0o077, 0);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_compaction_is_per_scope_and_keeps_latest_128() {
        let mut file = JournalFile::default();
        for owner in [scope("a"), scope("b")] {
            for index in 0..1_000 {
                let mut row = request(&format!("request-{index:04}"), owner.clone());
                row.state = ApprovalState::Expired;
                file.requests.push(row);
            }
        }
        compact(&mut file);
        for owner in [scope("a"), scope("b")] {
            let rows = file
                .requests
                .iter()
                .filter(|row| row.scope == owner)
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), APPROVAL_TERMINAL_REPLAY_MAX);
            assert_eq!(rows.first().unwrap().request_id.as_str(), "request-0872");
            assert_eq!(rows.last().unwrap().request_id.as_str(), "request-0999");
        }
    }

    #[test]
    fn same_manager_serializes_four_hundred_writers() {
        let (root, manager) = manager("concurrent");
        let mut workers = Vec::new();
        for worker in 0..4 {
            let manager = Arc::clone(&manager);
            workers.push(std::thread::spawn(move || {
                let owner = scope(&format!("worker-{worker}"));
                for index in 0..100 {
                    manager
                        .store_request(request(&format!("request-{worker}-{index}"), owner.clone()))
                        .unwrap();
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        for worker in 0..4 {
            assert_eq!(
                manager
                    .list_requests(&scope(&format!("worker-{worker}")))
                    .unwrap()
                    .len(),
                100
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pending_bound_rejects_the_129th_record() {
        let (root, manager) = manager("bound");
        let owner = scope("bound");
        for index in 0..PENDING_APPROVALS_PER_RUNTIME_MAX {
            manager
                .store_request(request(&format!("request-{index}"), owner.clone()))
                .unwrap();
        }
        assert!(matches!(
            manager.store_request(request("overflow", owner)),
            Err(RuntimeError::LimitExceeded { .. })
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_and_non_file_journals_are_rejected() {
        let (root, manager) = manager("invalid-file");
        let path = root.join("runtime/approvals.json");
        std::fs::create_dir_all(&path).unwrap();
        assert!(matches!(
            manager.list_requests(&scope("invalid-file")),
            Err(RuntimeError::InvalidData { .. })
        ));
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::write(&path, vec![b'x'; APPROVAL_JOURNAL_BYTES_MAX + 1]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(matches!(
            manager.list_requests(&scope("invalid-file")),
            Err(RuntimeError::LimitExceeded { .. })
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_permissive_mode_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (root, manager) = manager("security");
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let target = root.join("target.json");
        std::fs::write(&target, b"{\"requests\":[]}").unwrap();
        let path = runtime.join("approvals.json");
        symlink(&target, &path).unwrap();
        assert!(manager.list_requests(&scope("security")).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"{\"requests\":[]}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            manager.list_requests(&scope("security")),
            Err(RuntimeError::InvalidData { .. })
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
