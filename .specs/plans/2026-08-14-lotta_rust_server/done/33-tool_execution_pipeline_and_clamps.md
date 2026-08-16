# Task 33 — Tool execution pipeline, secret substitution, and result clamps

**Plan:** [plan.md](../plan.md) · **Certificate:** [33-tool_execution_pipeline_and_clamps-certificate.md](33-tool_execution_pipeline_and_clamps-certificate.md)

**Implements:** [05-tools-and-extensions.md §Execution pipeline](../../../05-tools-and-extensions.md#execution-pipeline) · [05-tools-and-extensions.md §Limits](../../../05-tools-and-extensions.md#limits)
**Depends on:** 32
**Produces:** the ordered hook→permission→sandbox→secret-substitution→executor→post-hook→scrub→clamp→persist→emit pipeline with the baseline result clamps
**Pointers:** `crates/lotta-tools/src/pipeline.rs`, `src/clamp.rs`, `src/scrub.rs`, `src/limits.rs`; reference: `../letta-code/src/tools/impl/tool-return-clamp.ts`, `../letta-code/src/tools/impl/truncation.ts`, `../letta-code/src/tools/impl/overflow.ts`, `../letta-code/src/tools/impl/validation.ts`

## Steps

- [x] Implement the pipeline stages in the exact order of the `05-tools-and-extensions.md` §Execution pipeline diagram
- [x] Validate name resolution and JSON Schema input before any policy or execution step
- [x] Substitute secrets only into the child environment or provider request, never into logs, results, or persisted records
- [x] Attribute hook and mod failure to its owner and guarantee it cannot leave a half-registered toolset
- [x] Clamp results at `TOOL_RESULT_MODEL_CHARS_MAX` (32,000 backstop) with the 30,000 shell/task/read and 10,000 grep per-family clamps, writing full overflow to a file
- [x] Enforce `TOOL_INPUT_BYTES_MAX`, `TOOL_RESULT_BYTES_MAX`, and `CHILD_PROCESS_OUTPUT_BYTES_MAX`

## Definition of done

- [x] The pipeline executes the ten stages in the §Execution pipeline order, and schema validation precedes every policy and execution step
- [x] Secrets are substituted only into child environments and provider requests, and never appear in a result, a log line, or a persisted record
- [x] Result clamps apply the 32,000-character backstop with the 30,000 and 10,000 per-family clamps, and overflow is written to a file rather than dropped
- [x] A hook or mod failure is attributed to its owner and cannot leave a half-registered toolset
- [x] `TOOL_INPUT_BYTES_MAX`, `TOOL_RESULT_BYTES_MAX`, and `CHILD_PROCESS_OUTPUT_BYTES_MAX` are named constants with below/at/above tests
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(pipeline::) + test(clamp::) + test(limits::)'` and sees the ten-stage order, secret containment, three-tier clamping, owner attribution, and the byte bounds pass
