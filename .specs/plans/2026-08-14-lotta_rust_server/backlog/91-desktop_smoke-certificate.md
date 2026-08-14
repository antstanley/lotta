# Done Certificate — Task 91: Desktop smoke suite

**Task:** [91-desktop_smoke.md](91-desktop_smoke.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 91. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 91) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a Desktop client connecting, syncing state, executing a turn, approving a tool, changing cwd, and reconnecting against the assembled binary.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must exercise the assembled binary: `00-overview.md` §Compatibility definition defines the Desktop surface as a smoke suite that connects, syncs, executes, approves, changes cwd, and reconnects.

## Obligations

- **O1 — The client connects, syncs authoritative snapshots, and executes a turn against the assembled binary over a real socket**
  - *Claim:* The flow runs against `target/*/lotta`, not an in-process server.
  - *Evidence to collect:* Run `cargo nextest run --test desktop -E 'test(connect_sync_turn)'` — expect PASS; read the harness and confirm it spawns the built binary and connects over TCP.
  - *Status:* ☐ unverified

- **O2 — A tool approval is requested, approved, and the tool executes exactly once**
  - *Claim:* `control_request` is received, the approval is sent, and the tool's execution count is one.
  - *Evidence to collect:* Run `cargo nextest run --test desktop -E 'test(approval)'` — expect PASS asserting one `control_request`, one approval, and an execution count of one on the fake tool port.
  - *Checks:* Resolve the approval resolution path — confirm it validates request ID, tool call ID, and lease generation (Task 56) rather than accepting any approval frame.
  - *Status:* ☐ unverified

- **O3 — A cwd change applies to subsequent turns**
  - *Claim:* After the change, the next turn's resolved working directory is the new one.
  - *Evidence to collect:* Run `cargo nextest run --test desktop -E 'test(cwd_change)'` — expect PASS asserting the resolved cwd on the following turn.
  - *Status:* ☐ unverified

- **O4 — Disconnect and reconnect recover subscriptions and the event sequence with no lost state**
  - *Claim:* After reconnect the client's subscriptions are restored, `event_seq` continues, and the conversation state matches pre-disconnect.
  - *Evidence to collect:* Run `cargo nextest run --test desktop -E 'test(reconnect)'` — expect PASS asserting all three properties.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test desktop` against the built binary and watches connect, sync, a turn, a tool approval, a cwd change, and a reconnect complete**
  - *Claim:* The Desktop suite is green against a real binary over a real socket.
  - *Evidence to collect:* Run the command, confirm zero failures, and confirm the harness log shows a spawned binary process and a TCP connection.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/ws/sync.rs` (Task 73) serves reconnect; confirm `ws::recovery::reconnect` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-runtime/src/approval/` (Task 56) serves the approval; confirm `approval::resolutions` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

This suite completes `00-overview.md` §Implementation acceptance criterion 4 alongside Task 90.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
