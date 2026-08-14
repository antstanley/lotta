# Lotta

Lotta is a Rust implementation of the local [Letta Code](https://github.com/letta-ai/letta-code) App Server contract. It is intended to host Letta agents without a Letta Cloud dependency while remaining compatible with existing App Server clients and local state.

> [!IMPORTANT]
> Lotta is currently a specification-only, greenfield project. No Rust server implementation exists yet.

## Goals

- Match the local App Server WebSocket protocol and OpenAI-compatible HTTP API.
- Preserve agent, conversation, transcript, provider, and Git-backed MemFS behavior.
- Preserve turn ordering, streaming, approvals, cancellation, queues, compaction, and recovery.
- Support local providers, built-in and external tools, MCP, skills, hooks, mods, subagents, and messaging channels.
- Run as a bounded, observable, self-hosted Rust service developed under Tiger Style.

## Specification

The Draft specification is indexed in [`.specs/README.md`](.specs/README.md).

| Area | Specification |
|---|---|
| Scope and parity criteria | [Overview](.specs/00-overview.md) |
| Entities and state machines | [Domain model](.specs/01-domain-model.md) |
| WebSocket and HTTP APIs | [App Server API](.specs/02-app-server-api.md) |
| Turns, queues, and recovery | [Runtime and turns](.specs/03-runtime-and-turns.md) |
| Local state and Git memory | [Persistence and MemFS](.specs/04-persistence-and-memfs.md) |
| Tools and extension surfaces | [Tools and extensions](.specs/05-tools-and-extensions.md) |
| Inference adapters | [Model providers](.specs/06-model-providers.md) |
| Channels and deployment | [Channels and operations](.specs/07-channels-and-operations.md) |
| Rust workspace design | [Architecture principles](.specs/architecture-principles.md) |
| Rust Tiger Style | [Development guidelines](.specs/development-guidelines.md) |
| Canonical entities | [JSON Schema](.specs/canonical-types.schema.json) |

## Compatibility baseline

The initial behavioral baseline is:

- [`letta-ai/letta-code`](https://github.com/letta-ai/letta-code), version `0.30.20`, commit [`300f923f`](https://github.com/letta-ai/letta-code/commit/300f923f16cc8eee50656d7da732902c1dea2b65).
- [`letta-ai/letta-app-server-deployment`](https://github.com/letta-ai/letta-app-server-deployment), commit [`a37ac8e`](https://github.com/letta-ai/letta-app-server-deployment/commit/a37ac8e45f2e071f6e6c2cbb48b8e52a011e8cc4), for deployment behavior.

Compatibility is defined by observable protocol and persistence behavior, not by translating the TypeScript implementation module-for-module. See [NOTICE](NOTICE) for attribution.

## Development status

Implementation acceptance requires the existing Letta Agent SDK and Desktop clients, OpenAI compatibility fixtures, cross-runtime persistence fixtures, and failure-injection suites to pass against the Rust server. Until then, all specification pages remain Draft.

## License

Licensed under the [Apache License 2.0](LICENSE), matching Letta Code's license. Letta names and brand assets remain subject to the exclusions stated in that license. Lotta is an independent project and is not affiliated with or endorsed by Letta, Inc.
