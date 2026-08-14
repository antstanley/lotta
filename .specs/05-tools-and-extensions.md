# 05 — Tools and Extensions

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines model-facing tools and extension compatibility. Built-in safety-sensitive behavior lives in Rust. Existing controller tools, MCP servers, skills, hooks, mods, and channel plugins connect through typed adapters.

---

## Responsibilities

1. Select model-appropriate toolsets and exact model-facing names/schemas.
2. Validate every tool input before policy or execution.
3. Apply hooks, permissions, approval policy, sandboxing, timeouts, output bounds, and secret scrubbing.
4. Execute Rust built-ins or route to an external owner.
5. Preserve tool lifecycle events and persisted tool results.
6. Discover skills and subagents with the same source precedence.
7. Provide bounded compatibility hosts for JavaScript mods and channel plugins.

---

## Tool registry

Each tool definition contains:

- internal stable name
- model-facing name per toolset
- JSON Schema input
- description asset
- execution owner: Rust, MCP, controller, mod sidecar, or channel gateway
- approval policy and permission action
- parallel-safety classification
- timeout and output limit
- secret-bearing fields and redaction policy

Toolset IDs are exactly `default`, `codex`, `codex_snake`, `gemini`, `gemini_snake`, and `none`; `auto` is a preference that resolves one of them. The default set carries the Anthropic-oriented names, while Codex and Gemini have snake/Pascal variants. Internal `Task` is globally exposed as `Agent`. An allowlist filters both built-ins and external tools; an empty allowlist exposes none.

---

## Execution pipeline

```text
model tool call
      │
      ▼
name resolution + JSON Schema validation
      │
      ▼
pre-tool hooks ── block/modify/allow
      │
      ▼
permission rules + sandbox gate
      │
      ├── deny ───────────────► structured denied result
      ├── ask ────────────────► approval request
      └── allow
            │
            ▼
secret substitution in child environment only
            │
            ▼
bounded executor / external call
            │
            ▼
post-success or post-failure hooks
            │
            ▼
scrub secrets + clamp result + persist + emit
```

Hook or mod failure is attributed to its owner. It cannot leave a half-registered toolset. Registry updates are built off to the side, validated, then swapped atomically.

---

## Rust built-ins

The Rust core implements the security and lifecycle primitives directly:

| Family | Required behavior |
|---|---|
| Files | read, write, edit, multi-edit, apply-patch, list, glob, grep, image view, and artifact-file read/write |
| Shell/process | one-shot shell/exec, PTY/session, background output, stdin, monitor, stop, and timeout |
| Memory | memory edit and patch operations with Git commit and path confinement |
| Planning/tasks | `update_plan`/`UpdatePlan`, `TodoWrite`, and task create/get/list/update/output/stop |
| Worktrees | enter/exit isolated Git worktree, ownership lock, provision includes/hooks/settings |
| Skills | load one registered skill and companion files |
| Interaction | approval and ask-user-question protocol bridge |
| LSP | `ReadLSP` diagnostics through configured language servers |

Aliases remain protocol-compatible but call one internal implementation. Tool output records distinguish success, user denial, interruption, timeout, validation failure, sandbox denial, spawn failure, and tool-defined error.

---

## Permissions and sandbox

Permission modes are `standard`, `acceptEdits`, `unrestricted`, and `strict`; the pinned default is `unrestricted`. File-backed rules load from user (`~/.letta/settings.json`, plus the legacy XDG path), project (`.letta/settings.json`), and local (`.letta/settings.local.json`) scopes. Session and mod rules layer in at check time. There is no baseline agent-file permission scope. Matching operates on normalized tool name, command, path, cwd, runtime scope, and requested action.

Shell analysis rejects bypasses rather than relying on string prefixes. File and memory paths are canonicalized before policy. Symlink traversal, `..`, alternate separators, shell redirection, subprocess launch, and command substitution are covered by negative tests.

OS sandbox adapters are:

- macOS Seatbelt
- Linux Bubblewrap
- explicit unsupported error on platforms without an enabled sandbox

Workspace sandbox roots constrain all filesystem tools and child processes. Peer workspaces below an isolation root remain hidden.

---

## External tools and MCP

Controller-owned external tools register during `runtime_start` or atomic updates. A tool can be unscoped or selected by `scope_id`. Calls carry runtime, request ID, tool call ID, name, validated arguments, and optional scope ID. The controller response returns content/error, and the server owns the fixed five-minute timeout. Disconnect rejects pending calls with a typed owner-disconnected result.

MCP supports config discriminants `stdio` (also the omitted default), `sse`, and `http` for streamable HTTP through a Rust MCP client. OAuth tokens and server credentials use the credential store. Tool discovery is bounded and namespaced. A server refresh replaces its tool group atomically.

---

## Skills

Discovery precedence from highest to lowest is:

