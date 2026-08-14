# 01 — Domain Model

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines the persistent and runtime entities Lotta manages. Their JSON shapes are formalized in [canonical-types.schema.json](canonical-types.schema.json). Field names follow the local Letta API and App Server wire contract.

---

## ID scheme

IDs are opaque strings on every public boundary. Lotta validates generated local IDs when it creates them, but accepts baseline conversation IDs as arbitrary non-empty strings and never rewrites a client-supplied ID.

| Prefix | Entity | Generation |
|---|---|---|
| `agent-local-` | Local agent | UUID v4 suffix |
| `local-conv-` | Generated non-default conversation | Monotonic local suffix; arbitrary non-empty conversation IDs are accepted on input |
| `letta-msg-` | API projection of a local message | Derived monotonic projection sequence; not stored in transcript JSONL |
| `ui-msg-` | Transcript/provider-facing local message | Monotonic local sequence stored in transcript entries |
| `local-run-` | Run | UUID or monotonic unique suffix |
| `resp_letta_` | Stored OpenAI Response | Unsigned base64url state cursor containing version, nonce, agent ID, and conversation ID |

`default` is a virtual conversation ID scoped to an agent. It is not globally unique. Internal maps use the pair `(agent_id, conversation_id)` as the key. `RuntimeScope` also carries optional `acting_user_id` for cloud-attributed input and channel gateway propagation.

---

## Schema definition map

| Definition | Role |
|---|---|
| `NonEmptyString`, `Timestamp` | Shared validated scalar types |
| `AgentId`, `ConversationId`, `MessageId`, `RunId` | Opaque typed identifiers |
| `RuntimeScope` | Agent/conversation pair used by protocol and runtime |
| `MemoryBlockInput` | Agent-creation memory file input |
| `Agent`, `Conversation`, `Run` | Primary local state entities |
| `LocalMessage` | Provider-facing message retained in transcript entries |
| `TranscriptManifest` | Version and message-format declaration |
| `SessionEntry`, `MessageEntry`, `CompactionEntry`, `TranscriptEntry` | JSONL transcript variants and union |
| `ApprovalRequest` | Paused tool execution awaiting a client decision |
| `ProviderConnection`, `ModelDescriptor` | Local inference connection and model catalog entry |
| `RuntimeConnection`, `ConversationRuntimeSnapshot` | Serializable runtime/connection projections |
| `QueueItem` | Pending work admitted to a conversation runtime |
| `ExternalToolRegistration` | Controller-owned tool definition |
| `Schedule` | Timed prompt targeting a runtime scope |
| `ChannelAccount`, `ChannelRoute` | Messaging identity and runtime binding |

---

## Persistent entities

### Agent

An Agent owns identity, default model configuration, system-prompt source, tags, compaction policy, and a MemFS repository.

Required fields:

- `id`, `name`, `system`, `tags`, `model`, `model_settings`
- optional `description`, `hidden`, `compaction_settings`

Creating an agent with local MemFS enabled stamps the Git-memory tag, initializes memory files from `memory_blocks`, creates the Git repository, and compiles the default conversation prompt before success is returned.

### Conversation

A Conversation is an ordered, stateful thread belonging to one Agent.

It carries:

- `id`, `agent_id`, `archived`, `created_at`, `updated_at`
- `archived_at`, `last_message_at`, `summary`
- `in_context_message_ids`
- optional conversation model override, model settings, context-window limit, tags, and hidden state

Archiving sets `archived_at`; unarchiving clears it. A conversation-level model overrides the agent model without mutating the agent.

### Transcript entry

The append-only transcript uses schema version 2 and message format `pi-session-entry-jsonl`.

Entry variants are:

- `session` — version 3 header carrying session ID, timestamp, and cwd
- `message` — parent-linked local message snapshot
- `compaction` — summary message, first kept entry, tokens before compaction, and optional stats

