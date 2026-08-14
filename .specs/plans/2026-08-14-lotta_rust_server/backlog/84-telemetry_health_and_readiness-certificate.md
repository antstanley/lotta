# Done Certificate — Task 84: Telemetry, security logging, health, and readiness

**Task:** [84-telemetry_health_and_readiness.md](84-telemetry_health_and_readiness.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 84. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 84) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** structured JSON daemon logs with the required fields and centralized scrubbing, plus the four health, readiness, capability, and metrics endpoints.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not make readiness depend on provider health: `07-channels-and-operations.md` §Assumptions *Readiness* keeps readiness to storage and auth so a model outage cannot restart a healthy state server.

## Obligations

- **O1 — Daemon logs are structured JSON carrying all eight required fields where applicable, with user content and secrets excluded**
  - *Claim:* A captured log stream contains the eight fields and none of the planted content or secret markers.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-telemetry -E 'test(logging::fields) + test(logging::exclusions)'` — expect the field assertion and marker-absence assertions for a prompt, a message body, a tool input, and a credential.
  - *Status:* ☐ unverified

- **O2 — `/healthz` is liveness only while `/readyz` requires state validation, loaded auth, and an accepting runtime registry**
  - *Claim:* With storage validation incomplete, `/healthz` returns healthy and `/readyz` returns not ready.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(health::probes)'` — expect `healthz_is_liveness_only` and `readyz_requires_all_three` with a case per readiness input.
  - *Checks:* Resolve the readiness inputs — confirm provider status is not among them. `07-channels-and-operations.md` §Health and readiness states provider outages do not fail process readiness.
  - *Status:* ☐ unverified

- **O3 — `/ws` and the OpenAI routes are not ready until storage validation completes, and a provider outage never changes readiness**
  - *Claim:* Before validation completes, upgrades and `/v1` requests are refused; a provider outage leaves readiness unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(health::gating)'` — expect `ws_gated_until_validated`, `openai_gated_until_validated`, and `provider_outage_does_not_change_readiness`.
  - *Status:* ☐ unverified

- **O4 — `/app-server-info` requires authentication and matches the WebSocket capability snapshot; `/metrics` is served only when enabled and separately authenticated or bound**
  - *Claim:* The HTTP capability response equals the WebSocket one, and `/metrics` is absent unless enabled.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(health::info_and_metrics)'` — expect `info_requires_auth`, `info_matches_ws_snapshot`, and `metrics_absent_unless_enabled`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer starts the server, curls `/healthz` and `/readyz` before and after storage validation, and curls `/app-server-info` with and without credentials, seeing liveness always healthy, readiness gated, and the capability route authenticated**
  - *Claim:* The four endpoints behave as specified from outside the process.
  - *Evidence to collect:* Run the four curls and confirm 200/503 on `/readyz` around validation, 200 on `/healthz` throughout, and 401 versus 200 on `/app-server-info`.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/` validation (Task 22) gates readiness; confirm `atomic::` still passes with the readiness probe attached : ☐ (PRESERVED / REGRESSION)

## Residue

Runtime-scoped events and metric families are Task 59; this task owns process-level logging and the HTTP probes.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
