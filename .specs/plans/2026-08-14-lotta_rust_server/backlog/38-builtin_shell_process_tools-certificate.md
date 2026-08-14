# Done Certificate — Task 38: Shell and process built-in tools

**Task:** [38-builtin_shell_process_tools.md](38-builtin_shell_process_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 38. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 38) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one-shot shell/exec, PTY session, background output, stdin, monitor, stop, and timeout under sandbox with the baseline output bounds.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not treat timeout as cancellation: `development-guidelines.md` §Async and concurrency requires timeout errors to remain distinct from cancellation.

## Obligations

- **O1 — The seven shell/process behaviors of §Rust built-ins execute: one-shot shell/exec, PTY session, background output, stdin, monitor, stop, and timeout**
  - *Claim:* Each behavior has a registered tool and a passing execution test.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::shell::)'` — expect at least seven execution cases named for the seven behaviors.
  - *Status:* ☐ unverified

- **O2 — Timeout is distinct from cancellation and both are bounded: `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT` interrupts, and a per-tool override stays bounded**
  - *Claim:* A tool exceeding the timeout yields the timeout outcome; a cancelled tool yields the interruption outcome; an override above the ceiling is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::shell::timeout)'` — expect `timeout_outcome`, `cancellation_outcome_differs`, and `override_bounded` to pass, driven by the fake clock.
  - *Checks:* Resolve the timeout outcome constructed here — confirm it is `ToolOutcome::Timeout` (Task 08), not `ProviderError::Timeout` (Task 07). `NAME SHADOWING`: both exist in scope once the runtime links both port modules.
  - *Status:* ☐ unverified

- **O3 — Child termination is two-stage: SIGTERM then SIGKILL after a 2,000 ms grace, and no orphan survives**
  - *Claim:* A child ignoring SIGTERM is killed after the grace, and no descendant process remains.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::shell::two_stage_kill)'` — expect PASS; the test spawns a SIGTERM-ignoring child, advances the fake clock by 2,000 ms, and asserts the process group is gone. The grace matches `03-runtime-and-turns.md` §Runtime bounds (`shell children use a 2,000 ms SIGTERM→SIGKILL grace`).
  - *Status:* ☐ unverified

- **O4 — Captured output is bounded at `CHILD_PROCESS_OUTPUT_BYTES_MAX` and the model-facing result clamps at the shell-family 30,000 characters with overflow written to a file**
  - *Claim:* A child emitting more than 16 MiB is bounded without unbounded buffering, and the model sees at most 30,000 characters plus a file pointer.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::shell::output_bounds)'` — expect the byte-cap and the character-clamp cases; the byte-cap case asserts peak buffer size, not only final length.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::shell::)'` and sees the seven behaviors, distinct timeout and cancellation outcomes, two-stage kill, and both output bounds pass**
  - *Claim:* The shell built-in module passes with no orphan process left behind.
  - *Evidence to collect:* Run the filter and confirm zero failures; after the run, confirm no test-spawned process remains via the harness's process-leak assertion.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/sandbox/` (Task 35) launches these children; confirm `sandbox::workspace` still passes with the shell launcher attached : ☐ (PRESERVED / REGRESSION)

## Residue

Background-process snapshots are listener state messages, not model-facing tools (`02-app-server-api.md` §WebSocket command groups); they are Task 72.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
