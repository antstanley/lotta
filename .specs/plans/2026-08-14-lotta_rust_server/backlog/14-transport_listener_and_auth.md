# Task 14 — Transport listener, capability-token and signed-bearer authentication

**Plan:** [plan.md](../plan.md) · **Certificate:** [14-transport_listener_and_auth-certificate.md](14-transport_listener_and_auth-certificate.md)

**Implements:** [02-app-server-api.md §Listener configuration](../../../02-app-server-api.md#listener-configuration) · [02-app-server-api.md §Responsibilities](../../../02-app-server-api.md#responsibilities)
**Depends on:** 05
**Produces:** an Axum listener that binds the baseline URL forms and enforces both `--ws-auth` modes, rejecting unauthenticated Origin-bearing clients even on loopback
**Pointers:** `crates/lotta-app-server/src/listener.rs`, `src/auth/capability_token.rs`, `src/auth/signed_bearer.rs`, `src/auth/origin.rs`; reference: `../letta-code/src/websocket/app-server-auth.ts`, `../letta-code/src/websocket/app-server.ts`, `../letta-code/src/cli/subcommands/listen.tsx`

## Steps

- [ ] Implement the listener flag surface of `02-app-server-api.md` §Listener configuration: `--listen`, `--openai-api`, `--ws-auth`, `--ws-token-file`, `--ws-token-sha256`, `--ws-shared-secret-file`, `--ws-issuer`, `--ws-audience`, `--ws-max-clock-skew-seconds`
- [ ] Bind `ws://127.0.0.1:0` for bare `--listen`; default the path to `/ws`, let a listen-URL path override it, and also accept `/` for upgrade compatibility
- [ ] Implement capability-token mode accepting a token file or a precomputed SHA-256 digest, never both, with constant-time comparison
- [ ] Implement signed-bearer mode verifying HS256, mandatory `exp`, optional `nbf`, configured issuer and audience, and clock skew bounded by `AUTH_CLOCK_SKEW_SECONDS_MAX`
- [ ] Fail before listening when a non-loopback bind has no authentication configured, and reject unauthenticated Origin-bearing requests even on loopback
- [ ] Read absolute secret files once at startup and keep their contents out of every log and error, and print the resolved base, WebSocket, and optional OpenAI URLs

## Definition of done

- [ ] Both `--ws-auth` modes work with the exact baseline flag surface, and capability-token mode rejects a configuration supplying both a token file and a digest
- [ ] Signed-bearer verification requires HS256 and `exp`, honours optional `nbf`, issuer, and audience, and rejects a configured skew above `AUTH_CLOCK_SKEW_SECONDS_MAX`
- [ ] A non-loopback bind without authentication fails before the socket listens, and an unauthenticated Origin-bearing request is rejected even on loopback
- [ ] URL resolution matches the baseline: bare `--listen` binds `ws://127.0.0.1:0`, `/ws` is the default path, a listen-URL path overrides it, and `/` is accepted for upgrade
- [ ] Secret files are read once at startup and never appear in a log line, an error message, or a debug rendering
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer starts the binary with `--listen ws://0.0.0.0:4500` and no `--ws-auth` and sees it exit before binding, then restarts with `--ws-auth capability-token --ws-token-file <path>` and sees an authenticated client connect while an unauthenticated Origin-bearing client is rejected

## Open questions

- Which browser origins, if any, are accepted alongside authenticated native clients (`02-app-server-api.md` §Open questions, *Origin policy*)? Until answered, every Origin-bearing client must authenticate.
