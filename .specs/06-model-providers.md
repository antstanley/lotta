# 06 — Model Providers

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines local provider connection, model catalog, inference, streaming, usage, and error normalization. It mirrors the local provider behavior built around pi-ai in the pinned Letta Code baseline.

---

## Responsibilities

1. Store provider connections and credentials locally.
2. List models with stable handles and provider metadata.
3. Resolve agent and conversation model settings.
4. Normalize provider requests, text/reasoning/tool-call streams, usage, and stop reasons.
5. Enforce context windows, timeouts, retries, image policy, and cancellation.
6. Support provider extensions without leaking vendor types into the runtime core.

---

## Provider classes

| Class | Initial compatibility path |
|---|---|
| OpenAI-compatible API | Native Rust adapter over `reqwest` and SSE |
| Anthropic Messages | Native Rust adapter |
| Local endpoints: Ollama, Ollama Cloud, LM Studio, llama.cpp | Native endpoint/discovery adapters |
| OpenAI Codex/ChatGPT OAuth | Compatibility provider host until OAuth and request dialect pass fixtures |
| Full pi-ai built-in catalog, including OpenAI, Anthropic, OpenRouter, Bedrock, Google/Vertex, ZAI, MiniMax, Moonshot/Kimi, and other registered providers | Compatibility provider host registered through the provider adapter protocol |
| Mod-defined providers | Mod compatibility host |

The compatibility provider host pins resolved `@earendil-works/pi-ai` version `0.82.1`, matching the parity baseline lockfile even though `package.json` declares `^0.82.1`. It communicates through length-prefixed JSON messages over local pipes and has no direct access to persistence or tools.

---

## Model handle and settings

A model handle is a stable provider/model identifier. Agent records store a handle plus open `model_settings`. Conversation overrides carry the same pair. Normalization preserves settings supported by the baseline, including context-window limits, reasoning effort/tier, endpoint/base URL, provider type, and provider-specific options.

Resolution order is:

1. request-scoped temporary override,
2. conversation model and settings,
3. agent model and settings,
4. configured local default.

A model update validates availability before persistence. Failure leaves the prior model untouched. `list_models` reports connection readiness and does not expose credentials.

---

## Normalized provider port

```text
ProviderRequest
├── model handle/settings
├── system prompt
├── ordered messages
├── tool definitions/tool choice
├── image parts
├── context and output limits
├── reasoning controls
└── cancellation/deadline

ProviderEvent
├── TextDelta
├── ReasoningDelta | RedactedReasoning
├── ToolCallStart | ToolCallArgumentsDelta | ToolCallEnd
├── Usage
├── ProviderMetadata
├── Stop
└── Error
```

Adapters translate every vendor response into this enum before the runtime sees it. Vendor errors translate into `ProviderError` with stable kind: authentication, authorization, invalid request, rate limit, quota, timeout, context overflow, overloaded, unavailable, protocol, cancelled, or unknown.

---

## Streaming invariants

- Text and reasoning order is preserved per provider stream.
- Tool-call IDs remain stable across argument deltas and results.
- Partial JSON arguments are buffered with a declared bound and validated only at tool-call end.
- Usage is monotonic; the final usage snapshot is persisted.
- A stop event is emitted once.
- Cancellation closes the network stream and suppresses late adapter events.
- Image elision or conversion follows the request's strict/drop image policy.
- Provider metadata required for continuation is persisted without vendor secrets.

---

## Context and compaction

The effective context window is the minimum of configured server maximum, model catalog value, agent setting, and conversation override. Token estimation is provider-aware where available and conservatively approximated otherwise.

Before sending, the runtime computes context pressure. Above the configured threshold it invokes [03-runtime-and-turns.md](03-runtime-and-turns.md)'s compaction flow. A provider-reported context overflow can trigger one additional bounded compaction/retry path. Repeated overflow is terminal and includes measured/estimated context details without message content.

---

## Connections and credentials

