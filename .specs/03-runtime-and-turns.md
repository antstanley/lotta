# 03 — Runtime and Turns

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines the execution state machine behind the App Server protocol. It preserves the listener invariants documented by `letta-code/src/websocket/listener/AGENTS.md` and the local backend behavior implemented in `src/backend/dev/fake-headless-backend.ts` (re-exported by `headless-backend.ts`).

---

## Responsibilities

1. Isolate execution by agent and conversation.
2. Serialize direct input admission and preserve baseline queue ordering, coalescing, drop, and rejection semantics.
3. Own active-turn state through one lease-based state machine.
4. Assemble system prompt, history, tools, skills, mods, permissions, reminders, cwd, sandbox, and model configuration.
5. Stream provider output and execute tool continuations.
6. Recover approvals, interrupted turns, and reconnect state.
7. Compact context manually or under pressure.
8. Emit authoritative state and exactly-once terminal outcomes.

---

## Runtime registry

The process owns one `ListenerRuntime`. It contains a map keyed by `(AgentId, ConversationId)`. Runtime creation is idempotent. A scope remains resident while lifecycle, queue, approval, interrupted-result, or sandbox-subscription state requires it. Once quiescent, it is evicted immediately; only its worktree watcher receives a separate 30-minute idle stop.

Per-turn ambient data uses Tokio task-local context rather than process globals:

- connection and device IDs
- agent/conversation IDs
- cwd and workspace sandbox
- permission mode
- selected skills and tool context
- cancellation token and lease generation

Spawned tasks receive an explicit snapshot. Background tasks cannot access a conversation implicitly.

---

## Lifecycle owner

```text
enum TurnState {
    Idle,
    Command { lease },
    Active { lease, turn_id, run_id },
    Cancelling { lease, turn_id, run_id },
}
```

`TurnState` is the only source of active state. UI projections such as `is_processing`, `loop_status`, active run IDs, cwd, and last stop reason are derived from it and related runtime fields. No parallel booleans can mutate activity.

Every async effect captures the current lease. Immediately after an await, it checks that:

1. the listener runtime is still active,
2. the conversation runtime still exists,
3. the captured lease is current,
4. cancellation policy permits the effect.

A stale lease does not write persistence, emit tool/protocol/channel events, or release a newer turn.

---

## Input and queue flow

```text
decode + validate
      │
      ▼
serialized admission chain
      │
      ├── duplicate ───────────────► prior acknowledgement
      ├── continuation/control ────► active lease
      ├── idle ────────────────────► acquire lease and start
      └── occupied ────────────────► bounded FIFO queue
                                           │
                                           ▼
                                  pump after owner settles
```

Queue item kinds and sources match the protocol. At 100 items, admitting a coalescable item replaces the oldest coalescable item; barrier items may pass the soft level. At 300 items, every admission is rejected with `buffer_limit`. Remove/cancel operations emit wire disposition `dequeued` or `cancelled`; internal drops retain their reason. Queue snapshots are emitted on every mutation. Task notifications, cron prompts, approval results, overlay actions, and mod continuations enter through the same queue rather than mutating a turn out of band.

The pump consumes only an `Idle` snapshot. It never “repairs” an impossible lifecycle state; impossible states are invariant failures with diagnostics.

---

## Turn setup

Setup executes in this order:

1. Resolve agent and conversation; reject cross-agent or archived-invalid access.
2. Resolve cwd; if deleted, fall back and record the original path for a one-time reminder.
3. Apply workspace sandbox and permission mode.
4. Synchronize or initialize local MemFS.
5. Compile the system prompt from managed prompt, memory files, available skills and runtime reminders.
6. Resolve conversation model override, agent model, provider connection, context window, and toolset.
7. Discover selected skill sources.
8. Load agent/global/project mods and hooks through compatibility adapters.
9. Merge built-in, MCP, mod, channel, and controller-owned external tools.
10. Build validated provider messages and emit sending/waiting loop status.

Failure before provider admission returns a terminal error without appending a user message. Failure after durable input append records an interrupted/error outcome that recovery can distinguish.

---

## Provider and tool loop

```text
send provider request
        │
        ▼
 stream text/reasoning/tool calls/usage/stop
        │
        ├── text/reasoning ──► persist projection + stream_delta
        │
        ├── tool call ───────► validate schema
        │                         │
        │                         ├── deny by policy ─► tool error result
        │                         ├── needs approval ─► control_request + wait
        │                         ├── external ───────► controller request + wait
        │                         └── local ──────────► bounded executor
        │                                                   │
        │◄──────────────────────── append tool result ◄──────┘
        │
        ├── context pressure ─► compact once, recompile, bounded retry
        ├── retryable error ──► emit retry, backoff, retry
        └── terminal stop ────► complete exactly once
```

Provider retries declare max attempts, retry-after handling, capped exponential backoff for transient/busy failures, and linear backoff for empty responses. The pinned baseline does not add jitter. An empty response, context overflow, transport failure, provider quota error, and user cancellation remain distinct typed reasons. Provider fallback never changes the persisted model unless a user command does so.

---

## Approvals

