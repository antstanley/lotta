# 02 — App Server API

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines Lotta's network contract. Protocol parity is anchored to `APP_SERVER_PROTOCOL_VERSION = 1` and the command/message unions in `letta-code/src/types/protocol_v2.ts` at the pinned baseline.

---

## Responsibilities

1. Bind an HTTP server and WebSocket control endpoint.
2. Authenticate non-loopback and Origin-bearing clients.
3. Decode every supported protocol-v1 command with strict discriminant validation.
4. Route commands to the process runtime or a scoped conversation runtime.
5. Emit correlated responses, state snapshots, stream deltas, and terminal events in order.
6. Optionally expose OpenAI-compatible HTTP routes.
7. Reap half-open clients and shut down without abandoning process children.

The transport does not own agent logic, persistence semantics, tool policy, or provider adaptation.

---

## Listener configuration

The listener and authentication flags mirror the local App Server. `--backend local` is a Lotta-only explicit mode selector; the baseline command is `letta server --listen`.

```text
lotta server --backend local --listen [ws://host:port]
             [--openai-api]
             [--ws-auth capability-token|signed-bearer-token]
             [--ws-token-file <absolute-path>]
             [--ws-token-sha256 <64-hex>]
             [--ws-shared-secret-file <absolute-path>]
             [--ws-issuer <value>]
             [--ws-audience <value>]
             [--ws-max-clock-skew-seconds <u32>]
```

Bare `--listen` binds `ws://127.0.0.1:0`. `/ws` is the default path, a path in the listen URL overrides it, and `/` is also accepted for WebSocket upgrade compatibility. Startup prints the resolved base URL, WebSocket URL, and optional OpenAI base URL.

Non-loopback binding without authentication fails before listening. Capability-token mode accepts either a token file or precomputed SHA-256 digest, never both. Signed-bearer mode verifies HS256, mandatory `exp`, optional `nbf`, configured issuer/audience, and clock skew. The baseline skew default is 30 seconds; Lotta rejects configuration above its 300-second safety maximum. Absolute secret files are read once at startup and their contents are never logged.

---

## WebSocket command groups

All command JSON uses the exact snake_case field names and string discriminants of `WsProtocolCommand`.

| Group | Commands |
|---|---|
| Runtime | `runtime_start`, `input`, `sync`, `abort_message`, `change_device_state` |
| External tools | runtime external-tool update and external-tool-call response commands |
| Teleport | commands `teleport_probe`, `teleport_request`, `teleport_failed`; messages `teleport_probe_response`, `teleport_ready`; continuation is `input.kind = teleport_continue` |
| Terminal | `terminal_spawn`, `terminal_input`, `terminal_resize`, `terminal_kill` |
| Files | search, grep, list directory, tree, read, write, edit, watch, unwatch, file operations |
| Memory | list/history/read-at-ref/read/write/delete/diff/enable MemFS |
| Models/providers | list models, list/connect/disconnect providers, usage read, update model/toolset |
| Schedules | cron list/add/get/runs/trigger/update/delete/delete-all |
| Skills | enable, disable |
| Agent management | create shortcut, list/retrieve/create/update/delete |
| Conversation management | list/retrieve/create/update/recompile/fork/messages/compact |
| Settings | cwd map, reflection settings, experiments |
| Channels | list, accounts, config, start/stop, pairing, routes, targets |
| Device commands | execute slash/mod command, remove queue item, branch search/checkout, secret list/apply |
| Introspection | `app_server_info` |

Unknown top-level `type` values are logged locally and dropped without a response or state mutation, matching the baseline. Malformed known `input` frames produce the baseline non-terminal loop-error notice. Extra object fields are ignored where baseline structural guards ignore them; required known fields and enum values remain strict.

Terminal sessions are scoped by connection and `terminal_id`. Spawn carries `cols`, `rows`, and optional cwd; success emits `terminal_spawned`, output emits `terminal_output`, and exit/spawn failure emits `terminal_exited`. Input and resize for an absent session are no-ops. To tolerate React Strict Mode, a live session younger than two seconds is reused on repeat spawn and ignores kill during that window. Connection cleanup kills its sessions. Background-process snapshots are listener state messages, not model-facing tools.