Every non-header entry has an `id`, `parentId`, and RFC3339 entry `timestamp`. Embedded `LocalMessage` objects use role `user`, `assistant`, or `toolResult` and a numeric millisecond `timestamp`.

### Provider connection

A ProviderConnection contains a provider ID, authentication method, non-secret connection metadata, and secret material stored outside ordinary diagnostics. A ModelDescriptor belongs to a provider connection or built-in local provider.

### Run

A Run records one provider-backed turn execution. It carries `id`, `agent_id`, `conversation_id`, status (`running`, `completed`, `failed`, or `cancelled`), timestamps, optional stop reason, background flag, metadata, and optional usage. A run is born `running` and can be cancelled through either conversation or run addressing.

### Schedule

A Schedule mirrors `CronTask`: identity and target, name/description, cron expression, IANA timezone, recurring flag, prompt, lifecycle status (`active`, `fired`, `missed`, or `cancelled`), fire/miss counters, jitter offset, and one-shot timestamps. Run history is stored separately in `runs/<schedule-id>.jsonl`. Schedule execution enqueues a `cron_prompt`; it does not bypass the conversation queue.

### Channel account and route

A ChannelAccount identifies one configured account for a channel plugin. Runtime account objects use camelCase; canonical `accounts.json` writes snake_case, including `group_policy`, `admin_users`, and `user_allowed_commands`. App Server snapshots expose the redacted common account fields and plugin-owned `config`. A ChannelRoute binds a channel account/chat/thread to an agent/conversation and carries inbound/outbound enablement.

---

## Runtime entities

### Listener runtime

The ListenerRuntime owns process-wide connections, process services, conversation runtimes, subscriptions, event sequencing, provider registry, and shutdown state.

### Connection

A Connection owns one authenticated transport, a stable ordinal, runtime subscriptions, request correlation, cancellation token, and per-connection event sequence. A reconnected connection can recover subscriptions and its next event sequence.

### Conversation runtime

A ConversationRuntime owns mutable execution state for exactly one `(agent_id, conversation_id)` pair:

- turn lifecycle and current lease
- serialized inbound chain and bounded pending queue
- cwd and workspace sandbox
- permission mode and temporary permissions
- skill sources, toolset, external tools, mods, hooks, reminders
- active run IDs, tool-call IDs, approvals, retry state, and last stop reason

### Turn and lease

A Turn is one accepted input plus all continuations required to reach a terminal stop. A TurnLease is an unforgeable generation token. Async work may emit or mutate state only while its captured lease remains current.

### Approval

An Approval pauses the active turn around a tool call. It carries a request ID, tool call ID, tool name, validated input, permission suggestions, optional blocked path, and optional diff previews. Resolution is `allow`, `deny`, or transport error.

### External tool registration

An ExternalToolRegistration is one controller-owned payload with `name`, optional `label`, `description`, and JSON Schema `parameters`. Runtime-start/update groups associate one or more definitions with an optional `scope_id`. Calls are correlated by request/tool-call IDs and resolved through the originating connection.

### Queue item

A QueueItem is a bounded pending input with stable ID, client message ID, kind, source, content, and enqueue timestamp. Wire removal dispositions are `dequeued` or `cancelled`. Internal drops additionally record `buffer_limit` or `stale_generation` reasons.

---

## Relationships

```text
Agent 1 ─────────────── * Conversation
  │                            │
  │                            ├── 1 Transcript
  │                            ├── 1 ConversationRuntime (while active)
  │                            └── * Schedule targets
  │
  ├── 1 Git MemFS repository
  ├── * Agent-scoped skills/mods/provider preferences
  └── * ChannelRoute targets

ListenerRuntime 1 ───── * Connection
       │                    │
       │                    └── * runtime subscriptions
       └─────────────── * ConversationRuntime
                              │
                              ├── 0..1 active TurnLease
                              ├── * queued inputs
                              ├── * pending approvals
                              └── * external tools
```

---

## State machines

### Turn lifecycle

