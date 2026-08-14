# Done Certificate — Task 18: Input admission chain and the bounded conversation queue

**Task:** [18-input_admission_and_queue.md](18-input_admission_and_queue.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 18. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 18) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a serialized admission chain and per-runtime FIFO queue with baseline coalescing at the soft tier, visible rejection at the hard tier, and a snapshot on every mutation.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not replace the 100/300 tiers with a warn/reject pair: `01-domain-model.md` §Assumptions *Queue compatibility* requires preserving the baseline soft-coalescing and hard-rejection semantics.

## Obligations

- **O1 — The soft tier replaces the oldest coalescable item rather than warning, and barrier items pass the soft level unreplaced**
  - *Claim:* At 100 items, admitting a coalescable item evicts the oldest coalescable one and keeps length at 100; admitting a barrier item takes the length to 101.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(queue::soft_tier)'` — expect `coalescable_replaces_oldest_coalescable` and `barrier_passes_soft_level` to pass. Compare against `../letta-code/src/websocket/listener/queue-no-coalesce.test.ts` for the barrier case.
  - *Checks:* Resolve the coalescing predicate — confirm it keys on the item's coalescability classification, not on the originating client or connection. Coalescing by client would drop barrier ordering, which §Input and queue flow requires preserving.
  - *Status:* ☐ unverified

- **O2 — The hard tier rejects every admission with the `buffer_limit` reason, and internal drops record `stale_generation` where applicable**
  - *Claim:* At 300 items every admission is rejected and the drop reason serializes as `buffer_limit`; a drop caused by a superseded generation serializes as `stale_generation`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(queue::hard_tier)'` — expect `rejects_at_hard_max` and `records_stale_generation` to pass. Compare both literals against `../letta-code/src/types/protocol.ts:492`.
  - *Status:* ☐ unverified

- **O3 — Wire removal dispositions are exactly `dequeued` and `cancelled`, and a queue snapshot is emitted on every mutation**
  - *Claim:* Remove emits `dequeued`, cancel emits `cancelled`, and each admission, replacement, removal, and drop produces one snapshot.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(queue::wire_transitions)'` — expect PASS. Trace one sequence: admit → snapshot 1; admit → snapshot 2; cancel → `cancelled` transition + snapshot 3; assert three snapshots and one `cancelled` disposition.
  - *Status:* ☐ unverified

- **O4 — A duplicate `client_message_id` returns the prior disposition without executing twice, and every out-of-band source enters through the queue**
  - *Claim:* Re-submitting a `client_message_id` yields the first disposition and no second admission; task notifications, cron prompts, approval results, overlay actions, and mod continuations all enqueue rather than mutating a turn directly.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(admission::duplicate) + test(admission::sources)'` — expect the duplicate case plus one case per source named in §Input and queue flow (five sources).
  - *Status:* ☐ unverified

- **O5 — The pump consumes only an `Idle` snapshot and yields after `QUEUE_PUMP_BATCH_MAX` items**
  - *Claim:* The pump refuses to run against a non-`Idle` state and yields between batches of 64.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(queue::pump)'` — expect `pump_requires_idle` and `yields_at_batch_max` to pass, the latter asserting the bound is the named `QUEUE_PUMP_BATCH_MAX` constant from `03-runtime-and-turns.md` §Runtime bounds.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(queue::) + test(admission::)'` and sees soft-tier replacement, barrier pass-through, hard-tier `buffer_limit`, wire dispositions, duplicate replay, and bounded pumping pass**
  - *Claim:* The queue and admission modules pass with soft/hard tier and disposition coverage.
  - *Evidence to collect:* Run the filter and confirm zero failures and both soft-tier cases in the summary.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/lifecycle.rs` (Task 17) supplies the `Idle` snapshot the pump reads; confirm `lifecycle::projections_derive_from_state` still passes now that the queue consumes them : ☐ (PRESERVED / REGRESSION)

## Residue

Queue snapshots are emitted as runtime events here; their wire framing is Task 20.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
