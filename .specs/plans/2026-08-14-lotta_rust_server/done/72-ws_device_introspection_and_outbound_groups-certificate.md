# Done Certificate — Task 72: Device commands, introspection, and outbound message groups

**Task:** [72-ws_device_introspection_and_outbound_groups.md](72-ws_device_introspection_and_outbound_groups.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-25 — implementation bookmark `lotta-implementation` = `eadc0670`

> Verification protocol for Task 72. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 72) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** slash/mod command execution, remove queue item, branch search/checkout, secret list/apply, `app_server_info`, and coverage of all six outbound message groups.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not turn a listener service into a model-facing tool: `05-tools-and-extensions.md` §Assumptions *Tool parity* keeps process snapshots and terminal sessions as protocol services.

## Obligations

- **O1 — All five device commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Device commands row has a passing case.
  - *Evidence:* The device-command selector passed 8/8; pinned command shapes and fixture-derived round trips cover slash/mod execution, queue removal, branch search/checkout, and secret list/apply.
  - *Status:* **SATISFIED**

- **O2 — Remove-queue-item goes through the queue and emits the `dequeued` or `cancelled` disposition with an authoritative snapshot**
  - *Claim:* The command produces the wire transition and a queue snapshot rather than mutating the queue silently.
  - *Evidence:* Queue-removal coverage passed 4/4. The deterministic active-turn production test cancelled a real `ActiveAdmission` queue item while the registry lock was held; `RuntimeEvent::UpdateQueue` emitted the removed disposition and authoritative snapshot broadcast, and the item was never pumped.
  - *Status:* **SATISFIED**

- **O3 — `app_server_info` reports protocol version 1 and the supported command set, and requires authentication**
  - *Claim:* The response's protocol version is 1 and an unauthenticated request is rejected.
  - *Evidence:* Introspection coverage passed 3/3, verifying protocol version 1, authentication enforcement, and truthful supported-capability reporting.
  - *Status:* **SATISFIED**

- **O4 — Every §Outbound message groups row has at least one message type emitted and covered by a test**
  - *Claim:* Control, Admission, State, Stream, Terminal, and Management each have covered emissions.
  - *Evidence:* Fixture-derived outbound coverage passed 6/6, one passing case for each of Control, Admission, State, Stream, Terminal, and Management.
  - *Status:* **SATISFIED**

- **O5 — Background-process snapshots are emitted as listener state messages and are not registered as model-facing tools**
  - *Claim:* The snapshot is a state message and no tool with that name exists in the registry.
  - *Evidence:* Background-snapshot coverage passed 1/1: a real `ShellToolBundle` manager emitted the snapshot through `RuntimeEvent::UpdateDeviceStatus`, and no corresponding model-facing tool was registered.
  - *Status:* **SATISFIED**

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence:* Full `lotta-app-server` passed 479/479 and root tests passed 34/34. Format, strict Clippy, rustdoc, and deny gates all passed. Source review found a 69-line maximum function, all lines at most 100 columns, and no panic additions or lint suppressions.
  - *Status:* **SATISFIED**

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::device::) + test(ws::introspection::) + test(ws::outbound::)'` and sees five device commands, queue-routed removal, protocol version 1, and all six outbound groups covered**
  - *Claim:* The device, introspection, and outbound coverage tests pass.
  - *Evidence:* The broad device, introspection, and outbound selector passed 30/30 with zero failures.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/queue.rs` (Task 18) serves removal: `queue::wire_transitions` passed 5/5 — **PRESERVED**.
- `crates/lotta-extensions/src/mods/host.rs` (Task 45) serves mod commands: `mods::registrations` passed 6/6 — **PRESERVED**.

## Residue

Channel commands are Task 79's group; this task covers the remaining ungrouped surface.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-25 at `lotta-implementation` bookmark `eadc0670`. All seven obligations are satisfied: fixture-pinned device routing, production-path queue cancellation with authoritative wire state, authenticated protocol-v1 introspection with truthful capabilities, all six outbound groups, and listener-only background snapshots are covered by passing focused tests. The broad selector and full repository gates pass, source constraints remain clean, and both queue and mod-registration regressions are preserved.
