# Done Certificate — Task 20: WebSocket runtime commands and event envelopes

**Task:** [20-ws_runtime_commands_and_event_envelopes.md](20-ws_runtime_commands_and_event_envelopes.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 20. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 20) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the Runtime command group decoded, routed, and answered with correctly enveloped, ordered events on a per-connection sequence.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not stamp connection-specific responses with the runtime envelope, and must not treat `idempotency_key` as replay-stable; `02-app-server-api.md` §Event envelopes and ordering states both explicitly.

## Obligations

- **O1 — The five Runtime-group commands decode, route, and answer, and `runtime_start` rejects mutually exclusive choices before allocation**
  - *Claim:* Each command produces its documented response, and a `runtime_start` naming both an existing and a new agent fails without creating either.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::runtime_group)'` — expect one case per command plus `rejects_mutually_exclusive_before_allocation`, the latter asserting the fake store recorded zero writes.
  - *Status:* ☐ unverified

- **O2 — The seven broadcast frame types carry the runtime envelope, a monotonic per-connection `event_seq`, `emitted_at`, and a per-emission `idempotency_key`; connection-specific responses carry none of these**
  - *Claim:* `control_request`, `update_device_status`, `update_loop_status`, `update_queue`, `stream_delta`, `turn_finished`, and `update_subagent_state` are stamped; `runtime_start_response` and management responses are not.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::envelope)'` — expect seven stamped cases and at least two unstamped cases. Read the `idempotency_key` case and confirm it asserts the `<type>:<seq>:<uuid>` form and uniqueness per emission, not stability across replay.
  - *Checks:* Resolve the sequence counter used by the stamper — confirm it is the per-connection counter on `RuntimeConnection`, not a process-global counter. `02-app-server-api.md` §Event envelopes and ordering scopes monotonicity to the connection.
  - *Status:* ☐ unverified

- **O3 — All six §Event envelopes and ordering invariants hold under test, including `input_accepted` before caused events and exactly-once `turn_finished` after the final delta**
  - *Claim:* The Task 13 ordering comparator passes against traces produced by this router.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::ordering_invariants)'` — expect six passing cases driven by `lotta_testkit::fixtures::traces`. Confirm the stale-lease invariant case asserts zero events after a replacement turn owns the scope.
  - *Status:* ☐ unverified

- **O4 — `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` rejects the 257th subscription and broadcast preserves stable connection-ordinal order**
  - *Claim:* The subscription cap rejects visibly, and two connections receive a broadcast in ordinal order.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::subscriptions)'` — expect `rejects_at_subscription_max` (naming the Task 05 constant) and `broadcast_preserves_ordinal_order` to pass.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::)'` and sees the Runtime group, envelope stamping, all six ordering invariants, and the subscription cap pass**
  - *Claim:* The ws module passes with the six ordering-invariant cases present.
  - *Evidence to collect:* Run the filter and confirm zero failures and six `ordering_invariants` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/queue.rs` snapshots (Task 18) are now framed as `update_queue`; confirm `queue::wire_transitions` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-app-server/src/listener.rs` (Task 14) now hands authenticated sockets to this router; confirm `auth::origin_bearing_loopback_rejected` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

The other fifteen command groups are Tasks 62–73. `sync`'s snapshot replay semantics are Task 73; this task routes the command and returns `sync_response`.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