Approval requests are part of the active lease. The runtime stores the request before emitting it. Resolution validates request ID, tool call ID, lease generation, and optional edited input against the original tool schema.

- Allow executes the approved call once.
- Deny appends a structured denied tool result and resumes the model when the tool protocol requires continuation.
- Abort moves `Active` to `Cancelling`; a late approval response cannot clear cancellation.
- Reconnect `sync` can probe persisted/backend state for stale approvals and replay unresolved requests.
- Process restart recovery either reconstructs a safe continuation or emits explicit denial/interruption; it never guesses that a tool ran.

---

## Compaction and prompt refresh

Supported modes match local backend behavior:

- `all` — summarize the eligible history into one summary message.
- `sliding_window` — summarize an oldest prefix and retain a configured recent percentage.

Triggers are manual API request, context pressure before provider call, or provider context-overflow response. Compaction is serialized with the turn lease, emits mod lifecycle callbacks, stores a transcript compaction entry, updates in-context IDs, recompiles the prompt, and records before/after token/message counts.

MemFS changes are detected by committed revision. Before the next turn, the prompt is recompiled. For providers supporting mid-conversation system messages, a committed memory update can be injected without rewriting history; otherwise the new prompt applies at the next provider request boundary.

---

## Cancellation and terminal behavior

Cancellation is cooperative first and forceful after a deadline:

1. Transition `Active` to `Cancelling` and cancel provider/tool tokens.
2. Normalize unfinished local tool calls to interrupted results.
3. Stop accepting side effects from the lease except cancellation terminal events.
4. Wait `TURN_CANCEL_GRACE_MS`.
5. Kill remaining child processes in the runtime scope.
6. The owner emits one `turn_finished(cancelled)` and releases the lease.

Terminal completion persists message/transcript state before announcing completion. Post-turn reflection and memory push happen after terminal projection and cannot change that turn's outcome.

---

## Runtime bounds

| Constant | Default | Behavior at limit |
|---|---:|---|
| `TURN_PROVIDER_RETRIES_MAX` | 3 | Stop with the normalized provider error |
| `TURN_EMPTY_RESPONSE_RETRIES_MAX` | 2 | Stop with `empty_response` |
| `CONTEXT_OVERFLOW_COMPACTIONS_MAX` | 3 | Stop with context overflow; preflight pressure uses reserve-token heuristics |
| `TURN_TOOL_CALLS_MAX` | 256 | Lotta hardening: stop before executing another tool |
| `TURN_STEPS_MAX` | 256 | Lotta hardening: terminate with `step_limit` |
| `TURN_CANCEL_GRACE_MS` | 10,000 | Lotta hardening: kill remaining runtime-owned children; shell children use a 2,000 ms SIGTERM→SIGKILL grace |
| `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT` | 180,000 | Lotta hardening: interrupt the local tool; per-tool overrides remain bounded |
| `EXTERNAL_TOOL_CALL_TIMEOUT_MS` | 300,000 | Match the baseline controller-call timeout |
| `APPROVAL_WAIT_MS_MAX` | 86,400,000 | Lotta hardening: interrupt the turn and replay an explicit expired approval state |
| `QUEUE_PUMP_BATCH_MAX` | 64 | Lotta hardening: yield before pumping the next batch |

Every loop asserts progress toward one of these bounds. Rows marked Lotta hardening have no equivalent reference constant and require boundary fixtures proving that ordinary baseline clients are unaffected.

---

## Observability

Structured events include runtime key, connection ID, turn/lease generation, run ID, provider, tool call ID, attempt, queue length, stop reason, and duration. Prompt text, message bodies, tool inputs, credentials, and secret-substituted commands are excluded by default.

Metrics cover admissions, queue depth, active turns, cancellation latency, retries, compactions, provider latency, tool duration, stale-lease suppressions, and terminal outcomes.

---

## Assumptions and open questions

**Assumptions**

- Provider adapters expose cancellation and a normalized event stream.
- Durable append operations can complete before terminal events are emitted.

**Decisions**

- *State ownership.* **One enum and one owner.** Parallel activity flags create contradictory queue and UI states.
- *Async safety.* **Lease checks after every awaited boundary.** Cancellation alone cannot prevent late completion from writing.
- *Queue policy.* **Per-runtime FIFO with baseline soft coalescing and a hard rejection ceiling.** This preserves barrier ordering while bounding replaceable status work.
- *Compaction.* **Recorded as transcript history.** Summary generation changes context and must be auditable and recoverable.
- *Runtime eviction.* **Evict immediately once quiescent.** This matches the listener; the worktree watcher, not the runtime, owns the 30-minute idle timer.
- *Approval timeout.* **Lotta interrupts after 24 hours.** The baseline has no approval timeout; this hardening must produce a recoverable terminal state rather than an implicit denial.

**Open questions**

- *Parallel tools.* Which built-in tools are certified parallel-safe in the initial Rust executor?
- *Crash recovery.* Is restart recovery limited to approvals and durable turns, or must in-flight provider streams resume from run IDs?
- *Approval compatibility.* Is a 24-hour approval expiry acceptable for all Desktop and channel workflows, or should the bound be configurable below a larger hard maximum?
