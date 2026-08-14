# Task 08 — Tool port contract and outcome records

**Plan:** [plan.md](../plan.md) · **Certificate:** [08-tool_port_contract-certificate.md](08-tool_port_contract-certificate.md)

**Implements:** [05-tools-and-extensions.md §Tool registry](../../../05-tools-and-extensions.md#tool-registry) · [05-tools-and-extensions.md §Responsibilities](../../../05-tools-and-extensions.md#responsibilities) · [architecture-principles.md §What goes where](../../../architecture-principles.md#what-goes-where)
**Depends on:** 06
**Produces:** the `ToolPort` trait and the tool definition/outcome records every executor, MCP server, controller tool, mod, and channel gateway implements
**Pointers:** `crates/lotta-runtime/src/ports/tool.rs`, `crates/lotta-runtime/src/ports/tool_outcome.rs`; reference: `../letta-code/src/tools/define-tool.ts`, `../letta-code/src/tools/model-facing-tool.ts`, `../letta-code/src/tools/impl/tool-return-clamp.ts`

## Steps

- [x] Define the tool definition record with the eleven typed fields grouped by `05-tools-and-extensions.md` §Tool registry: internal stable name, model-facing name per toolset, JSON Schema input, description asset, execution owner, approval policy, permission action, parallel-safety classification, timeout, output limit, and one secret-redaction specification combining the secret-bearing field set with its policy
- [x] Define the execution-owner enum as exactly Rust, MCP, controller, mod sidecar, and channel gateway
- [x] Define `ToolOutcome` distinguishing success, user denial, interruption, timeout, validation failure, sandbox denial, spawn failure, and tool-defined error
- [x] Define `ToolPort` with an execute method taking validated input, a cancellation token, and a deadline
- [x] Default the parallel-safety classification to sequential so no tool executes concurrently until a certified-safe set is defined
- [x] Add contract tests asserting each outcome variant survives serialization as conversation data

## Definition of done

- [x] The tool definition record carries all eleven typed fields grouped by `05-tools-and-extensions.md` §Tool registry, including parallel-safety classification and one secret-redaction specification containing the field set and policy
- [x] Parallel-safety defaults to sequential execution; no tool is classified parallel-safe without an explicit certified entry
- [x] `ToolOutcome` distinguishes all eight outcome classes of `05-tools-and-extensions.md` §Rust built-ins and each survives serialization as conversation data
- [x] The execution-owner enum is exactly Rust, MCP, controller, mod sidecar, and channel gateway
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::tool)'` and sees the eleven-field definition, sequential default, eight outcome variants, and five execution owners pass
