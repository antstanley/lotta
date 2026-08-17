# Task 51 — pi-ai compatibility provider host

**Plan:** [plan.md](../plan.md) · **Certificate:** [51-provider_compatibility_host-certificate.md](51-provider_compatibility_host-certificate.md)

**Implements:** [06-model-providers.md §Provider classes](../../../06-model-providers.md#provider-classes)
**Depends on:** 12, 42, 48
**Produces:** a compatibility host pinning resolved `@earendil-works/pi-ai` `0.82.1` over the shared sidecar, serving the full built-in catalog, Codex/ChatGPT OAuth, and mod-defined providers
**Pointers:** `crates/lotta-providers/src/host/client.rs`, `host/catalog.rs`, `host/oauth.rs`, `host/pin.rs`; reference: `../letta-code/src/backend/dev/pi-provider-registry.ts`, `pi-model-factory.ts`, `pi-oauth.ts`, `pi-provider-mod-registry.ts`, `../letta-code/src/providers/openai-codex-provider.ts`

## Steps

- [x] Pin the resolved `@earendil-works/pi-ai` version to `0.82.1`, matching the parity lockfile even though `package.json` declares `^0.82.1`
- [x] Speak to the host over the Task 42 length-prefixed JSON framing on local pipes, with no host access to persistence or tools
- [x] Expose the full built-in catalog: OpenAI, Anthropic, OpenRouter, Bedrock, Google/Vertex, ZAI, MiniMax, Moonshot/Kimi, and the other registered providers
- [x] Route OpenAI Codex/ChatGPT OAuth through the host with PKCE/device-code state under a bounded expiry
- [x] Register mod-defined providers through the host's provider adapter protocol
- [x] Replay the `fixtures/providers/` corpus through the host and diff against the expected normalized traces

## Definition of done

- [x] The host pins resolved pi-ai `0.82.1` and refuses to start against a different resolved version
- [x] The host has no direct access to persistence or tools, and communicates only over the shared framing
- [x] The built-in catalog is served through the host and mod-defined providers register through the provider adapter protocol
- [x] OAuth runs with PKCE/device-code state under `OAUTH_STATE_TTL_SECONDS`, validates callback state, and never logs a code or token
- [x] The `fixtures/providers/` corpus replays through the host to the expected normalized traces
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(host::)'` and sees the version pin enforced, host isolation, the catalog, bounded OAuth, and fixture replay pass
