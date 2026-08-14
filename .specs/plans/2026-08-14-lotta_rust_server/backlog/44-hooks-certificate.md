# Done Certificate — Task 44: Hook events, command and prompt executors, and owner attribution

**Task:** [44-hooks.md](44-hooks.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 44. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 44) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the eleven hook events with the prompt-hook subset restriction, sandboxed command hooks, and per-owner failure attribution.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not invent hook events: `05-tools-and-extensions.md` §Hooks and mods fixes the event set, and existing hooks keyed to those names would never fire against a different set.

## Obligations

- **O1 — All eleven hook events exist with the spec's names and each fires at its documented point**
  - *Claim:* The event enum has eleven variants and each has a firing test at the correct pipeline position.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(hooks::events)'` — expect eleven firing cases. Compare the names against `../letta-code/src/hooks/types.ts`.
  - *Checks:* Resolve the pre-tool and post-tool firing points against the Task 33 pipeline stage log — confirm pre-tool fires before the permission gate and post-tool after the executor, matching `05-tools-and-extensions.md` §Execution pipeline.
  - *Status:* ☐ unverified

- **O2 — Prompt hooks are supported for exactly the seven permitted events, and registering a prompt hook for notification, pre-compact, or a session event is rejected**
  - *Claim:* The seven permitted events accept a prompt hook; the four command-only events reject one.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(hooks::prompt_subset)'` — expect seven acceptance cases and four rejection cases.
  - *Status:* ☐ unverified

- **O3 — Command hooks run as sandboxed child processes under `COMMAND_HOOK_TIMEOUT_MS_DEFAULT`, and prompt hooks under `PROMPT_HOOK_TIMEOUT_MS_DEFAULT`**
  - *Claim:* The two timeouts are distinct named constants with the §Limits defaults, and a command hook cannot read outside the sandbox root.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(hooks::timeouts) + test(hooks::command_is_sandboxed)'` — expect the two timeout cases (60,000 and 30,000) and the confinement case.
  - *Status:* ☐ unverified

- **O4 — Block, modify, and allow outcomes work, a failure is attributed to its owning hook, and `HOOKS_PER_EVENT_MAX` rejects at the limit**
  - *Claim:* Each outcome changes the pipeline as documented, a failing hook's error names its owner, and the 65th hook on an event is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(hooks::outcomes) + test(hooks::bounds)'` — expect three outcome cases, an attribution case naming the owner, and below/at/above cases for `HOOKS_PER_EVENT_MAX`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(hooks::)'` and sees eleven events, the seven-event prompt subset with four rejections, both timeouts, sandboxed command hooks, and owner attribution pass**
  - *Claim:* The hooks module passes with the full event set.
  - *Evidence to collect:* Run the filter and confirm zero failures and eleven `events` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/pipeline.rs` (Task 33) gains real hook stages; confirm `pipeline::stage_order` and `pipeline::owner_attribution` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

Mod-registered hooks arrive through Task 45's host and reuse this event set rather than defining another.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