1. project (`.agents/skills`, with legacy `.skills` fallback),
2. agent (`~/.letta/agents/<agent-id>/memory/skills`, with `$MEMORY_DIR/skills` read fallback),
3. global (`~/.letta/skills`),
4. bundled.

Runtime selection can restrict sources. Frontmatter `id`, `name`, and `description` are optional in the baseline: ID/name derive from the path, and description falls back to the first body paragraph or `No description available`. Lotta preserves those fallbacks. Loading a skill reads its complete instructions and companion files. Skill scripts execute under the same permission and sandbox policy as direct tools.

---

## Subagents

Built-in types include general-purpose, fork, recall, reflection, memory, history-analyzer, and init. A subagent request includes type, description, prompt, model policy, background flag, max turns, tools, memory scope, parent scope, and optional existing agent/conversation.

The first compatibility implementation spawns a pinned Letta Code subprocess for built-in subagents. Lotta defines a versioned, bounded sidecar protocol around that subprocess; this protocol is a Lotta adapter contract, not a pre-existing language-neutral port in the baseline. Parent status receives bounded state snapshots and stream events; silent subagents do not broadcast stream output.

Subagent filesystem and MemFS access is confined to explicit roots. Fork inherits conversation context; general-purpose starts isolated; recall reads historical messages; reflection edits through a memory worktree and merges under a lock.

---

## Hooks and mods

Hook events cover pre/post tool, tool failure, permission request, user prompt, notification, stop, subagent stop, pre-compact, session start/end. Command hooks run as sandboxed child processes. Prompt hooks are supported only for pre/post tool, tool failure, permission request, user prompt, stop, and subagent stop; notification, pre-compact, and session events remain command-hook-only.

Mods can register tools, commands, providers, permissions, lifecycle events, and UI metadata. Existing TypeScript mods execute in a separate compatibility host with a versioned JSON-RPC protocol. The host receives only declared capabilities and a scoped conversation handle. It cannot access Rust memory directly.

The compatibility host supports reload, dispose, generation invalidation, diagnostics, and safe mode. `--no-mods` starts without the host and removes every mod-owned registration.

---

## Limits

The 32,000-character result backstop, command/prompt hook timeouts, and five-minute external-tool timeout match the baseline. Remaining registry, frame, input, concurrency, and process-output limits are Lotta hardening.

| Constant | Default |
|---|---:|
| `TOOL_INPUT_BYTES_MAX` | 4 MiB |
| `TOOL_RESULT_BYTES_MAX` | 1 MiB before model-facing clamp |
| `TOOL_RESULT_MODEL_CHARS_MAX` | 32,000 backstop; shell/task/read commonly clamp at 30,000 and grep at 10,000, with full overflow written to a file |
| `TOOLS_LOADED_MAX` | 1,024 (Lotta hardening) |
| `HOOKS_PER_EVENT_MAX` | 64 (Lotta hardening) |
| `COMMAND_HOOK_TIMEOUT_MS_DEFAULT` | 60,000 |
| `PROMPT_HOOK_TIMEOUT_MS_DEFAULT` | 30,000 |
| `EXTERNAL_TOOL_CALL_TIMEOUT_MS` | 300,000 |
| `MCP_SERVERS_PER_AGENT_MAX` | 64 |
| `MCP_TOOLS_PER_SERVER_MAX` | 512 |
| `SUBAGENTS_PER_PARENT_MAX` | 128 |
| `SUBAGENTS_CONCURRENT_PER_PARENT_MAX` | 16 |
| `MOD_HOST_MESSAGE_BYTES_MAX` | 8 MiB |
| `CHILD_PROCESS_OUTPUT_BYTES_MAX` | 16 MiB |

---

## Assumptions and open questions

**Assumptions**

- Existing JS/TS extensions can operate through serialized capability calls without sharing process memory.
- Tool schemas and descriptions can be extracted as pinned build assets from the reference implementation.

**Decisions**

- *Built-in boundary.* **Security-sensitive primitives are Rust-native.** Shell, filesystem, memory, policy, and process ownership cannot depend on an untrusted extension host.
- *Extension compatibility.* **Bounded sidecars over embedded JavaScript.** A crash or leak in an extension host must not corrupt the server core.
- *Tool registry updates.* **Atomic replacement.** Models never observe a partially loaded toolset.
- *Tool parity.* **Model-facing tool names, schemas, fallbacks, and output clamps follow the pinned registry.** Process snapshots and terminal sessions remain protocol services, not invented model tools.
- *Additional limits.* **Lotta bounds registries and sidecar frames where the baseline has no named cap.** Such rows are labeled hardening and tested at their boundaries.
- *Errors.* **Stable typed results.** Tool errors are conversation data and must survive persistence and client upgrades.

**Open questions**

- *Mod API coverage.* Which UI panel/statusline capabilities are meaningful to a headless Rust App Server?
- *Native subagents.* At what parity milestone does Lotta replace the Letta Code subprocess compatibility path?
