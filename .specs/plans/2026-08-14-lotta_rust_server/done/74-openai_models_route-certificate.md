
**Task:** [74-openai_models_route.md](74-openai_models_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-31 — implementation commit `92c30e4a0e76`

> Verification protocol for Task 74. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 74) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `GET /v1/models` listing up to 1,000 visible agents as OpenAI model objects, with visible agent-name and agent-ID resolution plus the exact pure/shared `model_not_found` contract consumed by future model-taking routes.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must use the shared listener authentication policy: `02-app-server-api.md` §Assumptions states HTTP and WebSocket share one listener and authentication policy.

## Obligations

- **O1 — `/v1/*` is registered only with `--openai-api`, while capability and health routes are always registered**
  - *Claim:* Without the flag, `/v1/models` returns 404 while `/healthz` and `/app-server-info` still respond.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::route_registration)'` — expect `v1_absent_without_flag` and `health_always_present`.
  - *Evidence:* The route-registration selector passed 3/3 at implementation commit `92c30e4a0e76`: `/v1/models` was absent without the flag and present with it, while `/healthz` and `/app-server-info` remained available in both modes.
  - *Status:* **SATISFIED**

- **O2 — A unique agent name is advertised as the model ID, a colliding name falls back to the agent ID, and raw agent IDs also resolve**
  - *Claim:* The three resolution cases behave as `02-app-server-api.md` §HTTP API specifies.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::model_resolution)'` — expect `unique_name_advertised`, `colliding_name_falls_back_to_id`, and `raw_agent_id_resolves`.
  - *Checks:* Resolve the collision check — confirm it considers all visible agents, not only the current page of the listing; a paged check would advertise a colliding name.
  - *Evidence:* The model-resolution selector passed 7/7, covering unique-name advertisement, collision fallback, raw-ID precedence and resolution, hidden ID/name misses, and visible-only ambiguity. The listing-cap proof additionally places the second duplicate outside the first 1,000 rows and confirms collision detection uses the complete visible set.
  - *Status:* **SATISFIED**

- **O3 — The listing is capped at 1,000 visible agents and hidden agents are excluded**
  - *Claim:* With more than 1,000 visible agents the response contains 1,000, and hidden agents never appear.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::listing_cap)'` — expect the cap case and a `hidden_excluded` case.
  - *Evidence:* The listing-cap selector passed 2/2: 1,001 visible agents yielded exactly 1,000 model objects with complete-set collision handling, and the hidden-agent case emitted only the visible agent. The focused OpenAI suite also proved below/at/above cap behavior.
  - *Status:* **SATISFIED**

- **O4 — The pure/shared missing-model contract exactly composes OpenAI `invalid_request_error` with code `model_not_found`, while `GET /v1/models` uses the shared listener authentication policy; HTTP error emission is deferred to Task 75**
  - *Claim:* Resolver misses compose the complete, untruncated model value into the exact baseline envelope, and unauthenticated model listing is rejected by the same policy as `/ws`; Task 74 adds no model-taking HTTP route.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::errors) + test(openai::model_resolution) + test(openai::auth_is_shared)'` — expect exact basic, Unicode, and long-value JSON contract cases; hidden ID/name misses and visible-only ambiguity cases; and shared-auth rejection. Trace `resolve::resolve` calling `errors::model_not_found`; reserve response conversion and the first external missing-model curl for Task 75.
  - *Evidence:* The combined selector passed 13/13: three exact error-envelope cases preserve basic, Unicode, and long model values; seven resolver cases prove all misses compose through `errors::model_not_found`; and three auth cases prove `/v1/models`, `/app-server-info`, and `/ws` share bearer-token, origin, and non-loopback policy while `/healthz` remains available. Source review confirms Task 74 adds no model-taking HTTP route or HTTP error conversion/emission; Task 75 owns both that emission and the first external missing-model curl.
  - *Status:* **SATISFIED**

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Fresh workspace check, strict workspace Clippy, format check, rustdoc, and deny all passed. The app-server suite passed 532/532. The focused OpenAI evidence passed 27/27, including the exact enabled OpenAI URL selector. Source review confirmed named constants with units last, bounded functions, and the repository's 100-column formatting requirement. The model projection's RFC3339 behavior is exact: `2026-08-14T12:34:56Z` and the equivalent `2026-08-14T14:34:56+02:00` both serialize as Unix second `1786710896`; `1969-12-31T23:59:59.500Z` floors to `-1`; absent or invalid `created_at` serializes as `0`.
  - *Status:* **SATISFIED**

- **O6 — Reviewable: a reviewer starts the server with and without `--openai-api` and curls `/v1/models`, `/healthz`, and `/app-server-info`, verifying flag gating, health/info availability, and listing wire shape, names, collisions, and visibility; Task 75 owns the first external `model_not_found` curl**
  - *Claim:* Task 74's only OpenAI HTTP route behaves as specified from outside the process, without claiming HTTP behavior for a model-taking route that does not yet exist.
  - *Evidence to collect:* Run both startups and curl `/v1/models`, `/healthz`, and `/app-server-info`: confirm 404 versus 200 for listing under flag off/on, 200 for health and info in both modes, `application/json` plus exact model-list wire fields, unique-name advertisement, collision fallback, and hidden-agent exclusion. Do not attempt or certify an external missing-model response until Task 75.
  - *Evidence:* Real-listener endpoint tests passed route registration 3/3, listing cap/visibility 2/2, shared auth 3/3, and the focused OpenAI suite 27/27 including exact URL publication. They verify flag-off 404 versus flag-on 200, always-present health/info endpoints, `application/json`, exact list/model wire fields, names, complete-set collision fallback, raw IDs, visibility, RFC3339-derived `created` seconds, and `owned_by: "letta"`. No external missing-model response is claimed: Task 75 owns the first model-taking HTTP emission and curl. A clean independent GPT-5.6 Sol review of final commit `92c30e4a0e76` returned `CORRECT / DONE` and confirmed O1–O6.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/listener.rs` (Task 14) supplies the auth policy; `auth::non_loopback_requires_auth` remains covered with `/v1` registered, shared-auth coverage passed 3/3, and the full app-server suite passed 532/532 — **PRESERVED**.

## Residue

Chat and Responses routes are Tasks 75 and 76. Task 75's first model-taking route owns request-parser size bounds, OpenAI HTTP error conversion/emission, and the first external `model_not_found` curl; Task 74 proves only the exact pure/shared resolver-to-error contract.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-31 at implementation commit `92c30e4a0e76`. All O1–O6 are satisfied: route registration passed 3/3, model resolution 7/7, listing cap/visibility 2/2, and the combined missing-model/resolution/shared-auth proof 13/13. Focused OpenAI coverage passed 27/27 including exact URL publication, and the app-server suite passed 532/532. Workspace check, strict Clippy, format, rustdoc, and deny gates are green. Exact RFC3339-to-Unix-second behavior, complete-visible-set collision handling, and shared listener authentication are proved; Task 75 correctly retains ownership of model-taking HTTP error emission and its first external curl. The Task 14 auth regression is preserved, and a clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
