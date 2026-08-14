# Done Certificate — Task 59: Runtime bounds and structured observability

**Task:** [59-runtime_bounds_and_observability.md](59-runtime_bounds_and_observability.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 59. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 59) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the ten runtime bounds enforced with progress assertions, and structured events and metrics that exclude prompts, message bodies, tool inputs, and secrets.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not log user content: `03-runtime-and-turns.md` §Observability excludes prompt text, message bodies, tool inputs, credentials, and secret-substituted commands by default.

## Obligations

- **O1 — All ten §Runtime bounds constants exist with the spec's names and defaults and have below/at/above tests**
  - *Claim:* `TURN_PROVIDER_RETRIES_MAX`, `TURN_EMPTY_RESPONSE_RETRIES_MAX`, `CONTEXT_OVERFLOW_COMPACTIONS_MAX`, `TURN_TOOL_CALLS_MAX`, `TURN_STEPS_MAX`, `TURN_CANCEL_GRACE_MS`, `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT`, `EXTERNAL_TOOL_CALL_TIMEOUT_MS`, `APPROVAL_WAIT_MS_MAX`, and `QUEUE_PUMP_BATCH_MAX` are defined with the table's defaults.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/bounds.rs` and compare each name and value against the §Runtime bounds table. Run `cargo nextest run -p lotta-runtime -E 'test(bounds::)'` — expect three cases per constant.
  - *Checks:* Resolve `TURN_STEPS_MAX` and `TURN_TOOL_CALLS_MAX` — confirm they are distinct constants with distinct at-limit behavior (`step_limit` termination versus stopping before another tool), not one constant serving both.
  - *Status:* ☐ unverified

- **O2 — Every loop asserts measurable progress toward one of the bounds**
  - *Claim:* Each loop in the runtime crate either iterates a bounded collection or asserts progress toward a named bound.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(bounds::loops_assert_progress)'` — expect PASS. Read the loops the test enumerates and confirm each has an assertion or a bounded iterator, per `development-guidelines.md` §Limits and bounds.
  - *Status:* ☐ unverified

- **O3 — Structured events carry the ten §Observability fields and exclude prompt text, message bodies, tool inputs, credentials, and secret-substituted commands**
  - *Claim:* A captured turn's events contain all ten fields and none of the five excluded classes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(observe::event_fields) + test(observe::exclusions)'` — expect the ten-field assertion and five absence assertions using marker values planted in the prompt, a message body, a tool input, a credential, and a substituted command.
  - *Status:* ☐ unverified

- **O4 — All ten §Observability metric families are emitted and named**
  - *Claim:* Each family appears in the metrics registry after a turn that exercises it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(observe::metric_families)'` — expect ten cases named for the ten families.
  - *Status:* ☐ unverified

- **O5 — Each Lotta-hardening bound has a boundary fixture proving ordinary baseline clients are unaffected**
  - *Claim:* For each bound marked hardening in the §Runtime bounds table, a fixture representing ordinary baseline usage stays below the bound.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(bounds::hardening_does_not_affect_baseline)'` — expect one case per hardening row, each replaying a `fixtures/reference-traces/` capture and asserting the bound was never reached.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(bounds::) + test(observe::)'` and sees ten bounds with boundary coverage, loop progress assertions, ten event fields with five exclusions, and ten metric families pass**
  - *Claim:* The bounds and observability modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and thirty or more bound cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/` (Tasks 19, 55, 57, 58) loops are now bound-asserted; confirm `turn::terminal_once`, `cancel::step_order`, and `compaction::effects` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

Process-level logging fields and metrics exposure are Task 84; this task owns runtime-scoped events and counters.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
