# Lotta — Rust Local Letta Server Overview

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

Lotta is a Rust server that hosts Letta agents without Letta Cloud. It implements the local-backend behavior and App Server interfaces exposed by Letta Code `0.30.20`, while retaining compatibility with the Letta Agent SDK, Desktop App Server clients, existing local state, and messaging-channel gateway protocol.

This page is the entry point. Detailed contracts are split across the pages indexed in [README.md](README.md).

---

## Problem

The TypeScript Letta Code process combines the App Server, local agent state, provider execution, tools, extensions, channels, and operational orchestration. Running it locally avoids Letta Cloud, but embeds server behavior in a CLI-oriented JavaScript harness.

Lotta provides the same local server surface in a bounded, strongly typed Rust runtime. Compatibility is measured at observable boundaries rather than by translating TypeScript module-for-module.

---

## Goals

1. Accept the App Server protocol used by the current Letta Agent SDK and Desktop clients.
2. Preserve local agent, conversation, transcript, provider, and MemFS behavior.
3. Preserve turn ordering, streaming, approval, cancellation, recovery, and queue semantics.
4. Expose the optional OpenAI-compatible Models, Chat Completions, and Responses APIs.
5. Execute built-in tools, MCP tools, client-owned external tools, skills, subagents, hooks, and mods with equivalent user-visible behavior.
6. Support local messaging channels through the same ChannelGateway/App Server contract.
7. Run as a single self-hosted binary for the App Server, with an optional separate channel process.
8. Make all resource limits, retries, state transitions, and failures explicit and observable.

## Non-goals

- Implement Letta Cloud environment registration, cloud relay routing, cloud-hosted secrets, or chat.letta.com access.
- Reproduce TypeScript implementation structure where the wire and persistence contract does not require it.
- Preserve undocumented bugs that are not captured by compatibility fixtures.
- Embed arbitrary JavaScript directly in the Rust core. JavaScript-compatible mods and channel plugins execute through a bounded sidecar adapter.
- Provide multi-tenant hostile-code isolation. Lotta is a trusted self-hosted agent runtime; OS sandboxing still constrains tools.

---

## System shape

```text
 Agent SDK / Desktop / custom controller
                  │
          WS /ws  │  optional HTTP /v1/*
                  ▼
┌───────────────────────────────────────────────────────────────┐
│                         lotta-server                          │
│                                                               │
│  transport ──► protocol ──► runtime registry                 │
│                                  │                            │
│                    ┌─────────────┼─────────────┐              │
│                    ▼             ▼             ▼              │
│              conversation    provider      tool runtime       │
│              turn machine    adapters      + approvals        │
│                    │             │             │              │
│                    └─────────────┼─────────────┘              │
│                                  ▼                            │
│                        persistence ports                      │
│                         │             │                       │
│                         ▼             ▼                       │
│                  JSON/JSONL store   Git MemFS                 │
└───────────────────────────────────────────────────────────────┘
             │                    │                    │
             ▼                    ▼                    ▼
      Ollama/OpenAI/etc.     OS shell/files       compatibility
                                                   sidecars
                                                       │
                                         JS mods / channel plugins
```

The transport layer authenticates and frames clients. The protocol layer maps exact JSON discriminants into typed commands. The runtime registry isolates `(agent_id, conversation_id)` scopes. Each conversation owns a serial queue and a lease-based turn state machine. Providers stream model events into the turn; tools can pause the stream for approval or external execution. Persistence writes reference-compatible agent JSON, transcript JSONL, compiled prompts, `providers/auth.json`, and Git-backed MemFS. Schedules, channel state, and settings retain their baseline paths outside the local-backend root.

---

## Compatibility definition

Lotta has parity only when all of these pass against the pinned reference:

