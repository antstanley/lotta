# Task 15 — Transport bounds, error envelopes, and closure

**Plan:** [plan.md](../plan.md) · **Certificate:** [15-transport_bounds_and_closure-certificate.md](15-transport_bounds_and_closure-certificate.md)

**Implements:** [02-app-server-api.md §Transport bounds](../../../02-app-server-api.md#transport-bounds) · [02-app-server-api.md §Errors and closure](../../../02-app-server-api.md#errors-and-closure)
**Depends on:** 05, 14
**Produces:** the nine transport bounds enforced before proportional work, plus stable HTTP/WebSocket error envelopes and half-open reaping
**Pointers:** `crates/lotta-app-server/src/bounds.rs`, `src/errors.rs`, `src/heartbeat.rs`; reference: `../letta-code/src/websocket/listener/heartbeat.ts`, `../letta-code/src/websocket/listener/listener-constants.ts`, `../letta-code/src/websocket/app-server.ts`

## Steps

- [x] Define the nine `02-app-server-api.md` §Transport bounds constants with their exact names and defaults
- [x] Reject an oversized HTTP body before JSON decode and close an oversized WebSocket frame with code 1009
- [x] Reject a decode whose field count exceeds `WS_MESSAGE_FIELDS_MAX` before doing field-proportional work, and reject a `request_id` longer than `REQUEST_ID_BYTES_MAX`
- [x] Implement `WS_PING_INTERVAL_MS` pings and `WS_PONG_TIMEOUT_MS` half-open termination
- [x] Emit stable HTTP status plus JSON error envelopes, pre-upgrade WebSocket errors as HTTP 400/401/403/503, and correlate protocol errors to `request_id` where present
- [x] Scrub internal errors before return and attach a generated incident ID to the diagnostic record only

## Definition of done

- [x] All nine §Transport bounds constants exist with the spec's names and defaults and have below/at/above tests
- [x] Oversized input is rejected before proportional work: the body cap fires before JSON decode, the frame cap closes with 1009, and the field cap fires before field-proportional work
- [x] Half-open sockets are reaped: a client that stops answering pings is terminated after `WS_PONG_TIMEOUT_MS`
- [x] Errors use stable envelopes: HTTP status plus JSON body, pre-upgrade WebSocket failures as HTTP 400/401/403/503, protocol errors correlated to `request_id`, and internal errors scrubbed with an incident ID retained only in diagnostics
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(bounds::) + test(errors::) + test(heartbeat::)'` and sees below/at/above coverage for all nine bounds, the four error classes, and half-open reaping pass
