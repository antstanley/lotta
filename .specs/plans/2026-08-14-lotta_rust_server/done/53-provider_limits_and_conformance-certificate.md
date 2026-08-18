# Done Certificate — Task 53: Provider limits and adapter conformance gate

**Task:** [53-provider_limits_and_conformance.md](53-provider_limits_and_conformance.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

> Verification protocol for Task 53. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 53) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** every `06-model-providers.md` limit enforced and the gate proving a native adapter and the compatibility host emit equivalent traces for the same fixture.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let a native adapter replace a host-served provider without this gate: `06-model-providers.md` §Conformance requires equivalent normalized traces on the same golden corpus first.

## Obligations

- **O1 — All ten §Limits constants exist with the table's names and defaults and have below/at/above tests**
  - *Claim:* `PROVIDERS_MAX`, `MODELS_PER_PROVIDER_MAX`, `PROVIDER_REQUEST_BYTES_MAX`, `PROVIDER_RESPONSE_EVENT_BYTES_MAX`, `TOOL_ARGUMENT_BYTES_MAX`, `PROVIDER_TIMEOUT_MS_DEFAULT`, `PROVIDER_RETRIES_MAX`, `PROVIDER_BACKOFF_MS_MAX`, `OAUTH_STATE_TTL_SECONDS`, and `IMAGE_BYTES_MAX` are defined with the spec defaults.
  - *Evidence to collect:* Read `crates/lotta-providers/src/limits.rs` and compare each against the §Limits table. Run `cargo nextest run -p lotta-providers -E 'test(limits::)'` — expect three cases per constant.
  - *Status:* ☑ SATISFIED — `limits::` passes ten named tests containing 30 below/at/above boundary checks. All ten canonical values match the spec and the named validators are tied to their production runtime boundaries.

- **O2 — The effective context window is the minimum of all four sources, and repeated overflow is terminal with measured/estimated details and no message content**
  - *Claim:* Given four differing values, the smallest wins; after `CONTEXT_OVERFLOW_COMPACTIONS_MAX` the turn stops with a detail payload containing no message text.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(context::effective_window) + test(context::repeated_overflow_terminal)'` — expect both PASS; the second asserts the detail payload has measured and estimated token counts and no content field.
  - *Status:* ☑ SATISFIED — `context::` 2/2 proves the four-source minimum and exactly three compaction decisions before a terminal detail carrying measured/estimated counts, limit, attempt, and provenance without content or secrets. The guard is called by native, host, and local adapters.

- **O3 — For every fixture, the native adapter and the compatibility host emit equivalent normalized traces**
  - *Claim:* Running both implementations over the same fixture yields traces equal under the Task 12 comparator.
  - *Evidence to collect:* Run `cargo nextest run --test provider_equivalence` — expect one case per fixture covered by both a native adapter and the host, each asserting trace equivalence and reporting the first divergence on failure.
  - *Checks:* Resolve which implementation each side of the comparison uses — confirm one is a `native::` adapter and the other the `host::` client, not the same implementation twice.
  - *Status:* ☑ SATISFIED — all seven indexed OpenAI-compatible/Anthropic overlap fixtures execute public native adapters and the real pinned pi-ai host against the same loopback transports. Task 12 comparison sees equivalent traces; neither side receives an expected trace.

- **O4 — All ten §Conformance contract dimensions are exercised by the gate, and the gate fails if a dimension has no case**
  - *Claim:* Request mapping, event order, tool-call assembly, usage, cancellation, errors, timeout, context overflow, retry-after, and image policy each have at least one equivalence case.
  - *Evidence to collect:* Run `cargo nextest run --test provider_equivalence -E 'test(dimension_coverage)'` — expect PASS; read the test and confirm it derives the dimension list from the fixture index so an uncovered dimension fails.
  - *Status:* ☑ SATISFIED — `provider_equivalence` 10/10 derives coverage from seven executed overlap cases plus real cancellation, timeout, context-overflow, and image-policy calls. Coverage exact-equals the canonical ten dimensions and fails if a case is removed.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — provider 165/165 and runtime/testkit 405/405 pass; format, full-workspace strict Clippy, rustdoc, and deny pass. Changed functions are ≤70 lines and changed lines ≤100 columns. Full-workspace failures are limited to pre-existing live sibling pin drift.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(limits::) + test(context::)'` and `cargo nextest run --test provider_equivalence` and sees the ten limits, the four-source context minimum, and native-versus-host trace equivalence pass**
  - *Claim:* Both suites pass with full dimension coverage.
  - *Evidence to collect:* Run both commands and confirm zero failures and a passing `dimension_coverage` case.
  - *Status:* ☑ SATISFIED — the named selector passes 12/12 and `provider_equivalence` passes 10/10 with first-divergence, request-mapping mutation, and cancellation-mutant checks.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-providers/src/native/` and `host/` (Tasks 49, 51) are compared here; native replay 7/7 and host replay 7/7 pass individually: ☑ PRESERVED

## Residue

The gate covers only dialects present in both implementations; dialects served solely by the host are covered by `host::replay` alone, which the gate logs explicitly.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 are satisfied. Canonical limits and context-overflow behavior are enforced at production boundaries, and the real native-versus-pi-host gate covers the exact overlap inventory and all ten conformance dimensions. Claude Fable/high session `4a3dcb5a-2259-4338-8e5b-251eda441f6f` returned `CORRECT / DONE`; the subsequent source-shape cleanup preserved all gates.
