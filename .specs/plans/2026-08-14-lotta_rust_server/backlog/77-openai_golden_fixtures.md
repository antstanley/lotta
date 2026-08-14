# Task 77 — OpenAI golden request, response, and SSE fixtures

**Plan:** [plan.md](../plan.md) · **Certificate:** [77-openai_golden_fixtures-certificate.md](77-openai_golden_fixtures-certificate.md)

**Implements:** [02-app-server-api.md §HTTP API](../../../02-app-server-api.md#http-api) · [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition)
**Depends on:** 13, 74, 75, 76
**Produces:** checked-in golden fixtures for all three routes, replayed against the running server with event-by-event SSE comparison
**Pointers:** `fixtures/openai/`, `tests/conformance/openai_golden.rs`, `tools/capture-openai-fixtures.mjs`; reference: `../letta-code/src/websocket/app-server-openai.test.ts`, `app-server-openai-responses.test.ts`, `app-server-openai-sdk.test.ts`

## Steps

- [ ] Capture golden request/response pairs for `/v1/models`, `/v1/chat/completions`, and `/v1/responses` from the pinned baseline
- [ ] Capture SSE streams for the streaming variants of both POST routes
- [ ] Cover stateful (chat-key), headerless, and idempotent-retry cases for chat completions
- [ ] Cover stored, non-stored, and `previous_response_id` cases for responses
- [ ] Replay each golden request against the running Rust server and compare event by event
- [ ] Sanitize every fixture of credentials and user content

## Definition of done

- [ ] Golden fixtures exist for all three routes covering JSON and SSE, and the index names every covered case
- [ ] Replaying each golden request against the running server reproduces the recorded response, with SSE compared event by event
- [ ] Chat-completion cases cover stateful, headerless, and idempotent-retry paths; response cases cover stored, non-stored, and `previous_response_id`
- [ ] No fixture contains a credential or user content
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test openai_golden` against a running server with `--openai-api` and sees every golden fixture match, including event-by-event SSE comparison
