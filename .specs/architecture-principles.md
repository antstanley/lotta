# Architecture Principles

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines how Lotta is organized. The architecture is a ports-and-adapters Rust workspace with a small composition root, one-way dependencies, and compatibility tests at every external boundary.

---

## Architectural pattern

The core contains domain types and state machines. It performs no filesystem, network, process, clock, random, Git, or provider I/O directly. Ports describe those effects. Adapters implement ports. The server binary composes concrete adapters and owns process lifecycle.

### What goes where

| Concern | Layer |
|---|---|
| IDs, entities, errors, state transitions | domain |
| Turn/queue/approval/compaction orchestration | runtime/application |
| WebSocket and HTTP JSON | transport adapter |
| JSON/JSONL and Git | persistence adapter |
| Provider APIs and sidecar RPC | provider adapter |
| Shell/files/MCP/sandbox | tool adapter |
| CLI/config/logging/process lifecycle | composition root |

Handlers decode, validate, call one application operation, and encode. They do not contain business logic.

---

## Workspace layout

```text
lotta/
├── Cargo.toml
├── rust-toolchain.toml
├── rustfmt.toml
├── deny.toml
├── crates/
│   ├── lotta-domain/          # IDs, entities, state machines, errors
│   ├── lotta-protocol/        # serde wire commands/messages and fixtures
│   ├── lotta-runtime/         # queues, turns, approvals, compaction
│   ├── lotta-store/           # JSON/JSONL persistence and migration
│   ├── lotta-memfs/           # Git memory repositories and prompt inputs
│   ├── lotta-providers/       # provider port, native adapters, sidecar client
│   ├── lotta-tools/           # registry, permissions, built-in executors
│   ├── lotta-extensions/      # skills, hooks, mods, MCP, subagents
│   ├── lotta-channels/        # channel protocol/control client types
│   ├── lotta-app-server/      # Axum HTTP/WS adapter and OpenAI routes
│   ├── lotta-config/          # validated configuration and secret references
│   ├── lotta-telemetry/       # tracing and metrics adapters
│   └── lotta-testkit/         # clocks, IDs, in-memory ports, golden fixtures
├── src/main.rs                # composition root only
├── fixtures/
│   ├── protocol/
│   ├── persistence/
│   ├── providers/
│   └── reference-traces/
└── .specs/
```

Crates stay responsibility-sized. A crate split is justified by dependency direction or replaceable adapter ownership, not naming preference.

---

## Dependency graph

```text
                            lotta binary
                                 │
                ┌────────────────┼────────────────┐
                ▼                ▼                ▼
          app-server         providers          tools/adapters
                │                │                │
                └──────────────► runtime ◄────────┘
                                      │
                              ┌───────┴────────┐
                              ▼                ▼
                           domain          port traits

 store, memfs, providers, tools, extensions, channels, telemetry
 implement ports; adapters never depend on the app-server transport.
```

Rules:

- `lotta-domain` depends only on narrowly chosen serialization/schema crates.
- `lotta-runtime` depends on domain and port traits, never concrete adapters.
- Adapter crates may depend on domain/protocol/runtime interfaces but not each other; cross-adapter orchestration belongs in the composition root.
- `lotta-protocol` owns public JSON and can depend on domain projection types, but domain never depends on protocol.
- No circular crate or module dependencies.
- Feature flags select adapters at composition time; they do not alter domain semantics.

---

## Rust baseline

- Edition 2024 on a pinned stable toolchain.
- Tokio multi-thread runtime with explicit task ownership and cancellation tokens.
- Axum/Tower for HTTP and WebSocket transport.
- Serde/serde_json for compatibility JSON.
- `thiserror` for crate errors; no vendor error crosses a port.
- `tracing` for structured events and OpenTelemetry/Prometheus adapters.
- `schemars` or checked-in generated JSON Schema only where generation output is deterministic and reviewed.
- `git2` is optional; the initial Git adapter can invoke a pinned external `git` executable to match credential and worktree behavior.

Dependency versions are pinned in `Cargo.lock`. Default features are disabled when they add unused network, TLS, or parser surfaces.

---

## Compatibility architecture

Compatibility is a first-class adapter:

```text
reference TypeScript baseline
      │ extract sanitized fixtures/traces
      ▼
checked-in compatibility corpus
      │
      ├── decode/encode through lotta-protocol
      ├── replay through Rust runtime with fake ports
      ├── read/write through both persistence implementations
      └── drive running Lotta with existing JS client/SDK
```

Every intentional incompatibility requires a change spec, protocol-version decision, migration, and client impact statement.

---

## Cross-cutting conventions

### Errors

Each crate exposes one typed error enum. Boundary adapters translate internal errors into stable protocol/HTTP codes. Programmer-invariant failures crash the owning process after structured diagnostics; user/provider/I/O errors remain values.

### IDs

Distinct newtypes prevent cross-entity ID use. Public IDs serialize as strings. The runtime key is a struct containing AgentId and ConversationId.

### Time

UTC RFC3339 on wire/disk. Durations use explicit `Duration`; persisted numeric durations include units in names. Runtime code obtains time from a `Clock` port.

### Limits

Every allocation, collection, loop, queue, retry, parser, and child process has a named bound. Boundary validation occurs before allocation proportional to untrusted input.

### Concurrency

One owner mutates each conversation runtime. Commands communicate through bounded channels. Shared maps store handles, not mutable conversation state. Lock order is documented and never spans `.await`.

### Secrets

Secret values use redacted wrapper types with no revealing `Debug`/`Display`. Substitution happens only in provider requests or child environments. Logs and errors pass through centralized scrubbing.

### Unsafe code

Workspace policy is `#![forbid(unsafe_code)]` except isolated OS adapter crates. Each exception has a SAFETY comment, invariant tests, and explicit review ownership.

---

## Testing architecture

| Tier | Purpose |
|---|---|
| Unit | Pure state machines, parsers, ID/path validation, error mapping |
| Property | Protocol decoding, queue/lease state, transcript parent chains, patch/path logic |
| Port contract | Every concrete adapter passes the same suite as its in-memory fake |
| Golden compatibility | TypeScript fixtures and event traces match Rust behavior |
| Cross-runtime | Each runtime reads and writes the other's state directory |
| Integration | Running server with fake provider and existing App Server client |
| End-to-end | Agent SDK/Desktop/channel smoke flows against built binary |
| Failure injection | disk full, partial writes, disconnects, timeouts, cancellation races, sidecar crashes |

Tests use fake clocks, deterministic ID generators, temporary roots, and local fake servers. No test depends on wall-clock sleeps or public provider availability.

---

## Assumptions and open questions

**Assumptions**

- Rust crates selected for transport and async execution support the required cancellation and backpressure semantics.
- Existing client packages can run as black-box conformance drivers in CI.

**Decisions**

- *Architecture.* **Ports and adapters.** The protocol, store, providers, tools, and compatibility hosts all change at different rates.
- *State mutation.* **Single owner per conversation.** This removes lock-order ambiguity from the turn state machine.
- *Compatibility evidence.* **Fixtures plus black-box clients.** Prose and Rust types alone cannot prove wire parity.
- *Sidecars.* **Adapters, never core dependencies.** The server remains functional with reduced extension/provider coverage when sidecars are disabled.

**Open questions**

- *Workspace granularity.* Which listed crates should begin as modules and split only after dependency pressure appears?
- *Schema generation.* Is Rust the eventual schema source of truth, or does the pinned TypeScript protocol continue generating compatibility fixtures?
