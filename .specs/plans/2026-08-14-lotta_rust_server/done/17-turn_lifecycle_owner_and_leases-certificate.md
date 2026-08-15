# Done Certificate — Task 17: Turn lifecycle owner and leases

**Task:** [17-turn_lifecycle_owner_and_leases.md](17-turn_lifecycle_owner_and_leases.md) · **Plan:** [plan.md](../plan.md)
**Validated:** 2026-08-15 · adversarial review loop 1

## Definition

DONE(Task 17) ≡ every obligation O1…O6 holds with named evidence and the downstream
Task 16 regression check is preserved.

## Obligations

- **O1 — Sole state owner and strictly increasing opaque leases: SATISFIED**
  - `lotta-domain::runtime::turn_state` keeps `TurnState` private; only `TurnLifecycle` writes it.
    `RuntimeEntry` stores exactly one `LifecycleOwner`, which wraps exactly one `TurnLifecycle`.
    Both `Idle -> Command` and `Idle -> Active` call checked `next_lease`; generation starts at
    zero and minted leases therefore begin at one and strictly increase.
  - `TurnLease` construction and owner identity remain private. Its public `Debug` and read-only
    `generation()` accessor reveal identity metadata only indirectly and do not make a token
    forgeable; equality checks the complete private owner UUID plus generation.
  - `get_or_create` documents that the injected UUID must come from `ports::IdGenerator` and be
    unique among live owners. Existing-key lookup occurs before allocation and intentionally ignores
    the supplied new UUID. This is practical for deterministic composition, but correctness relies on
    callers honoring the uniqueness precondition; duplicate injected UUIDs across distinct scopes
    would permit same-generation cross-owner token equality. The required generator contract provides
    uniqueness, and current callers use distinct deterministic UUIDs.
  - Exact selector listed one test and passed 1/1:
    `test(lifecycle::lease_generation_increases)`.

- **O2 — Four live post-await checks and stale suppression: SATISFIED**
  - `LeaseGuard::check` evaluates, in order, live listener activity, exact current runtime handle,
    the current registry owner's complete lease, and the captured shared cancellation token under its
    policy. It does not compare against a captured generation snapshot.
  - `apply_after_await` invokes its callback only after all checks pass. The semantic fake counters
    prove stale persistence, tool, protocol, and channel effects all remain zero.
  - `finish_turn_after_await` performs check and mutable release synchronously with no await between
    them; stale guards cannot release a replacement. `LifecycleOwner::finish_turn` is crate-visible,
    so external callers cannot bypass the guard for asynchronous turn completion.
  - `finish_command` is intentionally documented as synchronous and prechecks the complete current
    lease. No production command loop or awaited completion path exists yet. A future asynchronous
    command caller must use a guarded command-finish API rather than this synchronous method; this is
    not a current bypass because all current finish-command calls are synchronous tests.
  - The replacement trace genuinely yields before settlement/replacement, then resumes the stale
    guard. Independent listener-inactive, runtime-missing, stale-lease, and cancellation-policy cases
    are covered; the cancellation case is separately exercised in the four required stale selector
    cases.
  - Exact selector listed four tests and passed 4/4:
    `test(lifecycle::stale_lease_suppression)`.

- **O3 — Impossible states are diagnosed, not repaired: SATISFIED**
  - Runtime begin operations route impossible transitions to one intentional centralized invariant
    panic after emitting a structured trace. The panic names agent scope, conversation scope, `from`,
    and `to`; no repair transition is performed.
  - The exact test catches the panic, checks the complete diagnostic text, and verifies state remains
    `Command`. Stale late leases remain typed `StaleTurnLease` values and preserve state in domain
    tests; they do not enter the invariant panic path.
  - Domain `request_cancellation` retains a pre-existing logically unreachable post-check branch. It
    is not reachable from stale input because state and lease are checked before replacement, though
    its diagnostic is less rich than the runtime invariant helper.
  - Exact selector listed one test and passed 1/1:
    `test(lifecycle::impossible_state_is_invariant_violation)`.

- **O4 — Projections are derived and non-contradictory: SATISFIED**
  - `LifecycleProjection` is an ephemeral borrowed value assembled under immutable access from
    `TurnLifecycle`/`TurnStateView`; it cannot mutate or persist contradictory state.
    `RuntimeEntry` stores no lifecycle projection duplicate, and `RuntimeResidency` stores only four
    auxiliary terms.
  - Semantics are covered for Idle/Command/Active/Cancelling through state, `is_processing`, loop
    status, active run IDs, and last settled stop reason. The structural test rejects duplicate owner
    and entry projection fields.
  - Exact selector listed five tests and passed 5/5:
    `test(lifecycle::projections_derive_from_state)`.

- **O5 — Repository definition of done: SATISFIED**
  - `cargo fmt --all --check`: PASS.
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
  - `cargo nextest run --workspace --all-features`: PASS, 547/547.
  - `cargo deny check`: PASS (`advisories`, `bans`, `licenses`, `sources`).
  - Touched production files are below 1,000 lines and at or below 100 columns; reviewed added/core
    functions are below 70 lines. No task constant was introduced. The 113-line `lease.rs` file is
    within the file limit; the previously noted changed-lines concern does not violate the repository
    limits.
  - Production hygiene review found only the intentional centralized lifecycle panic newly added;
    no new production unwrap/expect/unsafe/lint suppression was introduced. The touched domain file's
    `unreachable!` predates Task 17. `tracing` is a direct normal dependency used by the invariant
    diagnostic and adds no redundant abstraction.

- **O6 — Reviewable lifecycle suite: SATISFIED**
  - Exact selector listed 13 nonzero tests and passed 13/13:
    `test(lifecycle::)`. The set includes lease minting, four stale-suppression cases, four independent
    live-check evidence, invariant diagnostics, and all projection cases.

## Regression check

**PRESERVED.** Task 16 registry integration remains coherent:

- `RuntimeResidency` stores only queue, approval, interrupted-result, and sandbox-subscription terms;
  `set_residency` obtains live owner state as the fifth term, so callers cannot publish a contradictory
  lifecycle snapshot.
- Quiescent Idle owners are evicted immediately; an active owner retains residency. One owner exists
  per exact `(AgentId, ConversationId)` registry entry.
- `test(registry::eviction)` passed 8/8, including live lifecycle retention, immediate eviction, and
  stale-handle rejection.
- Task 16's structural spawn scanner passed and still reports exactly the two audited production spawn
  sites. Task 04 domain runtime passed 18/18. Task 09 port contracts passed 10/10.

## Residue and decisions

- There is not yet a production persistence/provider/tool/channel effect path; Task 17 supplies the
  guard API and semantic mutation tests that future paths must use. The source-string test is only
  supplemental to behavior tests and structural state checks.
- Cancellation terminal semantics remain Task 57. This task correctly permits policy-selected cleanup
  only while listener, exact runtime, and lease remain live.
- Minimal future hardening: when asynchronous command execution is introduced, add
  `finish_command_after_await` (or reduce synchronous `finish_command` visibility) so awaited command
  completion cannot accidentally skip listener/cancellation checks.

## Conclusion

VERDICT: CORRECT
CONFIDENCE: high
SUMMARY: O1–O6 are satisfied with nonzero exact selectors, all repository gates pass, and Task 16,
Task 04, and Task 09 regressions are preserved.

DONE
