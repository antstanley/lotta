# Done Certificate — Task 49: Native OpenAI-compatible and Anthropic adapters

**Task:** [49-provider_native_api_adapters.md](49-provider_native_api_adapters.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 49. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 49) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** two native adapters whose normalized traces match the captured baseline fixtures event for event.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not collapse reasoning into text: `06-model-providers.md` §Streaming invariants preserves text and reasoning order per stream, and `03-runtime-and-turns.md` §Provider and tool loop persists reasoning projections.

## Obligations

- **O1 — Both adapters replay their `fixtures/providers/` corpus and produce the expected normalized trace event for event**
  - *Claim:* Every captured stream for the two dialects yields the recorded expected trace under the Task 12 comparator.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(native::replay)'` — expect one passing case per fixture in the two dialect directories, with the first divergence reported on failure.
  - *Status:* ☐ unverified

- **O2 — Vendor errors map to the twelve `ProviderError` kinds and no vendor type crosses the port**
  - *Claim:* Each error fixture maps to its expected kind, and the adapter's public signature exposes no vendor error type.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(native::error_mapping)'` — expect one case per error fixture. Grep the adapter's public API for `reqwest::Error` — expect zero, per `architecture-principles.md` §Rust baseline.
  - *Checks:* Resolve the error constructed on an HTTP 429 — confirm it is `ProviderError::RateLimit`, and on 402/quota exhaustion `ProviderError::Quota`; the two are distinct kinds in `06-model-providers.md` §Normalized provider port.
  - *Status:* ☐ unverified

- **O3 — Reasoning and redacted-reasoning events survive translation where the vendor emits them**
  - *Claim:* The reasoning fixtures produce `ReasoningDelta` and `RedactedReasoning` events rather than being folded into text.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(native::reasoning)'` — expect both cases, asserting the event kind rather than only the text content.
  - *Status:* ☐ unverified

- **O4 — The strict/drop image policy is applied and `PROVIDER_REQUEST_BYTES_MAX`/`PROVIDER_RESPONSE_EVENT_BYTES_MAX` are enforced**
  - *Claim:* Under strict policy an unsupported image fails the request; under drop policy it is elided; both byte bounds reject above the limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(native::image_policy) + test(native::byte_bounds)'` — expect the two policy cases and below/at/above cases for both bounds from `06-model-providers.md` §Limits.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(native::)'` and sees every OpenAI-compatible and Anthropic fixture replay to its expected trace with correct error kinds and image policy**
  - *Claim:* Both native adapters pass the fixture corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and a replay case count equal to the number of fixtures in the two dialect directories.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-testkit/src/contract/` provider suite (Task 09) must pass for these adapters as well as the fake; confirm both invocations pass : ☐ (PRESERVED / REGRESSION)

## Residue

OpenAI Codex/ChatGPT OAuth stays on the compatibility host until its OAuth and request dialect pass fixtures (`06-model-providers.md` §Provider classes); it is Task 51's scope.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
