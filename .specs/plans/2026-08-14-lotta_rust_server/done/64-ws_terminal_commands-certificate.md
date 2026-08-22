# Done Certificate — Task 64: WebSocket terminal command group

**Task:** [64-ws_terminal_commands.md](64-ws_terminal_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-21

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
  - *Evidence:* Exact selector passed 2/2. Sessions live in a map keyed by `(ConnectionId, terminal_id)` with monotone never-reused connection ids; every accessor keys through the connection, and the test proves foreign input/resize/kill produce zero emissions while the owner's session stays live.
  - *Status:* ☑ SATISFIED

- **O2 — Spawn emits `terminal_spawned`, output emits `terminal_output`, and both exit and spawn failure emit `terminal_exited`**
  - *Claim:* The three message types fire at their documented points, including on spawn failure.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::lifecycle)'` — expect four cases: spawned, output, exit, and spawn-failure-emits-exited.
  - *Evidence:* Exact selector passed 4/4. Spawn success emits `terminal_spawned`; output flows from the real child process (marker echo, live pid); natural exit is detected by real `child.wait()` and emits `terminal_exited`; spawn failure maps to a scrubbed `terminal_exited` with no record inserted.
  - *Status:* ☑ SATISFIED

- **O3 — Input and resize for an absent session are no-ops rather than errors**
  - *Claim:* Both commands against an unknown `terminal_id` succeed silently with no state change and no error message.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::absent_session_noop)'` — expect two cases asserting zero emissions and no error response.
  - *Evidence:* Exact selector passed 2/2. Input and resize against an unknown `terminal_id` emit nothing, produce no error frame, and mutate no state.
  - *Status:* ☑ SATISFIED

- **O4 — A live session younger than two seconds is reused on repeat spawn and ignores kill during that window**
  - *Claim:* A second spawn within two seconds returns the existing session, and a kill inside the window does not terminate it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::strict_mode_window)'` — expect `reuses_within_two_seconds`, `spawns_new_after_window`, and `ignores_kill_within_window`, all driven by the fake clock.
  - *Checks:* Resolve the age comparison's clock source — confirm it is the Task 06 `Clock` port so the window is deterministic in tests.
  - *Evidence:* Exact selector passed 3/3 driven entirely by the injected Clock port (`advance_ms(1999)` reuse, `advance_ms(WINDOW)` new spawn, kill inside the window ignored with the pid still live). Age comparisons use only the clock; after-window repeat spawn matches the pinned kill-old-then-spawn-fresh behavior with pid-guarded exit suppression. A follow-up fix wired the exited flag so a dead session is never reused inside the window, covered by its own regression test.
  - *Status:* ☑ SATISFIED

- **O5 — Connection cleanup kills that connection's sessions and leaves no orphan process**
  - *Claim:* Dropping a connection terminates its terminals and the process group is empty afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::cleanup)'` — expect PASS with a process-group emptiness assertion.
  - *Evidence:* Exact selector passed. The test spawns a real shell plus a background descendant, verifies the shared process group via `ps`, disconnects, and asserts zero live group members; cleanup signals the whole group TERM then KILL and reaps the leader.
  - *Status:* ☑ SATISFIED

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 231/231 and Task 38 shell suite 31/31 preserved. New functions stay within 70 lines and 100 columns; constants use units-last names; no production unwrap/expect and no new lint suppressions; workspace residue remains confined to the recorded external pinned-SHA/CLI family.
  - *Status:* ☑ SATISFIED

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::)'` and sees connection scoping, the lifecycle messages, absent-session no-ops, the two-second reuse window, and cleanup pass**
  - *Claim:* The terminal group passes with no orphan process.
  - *Evidence to collect:* Run the filter and confirm zero failures and the three `strict_mode_window` cases.
  - *Evidence:* Broad selector passed 20/20 with zero failures covering scoping, lifecycle messages, absent-session no-ops, the three Strict-Mode window cases, cleanup, and wire-shape pins. Independent review returned `CORRECT / DONE` and its flagged dead-liveness defect was fixed with a mutation-covered regression test before certification.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/builtin/shell/` (Task 38) owns PTY spawning; confirm `builtin::shell::two_stage_kill` still passes with terminal sessions attached : ☑ PRESERVED

## Residue

Background-process snapshots are listener state messages rather than terminal sessions and belong to Task 72.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O7 obligations are discharged by exact selectors and first-hand evidence over real child processes. Sessions are structurally scoped by connection and terminal id; lifecycle messages fire at the documented points including spawn failure; absent-session input and resize are silent no-ops; the two-second Strict-Mode reuse window is clock-driven with kill-inside-window ignored and dead-pid reuse prevented; and connection cleanup terminates whole process groups leaving no orphans. Sessions are pipe-backed rather than PTY-backed (documented divergence with upgrade path), resize is inert on pipes, and exit-message parity matches the pinned pid-guarded behavior. The Task 38 shell regression suite is preserved.
