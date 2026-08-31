# Done Certificate — Task 73: Snapshot sync, reconnect, and restart recovery

**Task:** [73-ws_sync_snapshot_recovery.md](73-ws_sync_snapshot_recovery.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-30 — implementation commit `d00e9d103c13`

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
  - *Evidence:* Fresh source audit passed 7/7 at implementation commit `d00e9d103c13`, confirming full authoritative replay, no client event cursor, and no `idempotency_key` replay deduplication. The combined sync/recovery selector passed 11/11.
  - *Status:* **SATISFIED**

- **O2 — State updates are snapshots, not diffs, except queue removals which carry explicit ordered transitions**
  - *Claim:* Every state message replaces prior state, and only queue removals include transitions.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::sync::snapshot_semantics)'` — expect a per-state-type case asserting full replacement plus a queue case asserting ordered transitions.
  - *Evidence:* The sync/recovery selector passed 11/11, including full-replacement snapshot semantics and explicit ordered queue-removal transitions; the independent source audit passed 7/7.
  - *Status:* **SATISFIED**

- **O3 — Reconnect recovers subscriptions and the next event sequence, and unresolved approvals are replayed**
  - *Claim:* After reconnect the client's subscriptions are restored, the sequence continues monotonically, and pending approvals arrive again.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::recovery::reconnect)'` — expect `restores_subscriptions`, `sequence_continues`, and `replays_pending_approvals`.
  - *Evidence:* Fresh reconnect coverage passed 6/6; supervisor recovery passed 2/2 and process-restart recovery passed 2/2. These cases restore subscriptions, continue the per-connection sequence, and replay unresolved approvals through the durable Task 56 path.
  - *Status:* **SATISFIED**

- **O4 — A missing terminal tool event is repaired by the next authoritative loop snapshot**
  - *Claim:* When a tool-end event is lost, the following loop snapshot restores a consistent view.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::recovery::repairs_missing_tool_end')` — expect PASS, matching §Event envelopes and ordering invariant 3.
  - *Evidence:* The dropped-end production-path proof passed: after the terminal tool event was omitted, the next authoritative loop snapshot repaired the client-visible state. This behavior is also covered by the 11/11 sync/recovery selector.
  - *Status:* **SATISFIED**

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Fresh strict workspace check, strict workspace Clippy, `cargo fmt --all --check`, and `cargo deny check` all passed. The Task 73 evidence suites passed at `d00e9d103c13`: sync/recovery 11/11, reconnect 6/6, supervisor 2/2, restart 2/2, approval recovery 8/8, sequence/envelope 9/9, and source audit 7/7.
  - *Status:* **SATISFIED**

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::sync::) + test(ws::recovery::)'` and sees snapshot replay with no event cursor, snapshot semantics, reconnect recovery, and tool-end repair pass**
  - *Claim:* Sync and recovery pass with the no-cursor grep clean.
  - *Evidence to collect:* Run the filter (expect zero failures) and the `last_event_id` grep (expect no match).
  - *Evidence:* The combined sync/recovery selector passed 11/11 and the source audit passed 7/7 with no event cursor. A clean independent GPT-5.6 Sol review of final commit `d00e9d103c13` returned `CORRECT / DONE` and confirmed O1–O6.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/approval/recovery.rs` (Task 56) supplies unresolved approvals; approval recovery passed 8/8 — **PRESERVED**.
- `crates/lotta-app-server/src/ws/envelope.rs` (Task 20) stamps sequences; sequence/envelope coverage passed 9/9 after recovery reused the counter — **PRESERVED**.

## Residue

`03-runtime-and-turns.md` §Assumptions leaves whether in-flight provider streams resume from run IDs open; recovery here restores state, not streams.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-30 at implementation commit `d00e9d103c13`. All O1–O6 are satisfied: authoritative snapshot replay has no event cursor or idempotency-key deduplication; state replacement and ordered queue-removal semantics pass; reconnect, supervisor, and process-restart recovery restore subscriptions, monotonic sequence, and unresolved approvals; and the dropped-end production proof shows the next loop snapshot repairs missing terminal state. Fresh focused evidence passed sync/recovery 11/11, reconnect 6/6, supervisor 2/2, restart 2/2, approval recovery 8/8, sequence/envelope 9/9, and source audit 7/7. Strict workspace check, Clippy, format, and deny gates are green, both downstream regressions are preserved, and a clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
