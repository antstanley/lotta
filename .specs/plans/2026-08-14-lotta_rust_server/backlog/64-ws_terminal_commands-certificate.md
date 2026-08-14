# Done Certificate — Task 64: WebSocket terminal command group

**Task:** [64-ws_terminal_commands.md](64-ws_terminal_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 64. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 64) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `terminal_spawn`/`terminal_input`/`terminal_resize`/`terminal_kill` scoped by connection and `terminal_id`, with the two-second Strict-Mode reuse window.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not error on input or resize for an absent session: `02-app-server-api.md` §WebSocket command groups states they are no-ops.

## Obligations

- **O1 — Sessions are scoped by connection and `terminal_id`; one connection cannot address another's session**
  - *Claim:* A `terminal_input` naming another connection's `terminal_id` is a no-op.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::scoping)'` — expect PASS asserting the other session received nothing.
  - *Status:* ☐ unverified

- **O2 — Spawn emits `terminal_spawned`, output emits `terminal_output`, and both exit and spawn failure emit `terminal_exited`**
  - *Claim:* The three message types fire at their documented points, including on spawn failure.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::lifecycle)'` — expect four cases: spawned, output, exit, and spawn-failure-emits-exited.
  - *Status:* ☐ unverified

- **O3 — Input and resize for an absent session are no-ops rather than errors**
  - *Claim:* Both commands against an unknown `terminal_id` succeed silently with no state change and no error message.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::absent_session_noop)'` — expect two cases asserting zero emissions and no error response.
  - *Status:* ☐ unverified

- **O4 — A live session younger than two seconds is reused on repeat spawn and ignores kill during that window**
  - *Claim:* A second spawn within two seconds returns the existing session, and a kill inside the window does not terminate it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::strict_mode_window)'` — expect `reuses_within_two_seconds`, `spawns_new_after_window`, and `ignores_kill_within_window`, all driven by the fake clock.
  - *Checks:* Resolve the age comparison's clock source — confirm it is the Task 06 `Clock` port so the window is deterministic in tests.
  - *Status:* ☐ unverified

- **O5 — Connection cleanup kills that connection's sessions and leaves no orphan process**
  - *Claim:* Dropping a connection terminates its terminals and the process group is empty afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::cleanup)'` — expect PASS with a process-group emptiness assertion.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::)'` and sees connection scoping, the lifecycle messages, absent-session no-ops, the two-second reuse window, and cleanup pass**
  - *Claim:* The terminal group passes with no orphan process.
  - *Evidence to collect:* Run the filter and confirm zero failures and the three `strict_mode_window` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/builtin/shell/` (Task 38) owns PTY spawning; confirm `builtin::shell::two_stage_kill` still passes with terminal sessions attached : ☐ (PRESERVED / REGRESSION)

## Residue

Background-process snapshots are listener state messages rather than terminal sessions and belong to Task 72.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
