# Done Certificate — Task 54: Turn setup resolution order

**Task:** [54-turn_setup_resolution.md](54-turn_setup_resolution.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 54. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 54) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the ten-step turn setup in the specified order, with the two documented failure modes distinguishable by recovery.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not reorder setup: `03-runtime-and-turns.md` §Turn setup fixes the sequence, and resolving the toolset before permissions or MemFS would change which tools a turn sees.

## Obligations

- **O1 — Setup executes the ten steps of §Turn setup in order, and a recorded stage log matches the spec sequence**
  - *Claim:* The stage log for a successful setup is exactly the ten steps in the documented order.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(setup::step_order)'` — expect PASS comparing the recorded log against the ten §Turn setup steps.
  - *Checks:* Resolve the toolset resolution call — confirm it invokes the Task 32 registry rather than an inline list, and that permission mode (Task 34) is applied before MemFS sync, as step 3 precedes step 4.
  - *Status:* ☐ unverified

- **O2 — A deleted cwd falls back and records the original path for a one-time reminder that fires once**
  - *Claim:* Setup succeeds with the fallback directory and the reminder is emitted on the first turn only.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(setup::deleted_cwd)'` — expect `falls_back` and `reminder_fires_once` to pass, the second running two turns and asserting a single reminder.
  - *Status:* ☐ unverified

- **O3 — Cross-agent and archived-invalid access are rejected before any allocation**
  - *Claim:* Resolving agent A's conversation under agent B fails, and an archived-invalid access fails, both without creating state.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(setup::access_rejection)'` — expect both cases asserting zero store writes.
  - *Status:* ☐ unverified

- **O4 — Failure before provider admission returns a terminal error without appending a user message, while failure after durable input append records a distinguishable interrupted/error outcome**
  - *Claim:* The two failure points produce different persisted outcomes that recovery can tell apart.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(setup::failure_modes)'` — expect `pre_admission_appends_nothing` and `post_append_records_interrupted`, the first asserting the transcript is unchanged and the second asserting a recoverable outcome marker.
  - *Status:* ☐ unverified

- **O5 — The merged tool set contains built-in, MCP, mod, channel, and controller-owned external tools, and the toolset resolution honours the agent's configured toolset and allowlist**
  - *Claim:* All five tool sources appear in the merged registry snapshot for a turn that has each configured.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(setup::tool_merge)'` — expect one case per source (five) plus `respects_toolset_and_allowlist`.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(setup::)'` and sees the ten-step order, deleted-cwd fallback with a one-time reminder, access rejection, the two failure modes, and the five-source tool merge pass**
  - *Claim:* The setup module passes with the step-order log asserted.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `step_order` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/loop.rs` (Task 19) now runs after real setup; confirm `turn::terminal_once` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-memfs/src/prompt/compile.rs` (Task 30) is invoked here; confirm `prompt::cache` still passes with skills supplied : ☐ (PRESERVED / REGRESSION)

## Residue

This task depends on tools, permissions, skills, hooks, and mods precisely because §Turn setup steps 5–9 require them; that is why the tool and extension packages are numbered before it.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
