# Lotta specifications

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This directory is the canonical design set for **Lotta**, a Rust implementation of the local Letta App Server. The compatibility baseline is `letta-ai/letta-code` version `0.30.20`, commit `300f923f`, together with the deployment behavior pinned by `letta-app-server-deployment/letta-code-version.txt` at `0.30.20`.

Lotta is greenfield. These Draft pages define the implementation contract and record the current divergence: no Rust implementation exists in this directory yet. Once implementation starts, every page is updated with verified source pointers and only moves to `Implemented` when its conformance suite passes.

Claims marked **baseline parity** reproduce observable Letta Code behavior. Claims marked **Lotta hardening** intentionally add bounds, durability, or safety while preserving accepted wire and storage inputs. A hardening that changes an observable client or cross-runtime contract requires a change spec and compatibility evidence before implementation.

## Reading order

| Page | Topic |
|---|---|
| [00-overview.md](00-overview.md) | Problem, goals, system shape, parity scope |
| [01-domain-model.md](01-domain-model.md) | Agents, conversations, messages, runs, runtimes, approvals, schedules, channels |
| [02-app-server-api.md](02-app-server-api.md) | WebSocket protocol, authentication, OpenAI-compatible HTTP API |
| [03-runtime-and-turns.md](03-runtime-and-turns.md) | Runtime scoping, queues, turn leases, streaming, recovery, compaction |
| [04-persistence-and-memfs.md](04-persistence-and-memfs.md) | Local storage compatibility, transcripts, MemFS, migrations |
| [05-tools-and-extensions.md](05-tools-and-extensions.md) | Tools, approvals, permissions, hooks, mods, skills, MCP, subagents |
| [06-model-providers.md](06-model-providers.md) | Provider connections, model catalog, inference and streaming adapters |
| [07-channels-and-operations.md](07-channels-and-operations.md) | Messaging channels, process topology, deployment and operations |
| [architecture-principles.md](architecture-principles.md) | Rust workspace, dependency direction, compatibility boundaries |
| [development-guidelines.md](development-guidelines.md) | Rust toolchain and Tiger Style rules |
| [canonical-types.schema.json](canonical-types.schema.json) | Canonical persisted and wire-adjacent entity shapes |

## Reference implementation

The parity source of truth is the adjacent checkout at `../letta-code/`. Important anchors are:

- `src/backend/backend.ts` and `src/backend/local/`
- `src/types/app-server-protocol.ts` and `src/types/protocol_v2.ts`
- `src/websocket/app-server*.ts` and `src/websocket/listener/`
- `src/tools/`, `src/mods/`, `src/hooks/`, `src/agent/subagents/`
- `src/channels/`
- `../letta-app-server-deployment/`

## Plans

Implementation plans live in `.specs/plans/`. Each plan is a self-contained kanban with task files, done certificates, and a dependency DAG.

| Plan | Status | Scope |
|---|---|---|
| [2026-08-14-lotta_rust_server](plans/2026-08-14-lotta_rust_server/plan.md) | In progress | Full Rust server implementation: 95 tasks across 9 milestones, from workspace bootstrap through conformance and release readiness |

## Change specs

No change specs exist. Changes to the parity baseline or intentional compatibility breaks belong in `.specs/changes/` before they alter these canonical pages.

## Assumptions and open questions

**Assumptions**

- The adjacent Letta Code checkout remains available while parity tests and fixtures are built.
- Ant Stanley owns product-level decisions for Lotta.

**Decisions**

- *Specification form.* **A layered Draft canonical set.** The server spans protocol, runtime, persistence, extension, provider, and operational concerns that cannot be reviewed safely in one document.
- *Single-project numbering.* **Numbered component pages live at the repository-global layer.** Lotta is one self-contained project, so the reading-order series and global architecture/guideline pages share `.specs/`; the numbering is recorded here as an intentional single-project convention.
- *Parity baseline.* **Letta Code `0.30.20` at `300f923f`.** Pinning a commit prevents “parity” from moving while Lotta is implemented.
- *Hardening labels.* **Observable departures are explicit.** Safety improvements do not masquerade as reference behavior and cannot invalidate SDK, Desktop, or storage compatibility silently.

**Open questions**

- *Version control.* Will `lotta/` be initialized as a Git repository, a jj workspace, or part of a parent repository?
- *Baseline upgrades.* Who approves moving the compatibility baseline to a newer Letta Code release?
