# Done Certificate — Task 12: Provider stream fixture corpus extraction

**Task:** [12-fixtures_provider_stream_corpus.md](12-fixtures_provider_stream_corpus.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 12. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 12) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a sanitized `fixtures/providers/` corpus of captured baseline streams plus their expected normalized `ProviderEvent` traces, checked in before any adapter.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must land before Tasks 49–51: `06-model-providers.md` §Conformance requires replaying captured sanitized baseline streams into each Rust adapter, and a native adapter may only replace a compatibility-host provider once both produce equivalent traces on this corpus.

## Obligations

- **O1 — Each of the five dialects has at least one captured raw stream and a matching expected normalized trace**
  - *Claim:* `fixtures/providers/<dialect>/` exists for all five dialects, each with a raw capture and an expected-trace file.
  - *Evidence to collect:* List `fixtures/providers/` and confirm five dialect directories. Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::index_is_complete)'` — expect PASS with a per-dialect case.
  - *Status:* ☐ unverified

- **O2 — The corpus covers every `06-model-providers.md` §Conformance dimension including cancellation, context overflow, retry-after, and image policy**
  - *Claim:* There is at least one fixture tagged for each of the ten named contract dimensions.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::dimensions_are_covered)'` — expect one passing case per dimension, reading the dimension list from the fixture index rather than a hard-coded array.
  - *Status:* ☐ unverified

- **O3 — Reasoning and redacted-reasoning streams are present, so an adapter dropping them fails replay**
  - *Claim:* At least one fixture emits `ReasoningDelta` and one emits `RedactedReasoning` in its expected trace.
  - *Evidence to collect:* Grep the expected-trace files for `ReasoningDelta` and `RedactedReasoning` — expect at least one occurrence each. Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::reasoning_present)'` — expect PASS.
  - *Checks:* Resolve the event names in the expected traces against the Task 07 `ProviderEvent` variants — confirm the trace vocabulary is the port enum, not a vendor event name.
  - *Status:* ☐ unverified

- **O4 — The replay helper drives an arbitrary `ProviderPort` and reports the first diverging event with its index**
  - *Claim:* Replaying a deliberately wrong implementation fails with a message naming the event index and both events.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::replay_reports_first_divergence)'` — expect PASS, and read the assertion message format to confirm it names the index, the expected event, and the actual event.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::)'` and sees five dialects, ten covered dimensions, reasoning coverage, and divergence reporting pass**
  - *Claim:* The provider-fixture module passes with per-dialect and per-dimension cases.
  - *Evidence to collect:* Run the filter and confirm zero failures and at least fifteen cases.
  - *Status:* ☐ unverified

## Regression check

- Task 07 provider events and Task 09 fixture indexing decode every expected trace; run `cargo nextest run -p lotta-testkit -E 'test(contract::provider) | test(fixtures::providers)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

Live provider calls are out of scope for CI; `development-guidelines.md` §Testing forbids depending on public provider availability, so the corpus is the only provider evidence until Task 53.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
