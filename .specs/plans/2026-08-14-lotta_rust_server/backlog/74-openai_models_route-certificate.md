# Done Certificate — Task 74: OpenAI `/v1/models` and agent-name model resolution

**Task:** [74-openai_models_route.md](74-openai_models_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 74. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 74) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `GET /v1/models` listing up to 1,000 visible agents as OpenAI model objects, with agent-name and agent-ID resolution and `model_not_found` errors.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must use the shared listener authentication policy: `02-app-server-api.md` §Assumptions states HTTP and WebSocket share one listener and authentication policy.

## Obligations

- **O1 — `/v1/*` is registered only with `--openai-api`, while capability and health routes are always registered**
  - *Claim:* Without the flag, `/v1/models` returns 404 while `/healthz` and `/app-server-info` still respond.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::route_registration)'` — expect `v1_absent_without_flag` and `health_always_present`.
  - *Status:* ☐ unverified

- **O2 — A unique agent name is advertised as the model ID, a colliding name falls back to the agent ID, and raw agent IDs also resolve**
  - *Claim:* The three resolution cases behave as `02-app-server-api.md` §HTTP API specifies.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::model_resolution)'` — expect `unique_name_advertised`, `colliding_name_falls_back_to_id`, and `raw_agent_id_resolves`.
  - *Checks:* Resolve the collision check — confirm it considers all visible agents, not only the current page of the listing; a paged check would advertise a colliding name.
  - *Status:* ☐ unverified

- **O3 — The listing is capped at 1,000 visible agents and hidden agents are excluded**
  - *Claim:* With more than 1,000 visible agents the response contains 1,000, and hidden agents never appear.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::listing_cap)'` — expect the cap case and a `hidden_excluded` case.
  - *Status:* ☐ unverified

- **O4 — A missing model returns OpenAI `invalid_request_error` with code `model_not_found`, and the route uses the shared listener authentication policy**
  - *Claim:* The error envelope matches the baseline shape and an unauthenticated request is rejected by the same policy as `/ws`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::errors) + test(openai::auth_is_shared)'` — expect the error-shape case (compare with `../letta-code/src/websocket/app-server-openai.ts:146`) and an auth case asserting rejection without route-local credentials.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer starts the server with and without `--openai-api` and runs `curl /v1/models` and `curl /healthz`, seeing the models route present only with the flag, names advertised correctly, and `model_not_found` for a missing model**
  - *Claim:* The route behaves as specified from outside the process.
  - *Evidence to collect:* Run the two startups and the three curls; confirm 404 versus 200 on `/v1/models`, 200 on `/healthz` in both, and the `model_not_found` envelope for an unknown model.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/listener.rs` (Task 14) supplies the auth policy; confirm `auth::non_loopback_requires_auth` still passes with `/v1` registered : ☐ (PRESERVED / REGRESSION)

## Residue

Chat and Responses routes are Tasks 75 and 76.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