---

## Core lifecycle

```text
client                              Lotta
  │                                   │
  ├── runtime_start(request_id) ─────►│ resolve/create agent + conversation
  │◄── runtime_start_response ────────┤ register subscription and tools
  │◄── update_device_status ──────────┤ initial replay
  │◄── update_loop_status ────────────┤
  │◄── update_queue ──────────────────┤
  │                                   │
  ├── input(request_id, messages) ───►│
  │◄── input_accepted(started|queued) ┤ acknowledgement precedes completion
  │◄── stream_delta* ─────────────────┤ text/reasoning/tool/status
  │◄── control_request? ──────────────┤ approval boundary
  ├── input(approval_response) ──────►│
  │◄── stream_delta* ─────────────────┤ continuation
  │◄── turn_finished ─────────────────┤ exactly once
  │                                   │
  ├── sync(request_id) ──────────────►│ replay authoritative snapshots
  │◄── sync_response ─────────────────┤ after replay
```

`runtime_start` accepts an existing or newly created agent, an existing or newly created conversation, cwd, permission mode, workspace sandbox, skill sources, client metadata, approval recovery flags, and external tools. Mutually exclusive choices are rejected before allocation.

`input_accepted` acknowledges admission only. `accepted: true` carries `started` or `queued`. Repeated `client_message_id` values return the prior disposition without executing twice.

---

## Event envelopes and ordering

The broadcast runtime frames `control_request`, `update_device_status`, `update_loop_status`, `update_queue`, `stream_delta`, `turn_finished`, and `update_subagent_state` carry:

- `runtime: { agent_id, conversation_id, acting_user_id? }`
- monotonically increasing per-connection `event_seq`
- RFC3339 `emitted_at`
- an `idempotency_key` unique to that emission (`<type>:<seq>:<uuid>`), not stable across replay

Connection-specific and management responses carry request correlation where their type defines it, but are not stamped with the runtime envelope or event sequence. State updates are snapshots, not diffs, except queue removals which include explicit ordered transitions.

The server preserves these invariants:

1. A connection sees increasing event sequence values.
2. `input_accepted` is emitted before events caused by that accepted input.
3. Tool-start precedes its tool-end; a missing terminal tool event is repaired by the next authoritative loop snapshot.
4. `turn_finished` is emitted once per admitted turn and after its final stream delta.
5. A stale lease emits nothing after a replacement turn owns the scope.
6. Broadcast and subscriber delivery preserve stable connection ordinal order.

---

## Outbound message groups

| Group | Message types |
|---|---|
| Control | `control_request`, external-tool-call request |
| Admission | `input_accepted`, abort/sync/runtime-start responses |
| State | device, loop, queue, subagent, memory, skills, cron, channel update snapshots |
| Stream | message delta, client-tool start/end, command start/end, status, retry, loop error |
| Terminal | `turn_finished`, terminal exited, operation responses |
| Management | agent, conversation, model, provider, memory, cron, channel, file, branch, secret responses |

The Rust protocol crate contains one `#[serde(tag = "type")]` enum for commands and one for messages. Every baseline discriminant has a variant. A compile-time manifest test compares the Rust variant list with a checked-in fixture extracted from the TypeScript union.

---

## HTTP API

The OpenAI-compatible `/v1/*` routes are enabled only with `--openai-api`. Capability and health routes are always registered.

| Method | Path | Contract |
|---|---|---|
| `GET` | `/v1/models` | Lists up to 1,000 visible agents as OpenAI model objects |
| `POST` | `/v1/chat/completions` | Stateful/header-keyed or stateless chat completion; JSON or SSE |
| `POST` | `/v1/responses` | Responses API subset with text, reasoning and function-call output; JSON or SSE |
| `GET` | `/app-server-info` | Authenticated capability discovery equivalent to the WebSocket command |
| `GET` | `/healthz`, `/readyz` | Baseline liveness and readiness probes |

An agent's unique, non-colliding name is its advertised model ID; otherwise its agent ID is advertised. Raw agent IDs also resolve. Missing models return OpenAI `invalid_request_error` with code `model_not_found`.

