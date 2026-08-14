# Done Certificate — Task 13: Reference trace corpus extraction

**Task:** [13-fixtures_reference_trace_corpus.md](13-fixtures_reference_trace_corpus.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 13. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 13) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a `fixtures/reference-traces/` corpus of baseline command/event traces for the queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery reliability surfaces.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not compare `idempotency_key` across replays: `02-app-server-api.md` §Event envelopes and ordering states it is unique per emission and not stable across replay.

## Obligations

- **O1 — All seven reliability surfaces of `00-overview.md` §Compatibility definition have a recorded trace**
  - *Claim:* `fixtures/reference-traces/` contains a trace for queue, abort, disconnect, stale-lease, retry, idempotency, and crash recovery.
  - *Evidence to collect:* List `fixtures/reference-traces/` and match against the seven surfaces named in the §Compatibility definition Reliability row. Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::index_is_complete)'` — expect seven passing cases.
  - *Status:* ☐ unverified

- **O2 — The six `02-app-server-api.md` §Event envelopes and ordering invariants are executable assertions over a trace**
  - *Claim:* The comparator checks increasing `event_seq`, `input_accepted` before caused events, tool-start before tool-end, exactly-once `turn_finished` after the final delta, no post-stale-lease emission, and stable connection-ordinal delivery.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::ordering_invariants)'` — expect six passing cases, one named per invariant. Read the exactly-once case and confirm it asserts both the count and the position relative to the final `stream_delta`.
  - *Status:* ☐ unverified

- **O3 — Trace comparison matches semantically-equivalent fields by rule, so generated UUIDs, timestamps, and `idempotency_key` values do not cause false failures**
  - *Claim:* Replaying a trace with different UUIDs and timestamps passes, while a changed discriminant or ordering fails.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::semantic_equivalence)'` — expect PASS. Read the rule table and confirm `idempotency_key` is compared for uniqueness only, per `02-app-server-api.md` §Event envelopes and ordering (`not stable across replay`).
  - *Checks:* Resolve the comparator's `idempotency_key` rule — confirm it asserts per-emission uniqueness rather than cross-replay equality; treating it as stable would encode a contradiction of the spec into every downstream conformance task.
  - *Status:* ☐ unverified

- **O4 — A happy-path turn trace exists that Task 21 can replay end to end**
  - *Claim:* A trace named for the vertical slice records `runtime_start`, `runtime_start_response`, `update_device_status`, `update_loop_status`, `update_queue`, `input`, `input_accepted`, `stream_delta`, and `turn_finished` in `02-app-server-api.md` §Core lifecycle order.
  - *Evidence to collect:* Read the slice trace file and confirm the message sequence matches the §Core lifecycle diagram. Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::slice_trace_shape)'` — expect PASS.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::)'` and sees the seven reliability traces, six ordering invariants, semantic-equivalence rules, and the slice trace pass**
  - *Claim:* The trace-fixture module passes with per-surface and per-invariant cases.
  - *Evidence to collect:* Run the filter and confirm zero failures and at least fourteen cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-testkit/src/fixtures/` gains a module alongside Task 11's persistence loader; confirm `fixtures::persistence::index_is_complete` still passes after the shared loader is extended : ☐ (PRESERVED / REGRESSION)

## Residue

Traces are recorded against `letta-code@300f923f`. Channel-gateway traces are captured separately in Task 92 because they need the channel host running.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
