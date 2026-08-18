# Task 52 — Provider connections, `auth.json` v1, and forced disconnect

**Plan:** [plan.md](../plan.md) · **Certificate:** [52-provider_connections_and_credentials-certificate.md](52-provider_connections_and_credentials-certificate.md)

**Implements:** [06-model-providers.md §Connections and credentials](../../../06-model-providers.md#connections-and-credentials) · [01-domain-model.md §Provider connection](../../../01-domain-model.md#provider-connection)
**Depends on:** 22, 27, 47
**Produces:** baseline-compatible plaintext `providers/auth.json` v1 with restrictive modes, redaction, and disconnect that refuses while turns are active unless forced
**Pointers:** `crates/lotta-providers/src/connections/store.rs`, `connections/connect.rs`, `connections/disconnect.rs`; reference: `../letta-code/src/backend/local/local-provider-auth-store.ts`, `../letta-code/src/providers/provider-connections.ts`, `../letta-code/src/providers/connect-provider-service.ts`

## Steps

- [x] Read and write `providers/auth.json` version 1 as the baseline provider record map, with configuration and API/OAuth secrets together in plaintext JSON
- [x] Enforce directory mode `0700` and file mode `0600` and route every write through the Task 22 atomic writer
- [x] Expose declarative connect/disconnect fields and auth methods and pass secrets to adapters without copying them into diagnostics
- [x] Refuse disconnect while active turns use the connection unless `force` is explicit; a forced disconnect cancels affected turns first
- [x] Enforce `PROVIDERS_MAX` as the provider-connection cap, distinct from the WebSocket `CONNECTIONS_MAX`
- [x] Round-trip the `fixtures/persistence/` `auth.json` fixture in both directions

## Definition of done

- [x] `providers/auth.json` v1 round-trips against the fixture with configuration and secrets together in plaintext, at modes `0700`/`0600`
- [x] Secrets reach adapters but never diagnostics: no captured log, error, or App Server snapshot contains a credential
- [x] Disconnect refuses while active turns use the connection unless `force` is explicit, and a forced disconnect cancels affected turns first
- [x] `PROVIDERS_MAX` bounds provider connections and is distinct from the WebSocket `CONNECTIONS_MAX`
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(connections::)'` and sees the plaintext v1 round trip at `0600`, three redaction cases, forced-disconnect ordering, and the 128 provider cap pass