```text
                          command completes
                    ┌────────────────────────┐
                    ▼                        │
idle ── command ──► command                  │
 │                                           │
 └── input/recovery ──► active ── success/error ──► idle
                           │
                           └── abort ──► cancelling ── owner settles ──► idle
```

Only the turn owner performs the final transition. A pending approval remains `active`; it is a continuation boundary, not a terminal state.

### Input disposition

```text
received ── duplicate client_message_id ──► return prior disposition
   │
   ├── runtime idle/continuation ─────────► started
   ├── runtime occupied ──────────────────► queued
   └── invalid/stopped/over limit ────────► rejected
```

### Conversation archival

```text
active ── archive ──► archived
  ▲                      │
  └────── unarchive ─────┘
```

---

## Required query patterns

| Query | Required behavior |
|---|---|
| Agent by ID | Direct lookup; 404 on absent or wrong local prefix |
| Agents by filters | Deterministic list order; name/query/tag/hidden filters compatible with local backend |
| Conversations for agent | Filter archived/hidden/tags; stable ordering and cursor behavior |
| Conversation by ID | Resolve with agent scope; never cross agents on `default` |
| Messages for conversation | Asc/desc ordering; `before`/`after`; limit; return-message-type filter |
| Messages for agent | Merge scoped conversations without losing chronological semantics |
| Message by projected ID | Resolve every projection key to the same source local message |
| Resume tail | Conversation plus newest bounded message suffix |
| Transcript search | Search persisted transcript text with agent/conversation filters |
| Runtime subscribers | Stable ordinal order for initialized live connections |

---

## Resource bounds

All values are named configuration constants and appear in status/metrics when reached.

| Constant | Default | Behavior at limit |
|---|---:|---|
| `CONNECTIONS_MAX` | 1,024 | Lotta hardening: reject upgrade with 503 |
| `RUNTIMES_MAX` | 4,096 | Lotta hardening: reject `runtime_start` |
| `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` | 256 | Lotta hardening: reject additional subscription |
| `QUEUE_ITEMS_SOFT_MAX` | 100 | Drop the oldest coalescable item to admit a new coalescable item; barriers may exceed this level |
| `QUEUE_ITEMS_HARD_MAX` | 300 | Reject every new item and emit `buffer_limit` |
| `PENDING_APPROVALS_PER_RUNTIME_MAX` | 128 | Lotta hardening: fail the turn as an invariant violation |
| `EXTERNAL_TOOLS_PER_RUNTIME_MAX` | 256 | Lotta hardening: reject registration atomically |
| `SCHEDULE_RUN_LOG_KEEP_LINES` | 2,000 | Rotate older lines when the line or byte bound is exceeded |
| `SCHEDULE_RUN_LOG_BYTES_MAX` | 2,000,000 | Rotate the per-schedule JSONL log |

---

## Assumptions and open questions

**Assumptions**

- Letta clients treat IDs as opaque strings except for known local-agent routing.
- The reference client types remain available as fixture input while the Rust wire types are established.

**Decisions**

- *Runtime key.* **Agent and conversation pair.** The `default` conversation makes a conversation ID alone insufficient.
- *Transcript history.* **Append-only parent-linked entries.** This preserves compaction boundaries and supports recovery without rewriting ordinary turns.
- *Lease ownership.* **Generation-token leases.** They prevent stale async work from contaminating a replacement turn.
- *Queue compatibility.* **Preserve the 100-item soft and 300-item hard tiers.** Coalescable status work may be replaced at the soft limit; barriers remain ordered and the hard limit rejects visibly.
- *Additional bounds.* **Lotta adds explicit limits where the baseline has none.** These hardening limits reject observably and are covered by boundary tests rather than being presented as reference constants.

**Open questions**

- *ID generation.* Should Lotta preserve sequential `local-conv-<n>` generation indefinitely while continuing to accept arbitrary baseline conversation IDs?
- *Storage indexing.* Does transcript scale require a derived SQLite index while JSON/JSONL remains canonical?
