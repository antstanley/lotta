# Done Certificate — Task 55: Provider and tool loop branches and typed stop reasons

**Task:** [55-provider_tool_loop_branches.md](55-provider_tool_loop_branches.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 55. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 55) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the four tool-call branches, context-pressure and retry paths, and the distinct typed stop reasons of the §Provider and tool loop diagram.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not collapse stop reasons: `03-runtime-and-turns.md` §Provider and tool loop keeps them distinct so recovery and clients can tell an empty response from a quota error.

## Obligations

- **O1 — All four tool-call branches are reachable and produce their documented result: denied, approval-pending, external, and local**
  - *Claim:* Each branch has a passing case and the denied branch produces a structured denied result rather than an error.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::branches)'` — expect four cases named for the four branches; read the denied case and confirm it asserts a structured tool result appended to the conversation.
  - *Status:* ☐ unverified

- **O2 — The five stop reasons remain distinct typed values that survive persistence**
  - *Claim:* Empty response, context overflow, transport failure, provider quota error, and user cancellation are five distinct variants and each round-trips.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::stop_reasons)'` — expect five cases asserting distinct discriminants after a serialization round trip.
  - *Checks:* Resolve the quota stop reason's source — confirm it maps from `ProviderError::Quota` (Task 07) and not from `ProviderError::RateLimit`; §Provider and tool loop keeps provider quota error distinct.
  - *Status:* ☐ unverified

- **O3 — Context pressure compacts once, recompiles, and retries under a bound; a retryable error emits a retry event and backs off using the Task 48 policy**
  - *Claim:* Pressure triggers exactly one compaction before retrying, and the retry delays match the deterministic policy.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::context_pressure) + test(turn::retryable_error)'` — expect the compaction-count assertion and the delay-sequence assertion, the second reusing the Task 48 recorded sequence.
  - *Status:* ☐ unverified

- **O4 — Provider fallback never changes the persisted model unless a user command does**
  - *Claim:* After a fallback, the stored agent and conversation model values are unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::fallback_preserves_model)'` — expect PASS asserting zero model writes to the store fake.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(turn::branches) + test(turn::stop_reasons) + test(turn::context_pressure) + test(turn::retryable_error) + test(turn::fallback_preserves_model)'` and sees every branch and stop reason covered**
  - *Claim:* The loop-branch module passes with all four branches and five stop reasons.
  - *Evidence to collect:* Run the filter and confirm zero failures and nine or more cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/loop.rs` (Task 19) is extended here; confirm `turn::sequential_tools`, `turn::tool_call_assembly`, and `turn::terminal_once` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

Approval resolution semantics are Task 56; compaction mechanics are Task 58. This task owns only the branch that emits and waits.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
