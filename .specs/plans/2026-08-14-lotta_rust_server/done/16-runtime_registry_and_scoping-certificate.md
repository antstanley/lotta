# Done Certificate — Task 16: Runtime registry, scoping, and immediate quiescent eviction

**Task:** [16-runtime_registry_and_scoping.md](16-runtime_registry_and_scoping.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-15 — loop 2 after remediation

## Definition

DONE(Task 16) ≡ every obligation O1…O6 holds with named evidence and the downstream regression check is preserved.

## Obligations

- **O1 — Exact pair keying, idempotent creation, and defensive `RUNTIMES_MAX` enforcement**
  - **Status:** ☒ SATISFIED
  - **Evidence:** `cargo nextest list -p lotta-runtime -E 'test(registry::keying)'` selected seven named tests. The exact run passed 7/7: `idempotent_create`, `default_conversation_is_agent_scoped`, `acting_user_not_identity`, `rejects_at_runtimes_max`, `existing_key_allowed_at_limit`, `rejects_corrupted_over_capacity_without_harming_existing`, and `generation_exhaustion_does_not_insert`.
  - Source uses the exact `(AgentId, ConversationId)` `RuntimeKey`; acting user is excluded. The capacity guard is `entries.len() >= RUNTIMES_MAX.value` and names `RUNTIMES_MAX` in the error. Tests populate 4,096 real entries, reject the 4,097th, permit an existing key at the limit, preserve an existing key with a deliberately corrupted 4,097-entry map, and prove generation exhaustion is atomic. Stale-generation mutation is rejected.

- **O2 — Immediate quiescent eviction by exactly five residency terms, with no runtime idle timer**
  - **Status:** ☒ SATISFIED
  - **Evidence:** The exact selector `test(registry::eviction)` listed and passed 8/8, including `evicts_immediately_when_quiescent`, exactly five `stays_resident_while_*` cases (lifecycle, queue, approval, interrupted result, sandbox subscription), stale-handle safety, and `no_runtime_idle_timer`.
  - `set_residency` takes a complete snapshot and synchronously removes a quiescent entry. Registry source has no idle-eviction timer. New entries temporarily use `TurnStateKind::Command` so startup cannot disappear before its first state publication; this is documented and has no current production caller, hence no present leak path.

- **O3 — Owned, automatic 30-minute worktree-watcher stop independent of registry residency**
  - **Status:** ☒ SATISFIED
  - **Evidence:** The exact selector `test(worktree_watcher::idle_stop)` listed and passed 6/6 using an injected `Arc<Clock>` and paused Tokio time: exact 1,800,000 ms threshold, refreshed deadline, restart after stop, clock-regression/remaining-sleep handling, explicit shutdown, and registry independence.
  - `WorktreeWatcher` owns shared state, a `CancellationToken`, and `JoinHandle`; the timer schedules itself with Tokio and requires no polling. The audited spawn captures only clock/state/cancellation. `shutdown` cancels, joins, and marks inactive; `Drop` aborts. `record_activity` cancels and aborts the prior timer before replacement. Not awaiting an already-aborted replacement during this synchronous method is acceptable: the old task cannot mutate after abort, its handle is dropped only after abort, and explicit shutdown remains the join path for the currently owned task. Marking the controller inactive is the Task 16 core stop policy; filesystem adapter teardown belongs to its later task.

- **O4 — Explicit task-local ambient snapshots and scope-free background behavior**
  - **Status:** ☒ SATISFIED
  - **Evidence:** The exact selector `test(registry::ambient_is_task_local)` listed and passed 7/7: all fields round-trip, optional sandbox absence, explicit snapshot/token/lease propagation, task isolation, raw spawn non-inheritance, outside-scope absence, and redacted `Debug`.
  - The snapshot carries connection ID, device ID, runtime scope, cwd, optional workspace sandbox, permission mode, immutable prevalidated selected skills, bounded tool context, cancellation token, and nonzero lease generation. Debug excludes cwd, sandbox roots, skill names, and tool payload. Scope-free spawn cannot obtain ambient context or registry state.
  - The recursive Syn/WalkDir scanner covers all production `.rs` files while excluding exactly the `#[cfg(test)]` module files. It structurally checks enclosing functions, requires exactly two production spawn calls (`spawn_scoped`, `spawn_idle_stop`), rejects appended/helper-token mutations, nested forbidden statics including generic interiors, thread-local state, and raw spawn variants, and skips test-only items. Superseded legacy test files are absent; no dead residue remains. No process-global registry/context exists.

- **O5 — Repository definition of done and hard source constraints**
  - **Status:** ☒ SATISFIED
  - **Evidence:** `cargo fmt --all --check`, workspace Clippy with `-D warnings`, rustdoc with `RUSTDOCFLAGS='-D warnings'`, `cargo deny check`, and `cargo nextest run --workspace --all-features` all exited 0. Workspace nextest passed 534/534 (one pre-existing nextest leak annotation, no failure).
  - Full runtime passed 91/91 in the initial run and two consecutive repeat runs. Task 09/testkit contract passed 10/10. Runtime source files are below 1,000 lines (maximum 822), no line exceeds 100 columns, and reviewed Task 16 production functions are within 70 lines. No production panic/unwrap/expect/unsafe or lint suppression was introduced. The crate retains one `RuntimeError`; only direct-path dev dependencies `syn` and `walkdir` were added.

- **O6 — Aggregate review selector**
  - **Status:** ☒ SATISFIED
  - **Evidence:** `cargo nextest list -p lotta-runtime -E 'test(registry::) + test(worktree_watcher::)'` selected 30 named tests. The exact run passed 30/30, including idempotent keying, defensive capacity cases, immediate eviction, all five residency terms, ambient isolation/redaction, structural scanner mutations, and automatic watcher stop.

## Additional review findings

- Public APIs expose typed state and redacted diagnostics; no accidental cwd/root/skill/tool payload leak was found.
- No Task 17 lifecycle owner or lease behavior was implemented beyond carrying the lease generation required by this task.
- Registry identity is the exact pair and stale handles are generation-safe.
- The watcher timer is separate from runtime residency and has no registry capture.
- Parent remediation for scanner cfg-test/nested-generic handling and shutdown state/join behavior is present and tested.

## Regression check

- **Status:** ☒ PRESERVED
- `cargo nextest run -p lotta-testkit -E 'test(contract::)'` passed 10/10. The registry currently has no store-port production caller, so this remains downstream contract regression evidence rather than a registry-driven integration.

## Residue

Interrupted-result recovery after process restart remains Task 73. Turn lifecycle ownership and leases remain Task 17. Worktree filesystem adapter teardown remains its later adapter task.

## Conclusion

VERDICT: DONE
CONFIDENCE: high
SUMMARY: CORRECT. All O1–O6 obligations are satisfied with nonzero exact selectors and fresh source evidence. Loop 1 blockers are remediated: defensive capacity behavior, automatic owned watcher scheduling and shutdown, optional/redacted ambient context, structural recursive spawn/global scanning, and exact certificate paths all pass. Workspace, lint, format, docs, deny, runtime-repeat, and Task 09 regression gates are green.
