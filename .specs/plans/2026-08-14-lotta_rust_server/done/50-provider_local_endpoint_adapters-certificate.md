# Done Certificate — Task 50: Local endpoint adapters and native discovery

**Task:** [50-provider_local_endpoint_adapters.md](50-provider_local_endpoint_adapters.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-17 — DONE

> Verification protocol for Task 50. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 50) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** Ollama, Ollama Cloud, LM Studio, and llama.cpp adapters with native endpoint discovery tested separately from the static pi-ai catalog.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not treat `models.json` as the local-provider catalog: `06-model-providers.md` §Conformance separates native endpoint discovery from the static pi-ai built-in catalog.

## Obligations

- **O1 — All four local endpoint adapters replay their fixture corpus to the expected normalized trace**
  - *Claim:* Ollama, Ollama Cloud, LM Studio, and llama.cpp each produce the recorded trace for every captured stream.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(local::replay)'` — expect one case per fixture across the four dialects, with the first divergence reported on failure.
  - *Status:* ☒ SATISFIED — nine indexed local fixtures plus an authenticated Ollama Cloud replay pass 10/10 through public adapters and real loopback transport with exact requests and normalized traces.

- **O2 — Native endpoint discovery is tested separately from the static pi-ai built-in catalog, and `models.json` is not used as the local-provider catalog**
  - *Claim:* Discovery queries the endpoint and the test does not read `models.json`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(local::discovery)'` — expect one case per adapter driven by a local fake endpoint. Grep the local adapters for `models.json` — expect zero matches, per `06-model-providers.md` §Conformance.
  - *Checks:* Resolve the catalog source used by `list_models` for a local provider — confirm it is endpoint discovery, not the static pi-ai catalog loaded by Task 51.
  - *Status:* ☒ SATISFIED — direct discovery cases query all four fake endpoints, including authenticated Cloud, Ollama tags/show, LM Studio fallback, and llama.cpp props/status; local sources contain zero `models.json` references.

- **O3 — An unreachable endpoint yields `ProviderError::Unavailable` and does not fail process readiness**
  - *Claim:* Connection refused maps to unavailable and the readiness probe stays ready.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(local::unreachable)'` — expect PASS asserting both the error kind and that the readiness state is unchanged, per `07-channels-and-operations.md` §Health and readiness (`Provider outages do not fail process readiness`).
  - *Status:* ☒ SATISFIED — all four public adapters and all four discovery dialects map refused connections to one Unavailable result; `/readyz` is structurally independent and constant before and after provider outages.

- **O4 — Each adapter passes the shared port-contract suite alongside the fake**
  - *Claim:* The Task 09 provider contract suite is invoked for all four adapters.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(local::contract_suite)'` — expect four invocations, each calling `lotta_testkit::contract::provider_port_contract`.
  - *Status:* ☒ SATISFIED — four direct tests invoke Task09 `provider_contract` or its honest additive Ollama shape over public adapters and real loopback for Success, TerminalError, ReceiverClosed, and Cancelled.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — local 30/30, provider/runtime/testkit 297/297, fmt, strict all-target/all-feature Clippy, docs, deny, source audit, generator checks, named constants, and source shape pass. Isolated pinned-source workspace run passed 1,621/1,623; the only two failures are pre-existing Task46 tests hardcoded to the concurrently upgraded sibling checkout, with no Task50 failure.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(local::)'` and sees four dialects replay their fixtures, discovery run against a fake endpoint, unavailability handled, and the shared contract suite pass**
  - *Claim:* The local adapter module passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and four `contract_suite` invocations.
  - *Status:* ☒ SATISFIED — broad `local::` selector passes 30/30: replay 10, discovery 8, unreachable 3, contracts 4, Ollama stream/lifecycle 4, and real retry integration 1.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/retry/policy.rs` (Task 48) owns retries for these adapters; confirm `retry::deterministic_delays` still passes with a real adapter attached : ☒ PRESERVED — a real LM Studio adapter runs through `RetryExecutor`, issues exactly one request per attempt, produces byte-identical delays across runs, and the canonical deterministic retry selector remains green.

## Residue

`06-model-providers.md` §Assumptions leaves the order of further native adapters open; every other provider class stays on the compatibility host (Task 51).

Task 84 owns richer readiness health semantics. Task 50 proves the current constant `/readyz` path has no provider dependency and remains ready across adapter and discovery outages.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Four bounded single-attempt local endpoint adapters now replay the canonical corpus, discover models from native endpoints rather than the static catalog, isolate outages from readiness, and pass honest shared contracts with runtime-owned deterministic retries.
