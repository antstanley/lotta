# Done Certificate — Task 73: Snapshot sync, reconnect, and restart recovery

**Task:** [73-ws_sync_snapshot_recovery.md](73-ws_sync_snapshot_recovery.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 73. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 73) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `sync` replaying authoritative snapshots rather than an event-ID delta, with reconnect subscription and sequence recovery and restart approval recovery.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not implement sync as event-ID replay: `02-app-server-api.md` §Event envelopes and ordering states `idempotency_key` is not stable across replay and state updates are snapshots.

## Obligations

- **O1 — `sync` replays authoritative snapshots and does not attempt a delta from a client-supplied last event ID**
  - *Claim:* The `sync` handler emits full snapshots and ignores any client-side event cursor.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::sync::replays_snapshots)'` — expect PASS asserting the emitted set is the full snapshot set. Grep `crates/lotta-app-server/src/ws/sync.rs` for a `last_event_id` or `since` parameter — expect none, per `02-app-server-api.md` §Core lifecycle (`replay authoritative snapshots`).
  - *Checks:* Resolve any deduplication key used during replay — confirm `idempotency_key` is not consulted; §Event envelopes and ordering states it is not stable across replay.
  - *Status:* ☐ unverified

- **O2 — State updates are snapshots, not diffs, except queue removals which carry explicit ordered transitions**
  - *Claim:* Every state message replaces prior state, and only queue removals include transitions.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::sync::snapshot_semantics)'` — expect a per-state-type case asserting full replacement plus a queue case asserting ordered transitions.
  - *Status:* ☐ unverified

- **O3 — Reconnect recovers subscriptions and the next event sequence, and unresolved approvals are replayed**
  - *Claim:* After reconnect the client's subscriptions are restored, the sequence continues monotonically, and pending approvals arrive again.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::recovery::reconnect)'` — expect `restores_subscriptions`, `sequence_continues`, and `replays_pending_approvals`.
  - *Status:* ☐ unverified

- **O4 — A missing terminal tool event is repaired by the next authoritative loop snapshot**
  - *Claim:* When a tool-end event is lost, the following loop snapshot restores a consistent view.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::recovery::repairs_missing_tool_end')` — expect PASS, matching §Event envelopes and ordering invariant 3.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::sync::) + test(ws::recovery::)'` and sees snapshot replay with no event cursor, snapshot semantics, reconnect recovery, and tool-end repair pass**
  - *Claim:* Sync and recovery pass with the no-cursor grep clean.
  - *Evidence to collect:* Run the filter (expect zero failures) and the `last_event_id` grep (expect no match).
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/approval/recovery.rs` (Task 56) supplies unresolved approvals; confirm `approval::recovery` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-app-server/src/ws/envelope.rs` (Task 20) stamps sequences; confirm `ws::envelope` still passes after recovery reuses the counter : ☐ (PRESERVED / REGRESSION)

## Residue

`03-runtime-and-turns.md` §Assumptions leaves whether in-flight provider streams resume from run IDs open; recovery here restores state, not streams.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
