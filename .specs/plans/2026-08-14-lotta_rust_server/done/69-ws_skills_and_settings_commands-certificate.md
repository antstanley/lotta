# Done Certificate — Task 69: WebSocket skills and settings command groups

**Task:** [69-ws_skills_and_settings_commands.md](69-ws_skills_and_settings_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-23

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
  - *Evidence:* Exact selector passed 8/8 including `enable_adds`, `disable_removes`, and `emits_snapshot`. The chain is wired end-to-end: enable/disable update the bridge selection, `prepare_turn` feeds `selected_sources().ids()` into `SetupInput.selected_skills`, setup selects through Task 36 discovery over the pinned-correct `<storage>/.letta/skills` root, and a production-level test proves the next turn's prompt includes an enabled skill's body and excludes it after disable.
  - *Status:* ☑ SATISFIED

- **O2 — The settings group covers cwd map, reflection settings, and experiments, each persisted through the correct side-store scope**
  - *Claim:* Each settings command writes to the scope §State outside the backend root assigns it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::scopes)'` — expect three cases asserting the written path.
  - *Checks:* Resolve the settings writer — confirm it is the Task 27 side store, not a direct filesystem write, so the atomic-write and conflict behavior applies.
  - *Evidence:* Exact selector passed 3/3 asserting the written path per scope (`<workspace>/.letta/settings.local.json` for local-project, `<home>/.letta/settings.json` for global, both together). All writes go through the Task 27 side store (atomic write + CAS); a grep confirms zero direct filesystem writes in settings.rs. Experiments persist to the global scope.
  - *Status:* ☑ SATISFIED

- **O3 — Reflection settings are validated before persistence and invalid values are rejected without writing**
  - *Claim:* An out-of-range reflection setting is rejected and nothing is persisted.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::reflection_validation)'` — expect PASS asserting zero writes; compare the validation rules against `../letta-code/src/websocket/listener/reflection-settings-validation.test.ts`.
  - *Evidence:* Exact selector passed 8/8 with rules field-by-field identical to the pinned validation test plus stricter extras; rejections happen at decode before any store call with zero-write assertions via file-existence checks. Reflection persistence uses the pinned camelCase vocabulary (`reflectionSettingsByAgent`/`stepCount`/`mergeInstructions` plus flat companions), matching the checked-in corpus.
  - *Status:* ☑ SATISFIED

- **O4 — A cwd change applies to subsequent turns, and a missing directory records the original path for a one-time reminder**
  - *Claim:* The next turn uses the new cwd; a missing directory falls back and reminds once.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::settings::cwd_change)'` — expect `applies_to_next_turn` and `missing_dir_falls_back_and_reminds_once`, reusing the Task 54 behavior.
  - *Evidence:* Exact selector passed 2/2 reusing Task 54's `CwdResolution`; production wiring makes it real — `change_device_state` persists through `apply_cwd_change` and `prepare_turn` consults `cwd_for_next_turn` (claiming the one-time reminder on missing-directory fallback) before falling back to the persisted conversation cwd. Production tests prove the next turn uses the changed cwd and the reminder fires exactly once.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 373/373 and production binary 28/28 including the new wiring tests. Workspace residue remains confined to the recorded external pinned-SHA/CLI family. New functions stay within 70 lines and 100 columns; constants use units-last names; zero production unwrap/expect and no new lint suppressions in the cumulative diff.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::skills::) + test(ws::settings::)'` and sees skill enable/disable, three settings scopes, reflection validation, and cwd change pass**
  - *Claim:* Both groups pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the three settings scope cases.
  - *Evidence:* Broad selector passed 32/32 with zero failures covering skill enable/disable, three settings scopes, reflection validation, and cwd change. Independent round-2 review returned `CORRECT / DONE`, verifying all four remediations at source, diff, and empirical test level.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/setup.rs` (Task 54) reads cwd and skills; confirm `setup::deleted_cwd` still passes : ☑ PRESERVED

## Residue

Skill discovery itself is Task 36; this group only selects among discovered skills.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced evidence. Skills enable/disable mutate the runtime selection that the next turn's prompt compilation consumes over the pinned-correct `.letta/skills` discovery root; settings cover cwd map, reflection, and experiments each persisted through the correct Task 27 side-store scope with atomic writes; reflection settings validate before persistence using the pinned camelCase vocabulary matching the checked-in corpus; and cwd changes apply to subsequent turns with the one-time missing-directory reminder reused from Task 54. Round-1's wiring defects were fixed and verified by production-level tests. The Task 54 deleted-cwd regression suite is preserved.
