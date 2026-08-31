# Task 75 — OpenAI `/v1/chat/completions` statefulness and idempotency

**Plan:** [plan.md](../plan.md) · **Certificate:** [75-openai_chat_completions_route-certificate.md](75-openai_chat_completions_route-certificate.md)

**Implements:** [02-app-server-api.md §HTTP API](../../../02-app-server-api.md#http-api) · [02-app-server-api.md §Transport bounds](../../../02-app-server-api.md#transport-bounds)
**Depends on:** 15, 55, 74
**Produces:** stateful, header-keyed, and stateless chat completions with `Idempotency-Key` caching, in-flight sharing, and failed-outcome eviction
**Pointers:** `crates/lotta-app-server/src/openai/chat.rs`, `openai/chat_keys.rs`, `openai/idempotency.rs`; reference: `../letta-code/src/websocket/app-server-openai-common.ts`, `../letta-code/src/websocket/app-server-openai-turn.ts`, `../letta-code/src/websocket/app-server-openai.ts`

## Steps

- [ ] Require a non-empty model and messages containing usable user text or image content
- [ ] Map `X-Letta-Chat-Key` to one persisted conversation and accept `X-OpenWebUI-Chat-Id` for streaming requests
- [ ] Send only the newest user input for stateful requests; for headerless requests create an ephemeral conversation, replay user/assistant transcript content, and delete the conversation after settlement
- [ ] Keep the chat-key map in-memory FIFO at `OPENAI_CHAT_KEYS_MAX`, so eviction makes the next request for that key create a new conversation
- [ ] Honour `Idempotency-Key` and `X-Idempotency-Key`: check the cache before conversation allocation, share an in-flight turn, replay a successful settled outcome, and evict failed outcomes, bounded by `CHAT_IDEMPOTENCY_OUTCOMES_MAX`
- [ ] Serve both JSON and SSE responses

## Definition of done

- [ ] `X-Letta-Chat-Key` pins one persisted conversation, `X-OpenWebUI-Chat-Id` is accepted for streaming, and a headerless request uses an ephemeral conversation deleted after settlement
- [ ] Stateful requests send only the newest user input, while headerless requests replay user/assistant transcript content
- [ ] Idempotency caching checks before conversation allocation, shares an in-flight turn, replays a successful settled outcome, and evicts a failed one
- [ ] `OPENAI_CHAT_KEYS_MAX` and `CHAT_IDEMPOTENCY_OUTCOMES_MAX` evict FIFO, and an evicted chat key causes the next request to create a new conversation
- [ ] Both JSON and SSE responses are served, and a request with no usable user text or image content is rejected
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer sends two `curl` requests with the same `Idempotency-Key`, the second while the first is still streaming, and observes one shared turn and one conversation created, then repeats after settlement and observes a replayed outcome
