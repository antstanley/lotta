# Done Certificate — Task 19: Minimal turn loop against fake provider and tool ports

**Task:** [19-minimal_turn_loop_fake_ports.md](19-minimal_turn_loop_fake_ports.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 19. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 19) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a turn that streams provider events, persists text and reasoning projections, executes one sequential local tool, and completes exactly once — all against fake ports.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not execute tools concurrently and must not drop reasoning events; both are load-bearing for `06-model-providers.md` §Streaming invariants and the undecided *Parallel tools* question.

## Obligations

- **O1 — Text and reasoning deltas are both projected and streamed; a stream that carries `ReasoningDelta` or `RedactedReasoning` does not lose them**
  - *Claim:* Each `TextDelta`, `ReasoningDelta`, and `RedactedReasoning` produces a persisted projection and one `stream_delta`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::projections)'` — expect three cases. Replay the reasoning fixture from Task 12 through the loop and assert the projection count equals the reasoning-event count.
  - *Status:* ☐ unverified

- **O2 — Tool-call arguments are buffered under a declared bound and validated only at `ToolCallEnd`, with tool-call IDs stable across deltas**
  - *Claim:* Partial JSON is not parsed mid-stream; validation happens once at end; the ID observed at start equals the ID at end and in the result.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::tool_call_assembly)'` — expect `validates_only_at_end`, `id_stable_across_deltas`, and `rejects_over_argument_bound` to pass, the last naming `TOOL_ARGUMENT_BYTES_MAX` from `06-model-providers.md` §Limits.
  - *Checks:* Resolve the JSON validation call — confirm it runs in the `ToolCallEnd` branch only. A parse in the delta branch would violate `06-model-providers.md` §Streaming invariants.
  - *Status:* ☐ unverified

- **O3 — Tool execution is sequential: two tool calls in one step execute one after the other, in stream order**
  - *Claim:* The fake tool port observes non-overlapping executions in the order the calls ended.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::sequential_tools)'` — expect PASS; read the test and confirm it asserts non-overlap via the fake's entry/exit log, not merely completion order.
  - *Checks:* Resolve the concurrency decision — confirm it reads the Task 08 parallel-safety classification and finds `Sequential`. `03-runtime-and-turns.md` §Assumptions leaves *Parallel tools* undecided, so concurrent execution here would decide it.
  - *Status:* ☐ unverified

- **O4 — A terminal stop completes the turn exactly once, after its final stream delta, and a superseded turn emits nothing**
  - *Claim:* One terminal event is emitted per admitted turn, positioned after the last delta; a turn whose lease was replaced emits zero events.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(turn::terminal_once)'` — expect `exactly_one_terminal`, `terminal_after_final_delta`, and `superseded_turn_emits_nothing` to pass.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(turn::)'` and sees reasoning projections, end-only argument validation, sequential tool execution, and exactly-once terminal completion pass against the fake ports**
  - *Claim:* The turn module passes with no real adapter linked.
  - *Evidence to collect:* Run the filter and confirm zero failures; then run `cargo tree -p lotta-runtime --edges normal` and confirm no adapter crate is present.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/queue.rs` (Task 18) pumps this loop; confirm `queue::pump` still passes with a real loop attached instead of a stub : ☐ (PRESERVED / REGRESSION)

## Residue

Approval, denial, external-tool, retry, compaction, and cancellation branches are Tasks 55–58. This task deliberately covers only the local-tool and terminal-stop path so the vertical slice can close.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
