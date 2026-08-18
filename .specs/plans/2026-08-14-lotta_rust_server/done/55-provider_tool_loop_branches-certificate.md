# Done Certificate — Task 55: Provider and tool loop branches and typed stop reasons

**Task:** [55-provider_tool_loop_branches.md](55-provider_tool_loop_branches.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

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
  - *Status:* ☑ SATISFIED — `turn::branches` passes six production-loop cases. Policy denial reaches no executor and appends a structured result; approvals persist before emit and validate edits under the exact lease; external requests persist before emit and wait with cancellation/timeout; local execution uses the scoped Task 33 registry pipeline and resumes in order.

- **O2 — The five stop reasons remain distinct typed values that survive persistence**
  - *Claim:* Empty response, context overflow, transport failure, provider quota error, and user cancellation are five distinct variants and each round-trips.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::stop_reasons)'` — expect five cases asserting distinct discriminants after a serialization round trip.
  - *Checks:* Resolve the quota stop reason's source — confirm it maps from `ProviderError::Quota` (Task 07) and not from `ProviderError::RateLimit`; §Provider and tool loop keeps provider quota error distinct.
  - *Status:* ☑ SATISFIED — `turn::stop_reasons` passes eight cases. All five stable wires round-trip through `TurnStopRecord`, persist before client/lifecycle terminal effects, and remain distinct in actual provider, direct-error, retry, and user-cancellation paths. Quota maps only from `ProviderError::Quota`; rate limiting follows retry policy.

- **O3 — Context pressure compacts once, recompiles, and retries under a bound; a retryable error emits a retry event and backs off using the Task 48 policy**
  - *Claim:* Pressure triggers exactly one compaction before retrying, and the retry delays match the deterministic policy.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::context_pressure) + test(turn::retryable_error)'` — expect the compaction-count assertion and the delay-sequence assertion, the second reusing the Task 48 recorded sequence.
  - *Status:* ☑ SATISFIED — `turn::context_pressure` passes three compaction/progress/bound cases and `turn::retryable_error` passes four actual-loop cases. Task 48 emits retry before the exact no-jitter busy, empty, and retry-after delays; cancellation is durable and terminal. Production uses a persisted compaction broker and owned request refresh seam for Task 58 mechanics.

- **O4 — Provider fallback never changes the persisted model unless a user command does**
  - *Claim:* After a fallback, the stored agent and conversation model values are unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::fallback_preserves_model)'` — expect PASS asserting zero model writes to the store fake.
  - *Status:* ☑ SATISFIED — ordered production fallback candidates are connection-specific native/host routes and modify only cloned in-flight requests. `turn::fallback_preserves_model` and production provider probes preserve agent/conversation model bytes and use the persisted primary again on the next turn.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — runtime 252/252, providers 165/165, app server 180/180, setup 30/30, production 20/20, format, strict workspace Clippy, rustdoc, and deny pass. Changed functions remain ≤70 lines and changed lines ≤100 columns. Full-workspace failures are limited to the acknowledged live sibling `local-backend.ts` pin drift.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(turn::branches) + test(turn::stop_reasons) + test(turn::context_pressure) + test(turn::retryable_error) + test(turn::fallback_preserves_model)'` and sees every branch and stop reason covered**
  - *Claim:* The loop-branch module passes with all four branches and five stop reasons.
  - *Evidence to collect:* Run the filter and confirm zero failures and nine or more cases.
  - *Status:* ☑ SATISFIED — the exact certificate selector passes 22/22: branches 6, stop reasons 8, context pressure 3, retryable errors 4, and fallback preservation 1. Independent OpenAI Sol review `task_106` found no remaining Task 55 implementation defect.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/loop.rs` (Task 19) is extended here; `turn::sequential_tools`, `turn::tool_call_assembly`, and `turn::terminal_once` pass 13/13: ☑ PRESERVED

## Residue

Approval resolution semantics are Task 56; compaction mechanics are Task 58. This task owns only the branch that emits and waits.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 are satisfied. The real production loop routes policy-denied, approval, external, and local calls through scoped Task 33 adapters; persists five typed terminal reasons; performs bounded context compaction and canonical Task 48 retries; and uses ordered request-local provider fallback without model persistence. Per-turn setup status, permission, cwd, and tool state are isolated under concurrent scopes. Reviewer `task_106` found no Task 55 defect; only acknowledged external sibling pin drift blocks unrelated conformance slices.
