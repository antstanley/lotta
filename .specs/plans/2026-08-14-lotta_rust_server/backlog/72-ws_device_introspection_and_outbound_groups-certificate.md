# Done Certificate — Task 72: Device commands, introspection, and outbound message groups

**Task:** [72-ws_device_introspection_and_outbound_groups.md](72-ws_device_introspection_and_outbound_groups.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::device::commands)'` — expect five cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — Remove-queue-item goes through the queue and emits the `dequeued` or `cancelled` disposition with an authoritative snapshot**
  - *Claim:* The command produces the wire transition and a queue snapshot rather than mutating the queue silently.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::device::remove_queue_item)'` — expect PASS asserting one transition and one snapshot.
  - *Checks:* Resolve the removal call — confirm it is the Task 18 queue operation that emits transitions, not a direct collection removal.
  - *Status:* ☐ unverified

- **O3 — `app_server_info` reports protocol version 1 and the supported command set, and requires authentication**
  - *Claim:* The response's protocol version is 1 and an unauthenticated request is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::introspection::)'` — expect `reports_protocol_version_1` (compared against `../letta-code/src/types/app-server-info.ts:1`) and `requires_authentication`.
  - *Status:* ☐ unverified

- **O4 — Every §Outbound message groups row has at least one message type emitted and covered by a test**
  - *Claim:* Control, Admission, State, Stream, Terminal, and Management each have covered emissions.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::outbound::group_coverage)'` — expect six cases named for the six rows, derived from the fixture's message list rather than hard-coded.
  - *Status:* ☐ unverified

- **O5 — Background-process snapshots are emitted as listener state messages and are not registered as model-facing tools**
  - *Claim:* The snapshot is a state message and no tool with that name exists in the registry.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::device::background_snapshot_is_state)'` — expect PASS asserting the message type and that the tool registry has no matching entry, per `02-app-server-api.md` §WebSocket command groups.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::device::) + test(ws::introspection::) + test(ws::outbound::)'` and sees five device commands, queue-routed removal, protocol version 1, and all six outbound groups covered**
  - *Claim:* The device, introspection, and outbound coverage tests pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and six `group_coverage` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/queue.rs` (Task 18) serves removal; confirm `queue::wire_transitions` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-extensions/src/mods/host.rs` (Task 45) serves mod commands; confirm `mods::registrations` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Channel commands are Task 79's group; this task covers the remaining ungrouped surface.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
