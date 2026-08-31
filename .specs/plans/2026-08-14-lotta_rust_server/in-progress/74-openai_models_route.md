# Task 74 — OpenAI `/v1/models` and agent-name model resolution

**Plan:** [plan.md](../plan.md) · **Certificate:** [74-openai_models_route-certificate.md](74-openai_models_route-certificate.md)

**Implements:** [02-app-server-api.md §HTTP API](../../../02-app-server-api.md#http-api)
**Depends on:** 20, 23, 47
**Produces:** `GET /v1/models` listing up to 1,000 visible agents as OpenAI model objects, with visible agent-name and agent-ID resolution plus the exact pure/shared `model_not_found` contract for future model-taking routes
**Pointers:** `crates/lotta-app-server/src/openai/models.rs`, `openai/resolve.rs`, `openai/errors.rs`; reference: `../letta-code/src/websocket/app-server-openai.ts`, `../letta-code/src/websocket/app-server-openai-common.ts`

## Steps

- [ ] Register `/v1/*` routes only when `--openai-api` is set, keeping capability and health routes always registered
- [ ] List up to 1,000 visible agents as OpenAI model objects
- [ ] Advertise an agent's unique non-colliding name as its model ID, falling back to the agent ID when the name collides
- [ ] Resolve raw agent IDs in addition to advertised names
- [ ] Define the exact pure/shared OpenAI `invalid_request_error` contract with code `model_not_found` for a missing model; defer HTTP emission to Task 75's first model-taking route
- [ ] Apply the shared listener authentication policy rather than a route-local scheme

## Definition of done

- [ ] `/v1/*` is registered only with `--openai-api`, while capability and health routes are always registered
- [ ] A unique agent name is advertised as the model ID, a colliding name falls back to the agent ID, and raw agent IDs also resolve
- [ ] The listing is capped at 1,000 visible agents and hidden agents are excluded
- [ ] The pure/shared missing-model contract exactly composes OpenAI `invalid_request_error` with code `model_not_found`, while `GET /v1/models` uses the shared listener authentication policy; HTTP error emission is deferred to Task 75
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer starts the server with and without `--openai-api` and curls `/v1/models`, `/healthz`, and `/app-server-info`, verifying flag gating, health/info availability, and listing wire shape, names, collisions, and visibility; Task 75 owns the first external `model_not_found` curl