| Surface | Compatibility test |
|---|---|
| WebSocket protocol | Existing `@letta-ai/letta-code/app-server-client` completes the same command fixtures and receives equivalent events |
| Agent SDK | Remote backend conformance suite creates, resumes, streams, compacts, forks, and deletes local agents/conversations |
| Desktop | Desktop smoke suite connects, syncs state, executes a turn, approves a tool, changes cwd, and reconnects |
| OpenAI API | Golden request/response/SSE fixtures for `/v1/models`, `/v1/chat/completions`, `/v1/responses` |
| Persistence | TypeScript and Rust runtimes round-trip both the local-backend root and the scoped `~/.letta` side stores without loss |
| MemFS | Both runtimes compile the same committed memory revision into equivalent prompt sections |
| Tools | Tool schema, approval policy, result, truncation, and error fixtures match |
| Channels | Existing ChannelGateway client drives a Rust App Server runtime through pairing, routing, turn, and reply fixtures |
| Reliability | Queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery traces match |

Exact equality is required for discriminants, IDs supplied by clients, persisted schema versions, protocol-required fields, storage keys, and compatibility-path names. Semantic equivalence is allowed for timestamps, generated UUIDs, human-readable diagnostics, Lotta-only stable error codes, and provider-dependent token counts. Explicit Lotta hardening may alter implementation mechanics only when reference clients and state remain accepted.

---

## Scope summary

| Area | Lotta contract | Reference anchor |
|---|---|---|
| Local backend | Full local capability set; cloud-only capabilities report unsupported | `src/backend/backend.ts`, `src/backend/local/local-backend.ts` |
| App Server | Protocol version 1, multi-client WebSocket server, optional OpenAI routes | `src/websocket/app-server.ts` |
| Runtime | Per-conversation queue, exactly-once terminal event, lease-guarded async work | `src/websocket/listener/` |
| Persistence | Compatible agent JSON, conversation JSON, schema-v2 transcript JSONL, prompt snapshots | `src/backend/local/local-store.ts` |
| Memory | Per-agent Git repository under the local backend root | `src/agent/memory-*.ts` |
| Providers | Local and API provider connections with streamed text/reasoning/tool calls | `src/backend/dev/`, `src/providers/` |
| Extensions | Built-in tools in Rust; external tools and compatibility sidecars | `src/tools/`, `src/mods/`, `src/hooks/` |
| Channels | Separate process, same App Server protocol and route/account model | `src/channels/`, `src/cli/subcommands/listen.tsx` |
| Deployment | Persistent local state, authenticated non-loopback binding, graceful shutdown | `letta-app-server-deployment/` plus self-host mode |

---

## Implementation acceptance

The implementation is accepted only when:

1. The Rust workspace builds with the pinned toolchain.
2. The protocol fixture generator records every `WsProtocolCommand` and `WsProtocolMessage` discriminant from the baseline.
3. Cross-runtime persistence tests pass in both directions for the backend root, provider auth, schedules, settings, and channel side stores.
4. Agent SDK and Desktop smoke suites pass without client patches.
5. The failure-injection suite proves baseline queue semantics, Lotta atomic-write/conflict hardening, cancellation, stale-lease suppression, and reconnect recovery.
6. Security tests prove non-loopback authentication, path confinement, secret redaction, and sandbox policy.

---

## Assumptions and open questions

**Assumptions**

- The Agent SDK and Desktop depend on the public App Server protocol rather than TypeScript internals.
- Local operators provide model credentials and accept responsibility for tool access to the host.
- Rust stable supports the selected async and WebSocket stack.

**Decisions**

- *Core language.* **Rust.** Ownership, exhaustive enums, fixed-width types, and explicit error values fit a long-running stateful server.
- *Parity boundary.* **Observable contracts, not source translation.** This keeps clients and data compatible while allowing a Rust-native architecture.
- *Hardening boundary.* **Implementation safety may exceed the baseline without narrowing accepted inputs.** Observable differences require explicit labels, tests, and a change-spec decision.
- *JavaScript compatibility.* **Out-of-process sidecars.** The Rust core remains memory-safe and bounded while existing plugin ecosystems remain usable.
- *Cloud dependency.* **None.** Cloud-only backend capabilities return stable unsupported errors rather than silently contacting Letta Cloud.

**Open questions**

- *Project name.* Is `lotta` the final binary, package, and protocol vendor name?
- *License.* Which license applies to the new Rust implementation and compatibility fixtures derived from Apache-2.0 Letta Code?
- *Support window.* How many Letta Code protocol baselines must Lotta support concurrently after `0.30.20`?
