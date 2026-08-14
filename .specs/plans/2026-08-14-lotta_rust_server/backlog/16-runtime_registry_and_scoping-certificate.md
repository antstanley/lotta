# Done Certificate — Task 16: Runtime registry, scoping, and immediate quiescent eviction

**Task:** [16-runtime_registry_and_scoping.md](16-runtime_registry_and_scoping.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 16. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 16) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a `(AgentId, ConversationId)`-keyed registry with idempotent creation, an explicit residency predicate, and immediate eviction once quiescent.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not keep an idle-but-quiescent runtime resident: `03-runtime-and-turns.md` §Runtime registry evicts immediately, and residency changes observable `update_device_status` and queue behavior.

## Obligations

- **O1 — The registry is keyed by the `(AgentId, ConversationId)` pair, creation is idempotent, and `RUNTIMES_MAX` rejects `runtime_start` at the limit**
  - *Claim:* Two starts for one scope yield one runtime; two scopes differing only in agent are distinct; the 4,097th scope is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(registry::keying)'` — expect `idempotent_create`, `default_conversation_is_agent_scoped`, and `rejects_at_runtimes_max` to pass. Confirm the rejection path names `RUNTIMES_MAX` from Task 05.
  - *Status:* ☐ unverified

- **O2 — A quiescent runtime is evicted immediately, and residency is decided by the five-part predicate rather than an idle timer**
  - *Claim:* A runtime with no lifecycle, queue, approval, interrupted-result, or sandbox-subscription state is removed on the same tick it becomes quiescent, and no idle-eviction constant exists.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(registry::eviction)'` — expect `evicts_immediately_when_quiescent` plus one `stays_resident_while_<state>` case per predicate term (five cases). Grep `crates/lotta-runtime/src/` for `IDLE_EVICT` — expect zero matches.
  - *Checks:* Resolve the 30-minute idle timer's owner — confirm it belongs to the worktree watcher (`../letta-code/src/websocket/listener/worktree-watcher.ts`), not the runtime. `03-runtime-and-turns.md` §Assumptions *Runtime eviction* assigns it explicitly to the watcher.
  - *Status:* ☐ unverified

- **O3 — The worktree watcher has its own 30-minute idle stop, independent of runtime residency**
  - *Claim:* The watcher stops 30 minutes after its last activity even while its runtime remains resident, and stopping it does not evict the runtime.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(worktree_watcher::idle_stop)'` — expect PASS; the test advances the fake clock by 30 minutes and asserts the watcher stopped while the runtime handle is still registered.
  - *Status:* ☐ unverified

- **O4 — Per-turn ambient data is task-local and every spawned task receives an explicit snapshot; no background task can reach a conversation implicitly**
  - *Claim:* Connection/device IDs, scope, cwd, sandbox, permission mode, selected skills, cancellation token, and lease generation are carried in task-local context, and spawned tasks take a snapshot parameter.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/scope_context.rs` and confirm the eight ambient fields listed in §Runtime registry. Grep the crate for `static` mutable globals and for `tokio::spawn` calls that capture the registry directly — expect zero. Run `cargo nextest run -p lotta-runtime -E 'test(registry::ambient_is_task_local)'` — expect PASS.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(registry::) + test(worktree_watcher::)'` and sees idempotent keying, immediate quiescent eviction, the five residency terms, and the watcher's separate idle stop pass**
  - *Claim:* The registry and watcher modules pass with the residency cases enumerated.
  - *Evidence to collect:* Run the filter and confirm zero failures and five `stays_resident_while_*` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-testkit/src/fakes/` store fakes back the registry's lookups; confirm Task 09's `contract::` suite still passes when driven through the registry : ☐ (PRESERVED / REGRESSION)

## Residue

Recovery of interrupted results after process restart is Task 73; this task only makes an interrupted result a residency term.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
