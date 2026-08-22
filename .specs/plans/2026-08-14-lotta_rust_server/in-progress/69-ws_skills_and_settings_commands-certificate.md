# Done Certificate — Task 69: WebSocket skills and settings command groups

**Task:** [69-ws_skills_and_settings_commands.md](69-ws_skills_and_settings_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 69. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 69) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** skills enable/disable plus the settings group — cwd map, reflection settings, and experiments.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not write settings outside the scopes `04-persistence-and-memfs.md` §State outside the backend root defines, or cross-runtime round trips break.

## Obligations

- **O1 — Skills enable and disable update the runtime's selected sources and emit a skills update snapshot**
  - *Claim:* Enabling a skill makes it available to the next turn's prompt compilation; disabling removes it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::skills::)'` — expect `enable_adds`, `disable_removes`, and `emits_snapshot`.
  - *Status:* ☐ unverified

- **O2 — The settings group covers cwd map, reflection settings, and experiments, each persisted through the correct side-store scope**
  - *Claim:* Each settings command writes to the scope §State outside the backend root assigns it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::scopes)'` — expect three cases asserting the written path.
  - *Checks:* Resolve the settings writer — confirm it is the Task 27 side store, not a direct filesystem write, so the atomic-write and conflict behavior applies.
  - *Status:* ☐ unverified

- **O3 — Reflection settings are validated before persistence and invalid values are rejected without writing**
  - *Claim:* An out-of-range reflection setting is rejected and nothing is persisted.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::reflection_validation)'` — expect PASS asserting zero writes; compare the validation rules against `../letta-code/src/websocket/listener/reflection-settings-validation.test.ts`.
  - *Status:* ☐ unverified

- **O4 — A cwd change applies to subsequent turns, and a missing directory records the original path for a one-time reminder**
  - *Claim:* The next turn uses the new cwd; a missing directory falls back and reminds once.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::cwd_change)'` — expect `applies_to_next_turn` and `missing_dir_falls_back_and_reminds_once`, reusing the Task 54 behavior.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::skills::) + test(ws::settings::)'` and sees skill enable/disable, three settings scopes, reflection validation, and cwd change pass**
  - *Claim:* Both groups pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the three settings scope cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/setup.rs` (Task 54) reads cwd and skills; confirm `setup::deleted_cwd` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Skill discovery itself is Task 36; this group only selects among discovered skills.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