Chat Completions requires a non-empty model and messages containing usable user text or image content. `X-Letta-Chat-Key` maps to one persisted conversation; streaming requests also accept `X-OpenWebUI-Chat-Id`. Stateful requests send only the newest user input. Headerless requests create an ephemeral conversation, replay user/assistant transcript content, and delete the conversation after settlement. The 4,096-entry chat-key map is in-memory FIFO; eviction causes the next request for that key to create a new conversation.

Responses supports `input`, `instructions`, `previous_response_id`, `store`, and `stream`. A successful `store: true` request retains its conversation and returns an unsigned `resp_letta_` base64url cursor carrying version, nonce, agent ID, and conversation ID. Otherwise it returns `resp_<uuid>` and deletes a headerless ephemeral conversation; an `X-Letta-Chat-Key` conversation remains stateful. A valid `previous_response_id` creates a hidden fork and returns `501 unsupported_backend` when conversation forking is unavailable. Failed outcomes are not stored. Streaming follows OpenAI event names and ends in completed or failed response state.

Only Chat Completions honors `Idempotency-Key` and `X-Idempotency-Key`. It checks the cache before conversation allocation, shares an in-flight turn, replays a successful settled outcome, and evicts failed outcomes. Responses does not implement idempotency-key caching at the pinned baseline.

---

## Transport bounds

| Constant | Default | Enforcement |
|---|---:|---|
| `HTTP_BODY_BYTES_MAX` | 20 MiB | Match the baseline OpenAI request-body cap; reject before JSON decode |
| `WS_FRAME_BYTES_MAX` | 100 MiB | Make the `ws` dependency's effective baseline default explicit; close 1009 |
| `WS_MESSAGE_FIELDS_MAX` | 4,096 | Lotta hardening: reject decode before field-proportional work |
| `WS_PING_INTERVAL_MS` | 30,000 | Protocol ping |
| `WS_PONG_TIMEOUT_MS` | 90,000 | Terminate half-open socket |
| `AUTH_CLOCK_SKEW_SECONDS_MAX` | 300 | Lotta hardening: reject startup value above bound |
| `REQUEST_ID_BYTES_MAX` | 256 | Lotta hardening: reject command |
| `CHAT_IDEMPOTENCY_OUTCOMES_MAX` | 1,024 | FIFO eviction, including the oldest in-flight entry; failed outcomes evict on settlement |
| `OPENAI_CHAT_KEYS_MAX` | 4,096 | In-memory FIFO eviction; the next request creates a new conversation |

---

## Errors and closure

- HTTP errors use status plus stable JSON error envelopes.
- WebSocket upgrade errors use HTTP 400/401/403/503 before upgrade.
- Protocol errors correlate to `request_id` where present.
- Internal errors are scrubbed before returning; diagnostics retain a generated incident ID.
- SIGINT/SIGTERM stops admissions, requests cancellation, waits a bounded grace period, persists settled state, closes sidecars and child processes, then closes sockets.

---

## Assumptions and open questions

**Assumptions**

- Existing clients negotiate protocol behavior by command support and `app_server_info`, not by TypeScript package identity.
- HTTP and WebSocket share one listener and authentication policy.

**Decisions**

- *Protocol source.* **Pinned discriminant fixture plus golden JSON.** Rust exhaustiveness alone cannot detect upstream TypeScript additions.
- *Decode policy.* **Strict known shapes with baseline-compatible unknown handling.** Unknown discriminants are dropped, extra fields tolerated where the baseline guards tolerate them, and malformed known inputs never execute.
- *Admission acknowledgement.* **Separate from completion.** Clients need to distinguish rejection, immediate execution, and queueing without waiting for a turn.
- *Non-loopback security.* **Authentication is mandatory.** The server exposes shell and filesystem capabilities on its host.
- *Transport hardening.* **Lotta makes implicit dependency bounds explicit and adds a field-count bound.** The HTTP and WebSocket byte caps do not narrow the pinned baseline's accepted range.

**Open questions**

- *Origin policy.* Which browser origins, if any, are accepted in addition to authenticated native clients?
- *TLS termination.* Does Lotta terminate TLS directly or require a reverse proxy for non-loopback production deployments?
