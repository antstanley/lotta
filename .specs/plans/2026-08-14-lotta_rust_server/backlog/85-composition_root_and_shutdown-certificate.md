# Done Certificate — Task 85: Composition root and the eight-step shutdown

**Task:** [85-composition_root_and_shutdown.md](85-composition_root_and_shutdown.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 85. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 85) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the workspace-root `src/main.rs` binary composing every adapter and executing the eight-step SIGTERM/SIGINT sequence, with lazy restart reload.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add a configuration-reload signal: `07-channels-and-operations.md` §Shutdown and restart defines an eight-step SIGTERM/SIGINT sequence and no reload.

## Obligations

- **O1 — `src/main.rs` at the workspace root is the only composition point and contains no business logic**
  - *Claim:* The binary wires adapters and starts the server; no domain or runtime decision is implemented there.
  - *Evidence to collect:* Read `src/main.rs` and confirm it only constructs adapters and calls into `lotta-app-server`/`lotta-runtime`. Run `cargo nextest run -E 'test(composition::no_business_logic)'` — expect PASS, asserting the file's function count and that no `TurnState` or queue decision appears.
  - *Checks:* Resolve the binary target — confirm it is the workspace-root `src/main.rs` named by `architecture-principles.md` §Workspace layout, and that no crate-local `main.rs` exists under `crates/`.
  - *Status:* ☐ unverified

- **O2 — All eight shutdown steps execute in order on SIGTERM and SIGINT, bounded by `SHUTDOWN_GRACE_MS`**
  - *Claim:* A recorded shutdown log matches the eight §Shutdown and restart steps and completes within the grace bound.
  - *Evidence to collect:* Run `cargo nextest run -E 'test(shutdown::step_order)'` — expect PASS comparing the log to the eight steps for both signals. Run the integration case sending a real SIGTERM to the built binary and confirm exit code 0 within `SHUTDOWN_GRACE_MS`.
  - *Status:* ☐ unverified

- **O3 — Shutdown abandons no child process or compatibility host, and releases the store lock**
  - *Claim:* After exit, no descendant process remains and the advisory lock file is released.
  - *Evidence to collect:* Run `cargo nextest run -E 'test(shutdown::no_orphans)'` — expect PASS asserting an empty process group and a released lock, per `02-app-server-api.md` §Responsibilities item 7.
  - *Status:* ☐ unverified

- **O4 — Restart reloads agents lazily, validates transcript manifests, restores schedules, and recompiles prompts only when inputs changed**
  - *Claim:* A restart performs the four documented behaviors and does not eagerly load every agent.
  - *Evidence to collect:* Run `cargo nextest run -E 'test(restart::)'` — expect `agents_lazy`, `manifests_validated`, `schedules_restored`, and `prompt_recompiled_only_on_change`, the last asserting zero recompilations when `rawSystemHash` and `memfsRevision` are unchanged.
  - *Status:* ☐ unverified

- **O5 — There is no configuration-reload signal handler; only SIGTERM and SIGINT are handled**
  - *Claim:* The signal handler set is exactly SIGTERM and SIGINT.
  - *Evidence to collect:* Grep `src/shutdown.rs` and `src/main.rs` for `SIGHUP` — expect zero matches. Run `cargo nextest run -E 'test(shutdown::handled_signals)'` — expect PASS asserting the handled set, since `07-channels-and-operations.md` §Shutdown and restart defines no reload.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer starts the built binary, sends SIGTERM, and observes the eight-step sequence in the log, exit within the grace bound, no orphan process, and a released store lock**
  - *Claim:* The shutdown sequence is observable against the real binary.
  - *Evidence to collect:* Start `target/debug/lotta server --backend local --listen`, send SIGTERM, and confirm the eight log lines in order, a 0 exit code, an empty process group, and no residual lock file.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/cancel.rs` (Task 57) is driven by shutdown step 3; confirm `cancel::exactly_once` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-channels/src/supervisor.rs` (Task 78) is stopped by step 6; confirm `topology::supervision` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Feature-flag selection of adapters happens here; `architecture-principles.md` §Dependency graph requires flags to select adapters without altering domain semantics.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
