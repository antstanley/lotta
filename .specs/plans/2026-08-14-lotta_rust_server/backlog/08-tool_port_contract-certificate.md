# Done Certificate — Task 08: Tool port contract and outcome records

**Task:** [08-tool_port_contract.md](08-tool_port_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 08. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 08) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the `ToolPort` trait and the tool definition/outcome records every executor, MCP server, controller tool, mod, and channel gateway implements.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not default any tool to parallel execution: `03-runtime-and-turns.md` §Assumptions leaves *Parallel tools* undecided, so the default must be the conservative one.

## Obligations

- **O1 — The tool definition record carries all ten fields named in `05-tools-and-extensions.md` §Tool registry, including parallel-safety classification and secret-bearing field redaction policy**
  - *Claim:* Each of the ten fields is present on the record type with a distinct type, not folded into a generic metadata map.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/ports/tool.rs` and check each field against the §Tool registry bullet list — expect ten matches. Run `cargo nextest run -p lotta-runtime -E 'test(ports::tool_definition_fields)'` — expect PASS.
  - *Status:* ☐ unverified

- **O2 — Parallel-safety defaults to sequential execution; no tool is classified parallel-safe without an explicit certified entry**
  - *Claim:* The default classification is `Sequential`, and constructing a definition without naming a classification yields `Sequential`.
  - *Evidence to collect:* Read the `Default` implementation for the parallel-safety type and confirm it returns the sequential variant. Run `cargo nextest run -p lotta-runtime -E 'test(ports::parallel_safety_defaults_sequential)'` — expect PASS.
  - *Checks:* Resolve the classification consulted by any future scheduler — confirm it reads this field rather than a hard-coded concurrency setting. `03-runtime-and-turns.md` §Assumptions records *Parallel tools* as an open question, so a concurrent default would decide it silently.
  - *Status:* ☐ unverified

- **O3 — `ToolOutcome` distinguishes all eight outcome classes of `05-tools-and-extensions.md` §Rust built-ins and each survives serialization as conversation data**
  - *Claim:* The enum has eight variants and each round-trips through serde without losing its discriminant.
  - *Evidence to collect:* Read the `ToolOutcome` definition and count variants — expect eight (success, user denial, interruption, timeout, validation failure, sandbox denial, spawn failure, tool-defined error). Run `cargo nextest run -p lotta-runtime -E 'test(ports::tool_outcome_round_trip)'` — expect eight passing cases.
  - *Checks:* Resolve the `timeout` variant used by tool results — confirm it is `ToolOutcome::Timeout`, not `ProviderError::Timeout` from Task 07. `NAME SHADOWING`: both crates expose a `timeout` concept and they must not be interchanged at the loop boundary.
  - *Status:* ☐ unverified

- **O4 — The execution-owner enum is exactly Rust, MCP, controller, mod sidecar, and channel gateway**
  - *Claim:* Five owner variants exist, matching §Tool registry, so an unowned tool is unrepresentable.
  - *Evidence to collect:* Read the owner enum and count variants — expect five with the spec's names. Run `cargo nextest run -p lotta-runtime -E 'test(ports::execution_owner)'` — expect PASS.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::tool)'` and sees the ten-field definition, sequential default, eight outcome variants, and five execution owners pass**
  - *Claim:* The tool-port test module passes with a case for each obligation above.
  - *Evidence to collect:* Run the filter and confirm zero failures and the presence of the `parallel_safety_defaults_sequential` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/ports/mod.rs` gains a module; confirm Task 06's `ports::dependency_direction` test still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Toolset IDs, model-facing name maps, and allowlist behavior are Task 32; the built-in executors are Tasks 37–40.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