Provider connect/disconnect commands expose declarative fields and auth methods. The compatibility surface is `providers/auth.json` version 1: provider configuration and API/OAuth secrets are stored together in plaintext JSON protected by directory mode `0700`, file mode `0600`, and log redaction. Adapters receive secrets without copying them into diagnostics. Live at-rest encryption requires a future migration because the pinned TypeScript runtime cannot read an encrypted replacement. Disconnect refuses while active turns use the connection unless `force` is explicit; forced disconnect cancels affected turns first.

OAuth flows run in the compatibility host or a native adapter with PKCE/device-code state stored under a bounded expiry. OAuth callbacks validate state and never log authorization codes or tokens.

---

## Retry and fallback

Adapters identify retryable errors but the runtime owns retry policy. Each policy declares attempts, retry-after handling, capped exponential backoff for transient/busy errors, linear empty-response delay, and total deadline. The pinned provider-turn retry path has no jitter. Authentication, invalid request, unsupported model, and schema errors do not retry.

Transport/provider fallback is explicit in model configuration or a compatibility map. A fallback emits a retry event naming source and destination transport. It does not silently modify the persisted model.

---

## Limits

The three-retry and 60-second backoff values match the provider-turn baseline. Remaining adapter cardinality, byte, timeout, OAuth, and image limits are Lotta hardening unless a native provider fixture establishes a tighter baseline value.

| Constant | Default |
|---|---:|
| `PROVIDERS_MAX` | 128 |
| `MODELS_PER_PROVIDER_MAX` | 10,000 |
| `PROVIDER_REQUEST_BYTES_MAX` | 32 MiB |
| `PROVIDER_RESPONSE_EVENT_BYTES_MAX` | 8 MiB |
| `TOOL_ARGUMENT_BYTES_MAX` | 4 MiB |
| `PROVIDER_TIMEOUT_MS_DEFAULT` | 600,000 |
| `PROVIDER_RETRIES_MAX` | 3 |
| `PROVIDER_BACKOFF_MS_MAX` | 60,000 |
| `OAUTH_STATE_TTL_SECONDS` | 600 |
| `IMAGE_BYTES_MAX` | 20 MiB for provider/tool ingestion; OpenAI requests remain bounded by the 20 MiB encoded HTTP body |

---

## Conformance

Provider fixture tests replay captured sanitized streams from the TypeScript baseline into each Rust adapter. Native endpoint discovery is tested separately from the static pi-ai built-in catalog; `models.json` is not treated as the local-provider catalog. Contract tests verify request mapping, event order, tool-call assembly, usage, cancellation, errors, timeout, context overflow, retry-after, and image policy.

A native adapter replaces a compatibility-host provider only after both produce equivalent normalized event traces for the same golden fixture corpus.

---

## Assumptions and open questions

**Assumptions**

- Provider APIs permit sanitized request/stream fixtures to be retained for testing.
- Operators accept that remote provider connections send prompts outside the local server, while local providers keep inference on-device.

**Decisions**

- *Core boundary.* **One normalized provider event enum.** Vendor SDK types cannot leak into runtime or persistence.
- *Migration path.* **Native common providers plus a pinned compatibility host.** This enables API parity before every provider dialect is reimplemented.
- *Retry ownership.* **Runtime, not adapters.** Central policy prevents nested retries from exceeding deadlines.
- *Credential compatibility.* **Live provider auth remains `auth.json` with restrictive permissions and mandatory redaction.** Encryption is not falsely claimed while the baseline requires plaintext round trips.
- *Retry timing.* **No jitter at the pinned baseline.** Exact retry traces use retry-after, exponential transient/busy delay, linear empty-response delay, and the 60-second cap.

**Open questions**

- *Live credential encryption.* Should a future change spec add keyring/KMS/encrypted-file storage with migration and TypeScript coexistence behavior?
- *Native provider order.* Which providers beyond OpenAI-compatible, Anthropic, Ollama, LM Studio, and llama.cpp receive native adapters first?
