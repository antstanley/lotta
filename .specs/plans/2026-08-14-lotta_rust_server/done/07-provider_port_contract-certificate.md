# Task 07 · Provider port contract — final certification

**Task:** [07-provider_port_contract.md](07-provider_port_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Re-certified 2026-08-14 after narrow guideline fix — DONE

## Definition

DONE(Task 07) requires O1–O6 SATISFIED and regression PRESERVED. This verifier independently reviewed the complete current state, owning task/spec/schema, Task 06 regression requirements, all changed source and tests, and the provider contract references. It specifically re-certified the post-certification recursion, physical test-split, visibility, and manifest cleanup. Prior reports were used only as leads. No implementation or task text was modified.

## Obligations

### O1 — Exact normalized provider event enum

**Status: SATISFIED**

- `ProviderEvent` has exactly ten variants: `TextDelta`, `ReasoningDelta`, `RedactedReasoning`, `ToolCallStart`, `ToolCallArgumentsDelta`, `ToolCallEnd`, `Usage`, `ProviderMetadata`, `Stop`, and `Error`.
- Fields use bounded, stable runtime/domain values; tool argument chunks are bytes and admit arbitrary UTF-8/JSON splits. No vendor type crosses the enum.
- Exact selector `test(ports::provider_event_variants)` passed 1/1.

### O2 — Exact stable provider error enum and vendor isolation

**Status: SATISFIED**

- `ProviderError` has exactly twelve variants: authentication, authorization, invalid request, rate limit, quota, timeout, context overflow, overloaded, unavailable, protocol, cancelled, and unknown.
- Each carries the same normalized `ProviderErrorContext` with a stable code and bounded scrubbed context. No vendor SDK/error type or concrete provider dependency crosses the port.
- Broad structural selector passed the exact-twelve check.

### O3 — Canonical domain `ModelDescriptor`

**Status: SATISFIED**

- `ProviderRequest.model` is `lotta_domain::ModelDescriptor`; runtime ports do not redefine it.
- Canonical fields remain `handle`, `provider_id`, `available`, optional `context_window`, and optional `model_settings`.
- Exact selector passed 1/1; workspace schema conformance for the model passed.

### O4 — Eight reusable, violation-sensitive streaming invariants

**Status: SATISFIED**

- `SourceEvent` now has explicit source-owned variants for text, reasoning, redacted reasoning, tool start/arguments/end, usage, metadata, stop, and error. It has no `ProviderEvent` pass-through and no normalized-to-source conversion.
- Each of exactly eight invariant functions exercises `ProviderPort` through the generic concurrent `run_port`/`run_port_with_capacity` harness. Compliant source normalization and purpose-faulty behavior execute through the bounded asynchronous port; direct accumulator/channel checks supplement this matrix.
- Arrival order detects reverse, dropped reasoning, and collapsed redaction.
- Tool identity covers rewritten start/delta/end IDs, rewritten name, duplicate start, unknown end, and replay after end; valid and mismatched source IDs execute through the port.
- Argument buffering covers split UTF-8/JSON, merged/split chunks, malformed end, overflow, exact 4 MiB, over-bound rejection, and state cleanup. A fresh accumulator rejects 4 MiB + 1 while target capacity remains zero, proving the ceiling is checked before proportional reserve.
- Usage covers all four decreases, impossible components, checked-total overflow, and final snapshot retention.
- Terminal behavior covers legal stop and legal error, omitted/duplicate stop, post-stop and post-error output.
- Cancellation runs a real port with queued and late sends, plus blocked-send cancellation and receiver suppression.
- Image coverage runs compliant, dropped, and reordered mixed text/two-image request mappings through dedicated ports and directly verifies strict/drop/support behavior.
- Metadata coverage runs classified source metadata through dedicated compliant/faulty ports, rejects top-level and deeply nested object/array secret keys, discards explicitly secret values, retains continuation metadata, and verifies metadata/event/trace `Debug` redaction. `json_has_secret_key` is iterative and nonrecursive; its worklist traverses only the already validated bounded JSON tree and the deep-nesting case passes without panic.
- Exact invariant selector passed exactly 8/8.

### O5 — Repository DoD, bounded transport usability, and hard limits

**Status: SATISFIED**

- `ProviderRequest` includes canonical model/settings, system prompt, ordered messages/content/images, tools/tool choice, image policy, context/output limits, reasoning controls, cancellation, and deadline.
- Compact-wire accounting uses borrowed serialization into an O(1) checked writer. It includes nested model settings, messages, escaped strings, image byte-array encoding, schemas, tool choice, limits, reasoning, and deadline; equality succeeds, over-bound and arithmetic overflow reject, and production measurement allocates no wire-sized output.
- Tool argument deltas are raw bounded bytes; parsing occurs only at end. Byte deltas are preserved, malformed JSON is rejected at end, completed values are returned rather than retained, and protocol/limit/stop/error paths clear active/seen/ended state.
- Opaque sink/receiver halves expose awaitable `send`/`receive`, bounded backpressure, cancellation-biased race suppression, queued-event draining, and clean closure. There is no public raw sender/receiver, `try_send`, or unbounded bypass; internal `try_recv` is used only to drain suppressed events after terminal cancellation.
- Seven provider `ResourceBound` rows have exact stable order/metadata, Reject behavior, units-last names, canonical 32/8/4 MiB values, and exact cardinalities. Below/at/over observability passes for all seven. Owning request, event text, argument, channel, message, content, and tool constructors/checks cover applicable boundaries.
- Task 06 retains exactly 19 runtime bounds.
- Public Task 07 APIs have substantive ownership, limits, errors, cancellation, and secrecy documentation. Source audit found zero production missing-doc or missing-errors blanket suppressions.
- Production audit found no panic/unwrap/expect/unsafe/direct clock/unbounded channel/vendor SDK/credential/retry/adapter scope, no Task 08 implementation, and no raw bound leak or unnecessary concrete dependency.
- Formatting, denied-warning Clippy, denied-warning rustdoc, deny, dependency trees, and full workspace all passed. Provider tests are physically split into `provider_tests.rs` (11 lines), `harness.rs` (761), and `streaming_invariants.rs` (544), use no `include!`, and keep helpers at `pub(crate)`/`pub(super)` only where selector forwarding or sibling access requires it. Every Rust file is at most 1,000 lines, every line is at most 100 columns, and provider production functions are at most 70 lines.

### O6 — Reviewable selectors

**Status: SATISFIED**

- Broad `test(ports::provider)` is nonzero and passed 16/16 (at least twelve), including structural checks and the same eight named invariant functions.
- Event selector passed 1/1; model selector passed 1/1; invariant selector passed exactly 8/8.
- Exact seven-bound, async channel, Task 06 dependency, and Task 06 nineteen-bound checks passed.

## Regression check

**PRESERVED.** Task 06 dependency direction passed 1/1 and exact nineteen-row bounds passed 1/1. Full workspace passed 123/123. Runtime normal dependencies remain domain, serde/serde_json, thiserror, Tokio, and tokio-util; the empty dev-dependency table is gone without dependency loss. No provider adapter edge, Task 08 surface, retry/fallback, credentials, or turn-loop implementation was introduced.

## Verification evidence

- Selectors: provider 16/16; event 1/1; model 1/1; invariants exactly 8/8; provider bounds 1/1; Task 06 bounds 1/1; Task 06 dependency 1/1.
- Full workspace: 123/123.
- `cargo fmt --all --check`: PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`: PASS.
- `cargo deny check`: PASS.
- `cargo tree -p lotta-runtime -e normal` and workspace duplicate tree audited; no forbidden adapter dependency or unnecessary production addition found.
- Source/allocation/secrecy/channel/model/enum/fault-matrix/vendor/forbidden/hard-limit audits completed. Audits found zero docs suppressions, source smuggling, recursion, vendor types, public raw channel halves, forbidden unsafe/direct-clock/unbounded-channel use, over-100-column lines, over-1,000-line files, or over-70-line provider production functions. External implementability follows from the public object-safe trait, public `PortFuture`, public opaque channel constructor/halves, and normal awaitable public methods; the in-crate object-safety/opacity checks pass.

## Conclusion

- **Correctness: CORRECT**
- **Completeness: DONE**
- **Confidence: high**

O1–O6 remain SATISFIED and regression is PRESERVED after the narrow post-certification guideline fix. The secret-key walk is iterative over bounded JSON, provider tests are physically split with narrow helper visibility and unchanged exact selectors, and the empty dev-dependency table was removed without dependency regression. Prior 10/12/request/channel/source-matrix/bounds/docs guarantees remain proven.
