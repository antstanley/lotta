# Done Certificate — Task 33: Tool execution pipeline, secret substitution, and result clamps

**Task:** [33-tool_execution_pipeline_and_clamps.md](33-tool_execution_pipeline_and_clamps.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 33. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 33) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the ordered hook→permission→sandbox→secret-substitution→executor→post-hook→scrub→clamp→persist→emit pipeline with the baseline result clamps.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not reorder validation after policy: `05-tools-and-extensions.md` §Responsibilities item 2 requires validating every tool input before policy or execution.

## Obligations

- **O1 — The pipeline executes the ten stages in the §Execution pipeline order, and schema validation precedes every policy and execution step**
  - *Claim:* A traced call visits name resolution, schema validation, pre-tool hooks, permission and sandbox gate, secret substitution, executor, post hooks, scrub, clamp, persist, emit — in that order.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(pipeline::stage_order)'` — expect PASS; the test records a stage log and asserts the exact sequence. Trace one call: `{"name":"Read","input":{...}}` → resolved → validated → hooks → permitted → substituted → executed → scrubbed → clamped → persisted → emitted.
  - *Checks:* Resolve the validation call — confirm it is the JSON Schema validator for the tool's declared input schema, not a generic JSON parse. Policy running before validation would let an unvalidated path reach the permission matcher.
  - *Status:* ☐ unverified

- **O2 — Secrets are substituted only into child environments and provider requests, and never appear in a result, a log line, or a persisted record**
  - *Claim:* A tool whose command references a secret sees the value in its environment while the emitted result, the log, and the transcript entry contain only the placeholder.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(pipeline::secret_substitution)'` — expect PASS; the test asserts the marker value appears in the captured child environment and is absent from the result, the captured tracing output, and the persisted record.
  - *Status:* ☐ unverified

- **O3 — Result clamps apply the 32,000-character backstop with the 30,000 and 10,000 per-family clamps, and overflow is written to a file rather than dropped**
  - *Claim:* A shell result over 30,000 characters is clamped to 30,000 with a file pointer; a grep result over 10,000 is clamped to 10,000; an unclassified family clamps at 32,000.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(clamp::)'` — expect three family cases plus `overflow_written_to_file`, matching `05-tools-and-extensions.md` §Limits and `../letta-code/src/tools/impl/tool-return-clamp.ts`.
  - *Status:* ☐ unverified

- **O4 — A hook or mod failure is attributed to its owner and cannot leave a half-registered toolset**
  - *Claim:* An owner-thrown failure surfaces with the owner's identity and the registry is unchanged afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(pipeline::owner_attribution)'` — expect PASS; the test asserts the error names the owner and that the registry snapshot before and after are equal.
  - *Status:* ☐ unverified

- **O5 — `TOOL_INPUT_BYTES_MAX`, `TOOL_RESULT_BYTES_MAX`, and `CHILD_PROCESS_OUTPUT_BYTES_MAX` are named constants with below/at/above tests**
  - *Claim:* The three §Limits constants exist with the table's defaults and reject above the limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(limits::)'` — expect three cases per constant; compare defaults against the `05-tools-and-extensions.md` §Limits table.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(pipeline::) + test(clamp::) + test(limits::)'` and sees the ten-stage order, secret containment, three-tier clamping, owner attribution, and the byte bounds pass**
  - *Claim:* The pipeline, clamp, and limits modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `stage_order` case in the summary.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/registry.rs` (Task 32) supplies the definitions the pipeline reads; confirm `registry::atomic_swap` still passes with the pipeline consuming snapshots : ☐ (PRESERVED / REGRESSION)

## Residue

The permission and sandbox gate is a stub returning allow until Tasks 34–35 land; the pipeline order test pins the position now so those tasks cannot insert the gate elsewhere.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
