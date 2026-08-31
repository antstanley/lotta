# Task 76 — OpenAI `/v1/responses`, cursors, and hidden fork

**Plan:** [plan.md](../plan.md) · **Certificate:** [76-openai_responses_route-certificate.md](76-openai_responses_route-certificate.md)

**Implements:** [02-app-server-api.md §HTTP API](../../../02-app-server-api.md#http-api) · [01-domain-model.md §ID scheme](../../../01-domain-model.md#id-scheme)
**Depends on:** 71, 75
**Produces:** the Responses subset with unsigned `resp_letta_` cursors, `previous_response_id` hidden fork, and `501 unsupported_backend` when forking is unavailable
**Pointers:** `crates/lotta-app-server/src/openai/responses.rs`, `openai/cursor.rs`; reference: `../letta-code/src/websocket/app-server-openai-responses.ts`, `../letta-code/src/websocket/app-server-openai-common.ts`

## Steps

- [x] Support `input`, `instructions`, `previous_response_id`, `store`, and `stream`
- [x] On a successful `store: true` request, retain the conversation and return an unsigned `resp_letta_` base64url cursor carrying version, nonce, agent ID, and conversation ID
- [x] Otherwise return `resp_<uuid>` and delete a headerless ephemeral conversation, while an `X-Letta-Chat-Key` conversation remains stateful
- [x] Create a hidden fork for a valid `previous_response_id`, returning `501 unsupported_backend` when conversation forking is unavailable
- [x] Do not store failed outcomes and do not implement idempotency-key caching
- [x] Follow OpenAI event names for streaming and end in a completed or failed response state

## Definition of done

- [x] A successful `store: true` request retains its conversation and returns an unsigned `resp_letta_` base64url cursor carrying version, nonce, agent ID, and conversation ID
- [x] A non-stored request returns `resp_<uuid>`, deletes a headerless ephemeral conversation, and leaves an `X-Letta-Chat-Key` conversation stateful
- [x] A valid `previous_response_id` creates a hidden fork, and `501 unsupported_backend` is returned when forking is unavailable
- [x] Failed outcomes are not stored and no idempotency-key caching exists on this route
- [x] Streaming follows OpenAI event names and ends in a completed or failed response state
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer posts a `store: true` request, decodes the returned `resp_letta_` cursor, then posts a second request with `previous_response_id` set to it and observes a hidden fork continuing the conversation
