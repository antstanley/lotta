# Done Certificate — Task 07: Normalized provider port contract

**Task:** [07-provider_port_contract.md](07-provider_port_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 07. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 07) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the `ProviderRequest`/`ProviderEvent`/`ProviderError` contract exactly as `06-model-providers.md` specifies it, so no vendor type can reach the runtime.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not narrow the event or error enums: `06-model-providers.md` §Normalized provider port fixes both lists, and dropping `ReasoningDelta`/`RedactedReasoning` would break `03-runtime-and-turns.md` §Provider and tool loop, which persists reasoning projections.

## Obligations

- **O1 — `ProviderEvent` has exactly the ten variants of `06-model-providers.md` §Normalized provider port, including both reasoning variants and `ProviderMetadata`**
  - *Claim:* The enum is ten-variant with the spec's names, and reasoning deltas survive from adapter to runtime.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/ports/provider_event.rs` and list the variants — expect exactly `TextDelta`, `ReasoningDelta`, `RedactedReasoning`, `ToolCallStart`, `ToolCallArgumentsDelta`, `ToolCallEnd`, `Usage`, `ProviderMetadata`, `Stop`, `Error`. Run `cargo nextest run -p lotta-runtime -E 'test(ports::provider_event_variants)'` — expect PASS.
  - *Checks:* Resolve the tool-call event triple — confirm three distinct variants (`ToolCallStart`, `ToolCallArgumentsDelta`, `ToolCallEnd`) exist rather than a collapsed start/complete pair, because `06-model-providers.md` §Streaming invariants requires arguments to be validated only at tool-call end.
  - *Status:* ☐ unverified

- **O2 — `ProviderError` carries exactly the twelve stable kinds and no vendor error type crosses the port**
  - *Claim:* The error enum has twelve variants matching the spec list, and no adapter-specific type appears in the port signature.
  - *Evidence to collect:* Read the `ProviderError` definition and count variants — expect twelve with the spec's names. Grep `crates/lotta-runtime/src/ports/provider.rs` for `reqwest`, `serde_json::Error`, and any vendor SDK type — expect zero, per `architecture-principles.md` §Rust baseline (`no vendor error crosses a port`).
  - *Status:* ☐ unverified

- **O3 — `ModelDescriptor` is the `canonical-types.schema.json` shape (`handle`, `provider_id`, `available`, optional `context_window`, `model_settings`) and is not redefined in the port**
  - *Claim:* The port imports the domain `ModelDescriptor` rather than declaring its own struct.
  - *Evidence to collect:* Grep `crates/lotta-runtime/src/ports/` for `struct ModelDescriptor` — expect zero matches, and confirm a `use lotta_domain::…::ModelDescriptor` import instead. Run `cargo nextest run -p lotta-runtime -E 'test(ports::model_descriptor_is_domain_type)'` — expect PASS.
  - *Checks:* Resolve `ModelDescriptor` at its use site in the port — confirm it resolves to the `lotta-domain` entity validated against `$defs.ModelDescriptor`, not to a locally declared struct with `provider`/`model_id`/`display_name` fields.
  - *Status:* ☐ unverified

- **O4 — All eight `06-model-providers.md` §Streaming invariants are expressed as executable contract tests over the port**
  - *Claim:* There is one failing-if-violated test per invariant, runnable against any `ProviderPort` implementation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(ports::streaming_invariants)'` — expect eight passing cases named for the eight invariants. Read the cancellation case and confirm it asserts that events emitted after cancellation are suppressed rather than merely ignored downstream.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::provider)'` and sees the ten-variant event enum, twelve-kind error enum, domain `ModelDescriptor`, and eight streaming-invariant cases pass**
  - *Claim:* The provider-port test module passes with the invariant cases enumerated.
  - *Evidence to collect:* Run the filter and confirm the summary lists the eight streaming-invariant cases and zero failures.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/ports/mod.rs` re-exports the port set added in Task 06; confirm the Task 06 `ports::dependency_direction` test still passes after this module is added : ☐ (PRESERVED / REGRESSION)

## Residue

Retry policy is Task 48 (the runtime owns it, not the adapter). Concrete adapters are Tasks 49–51.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
