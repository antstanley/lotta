# Independent Spec-Reviewer Pass — Lotta Canonical Spec vs `2026-08-14-lotta_rust_server` Plan

Mode: adapted **R1** (delta-vs-canonical reference resolution and consistency trace) plus **R2**'s forward/reverse coverage passes, retargeted from "code" to "plan artifacts", with the pinned `letta-code@300f923f` checkout used as the parity oracle. This is **not** an R2 canonical-vs-code verdict; no Rust exists.

---

## Checkpoint 1 — Premises and artifacts reviewed

**P1 — Canonical set (read end to end).** `.specs/README.md`, `00-overview.md` (145 L), `01-domain-model.md` (259 L), `02-app-server-api.md` (205 L), `03-runtime-and-turns.md` (220 L), `04-persistence-and-memfs.md` (253 L), `05-tools-and-extensions.md` (192 L), `06-model-providers.md` (159 L), `07-channels-and-operations.md` (213 L), `architecture-principles.md` (195 L), `development-guidelines.md` (236 L), `canonical-types.schema.json` (24 `$defs`).

**P2 — Plan artifacts (read end to end).** `plan.md` (253 L), 46 task files, 46 done certificates (5,275 L total in `backlog/`), plus one stray artifact.

**P3 — Method sources (read in full).** `spec-reviewer/SKILL.md` + `references/r1`, `r2`, `r3`; `spec-planner/SKILL.md` + `checklist.md`, `task-decomposition.md`, `plan-template.md`, `model-policy.md`; `done-certificates/SKILL.md` + `certificate-template.md`, `semiformal-method.md`.

**P4 — Parity oracle.** `../letta-code` at `300f923f16cc8eee50656d7da732902c1dea2b65`, `package.json` version `0.30.20`; `../letta-app-server-deployment/letta-code-version.txt` = `0.30.20`. **The baseline pin in `.specs/README.md` and `plan.md` is exact and verified.**

**P5 — Rule under test.** A plan is a *buildable, reviewable decomposition of a spec*: every in-scope spec section maps to a task, every task is authorized by a named spec section, every pointer resolves, every DoD is checkable, and every certificate names evidence two validators would collect identically.

**Standing note on the spec itself.** Wherever I spot-checked canonical claims against the baseline, the spec held: the 20 channel service commands (`types/service-protocol.ts:7-27`), `--ws-auth capability-token|signed-bearer-token` (`websocket/app-server-auth.ts:11`), `QueueItemDroppedReason = "buffer_limit" | "stale_generation"` (`types/protocol.ts:492`), `codex_snake`/`gemini_snake` toolsets (`tools/toolset.ts:198,204`), `APP_SERVER_PROTOCOL_VERSION = 1` (`types/app-server-info.ts:1`), and no jitter in the provider-turn retry path (jitter appears only in `websocket/listen-register.ts:214,253`, the Cloud register path). **The divergences below are the plan departing from an accurate spec, not the spec being wrong.**

---

## Checkpoint 2 — Reference / link / pointer resolution

### 2a. Prospective Lotta paths (correctly forward-looking)

`crates/lotta-*/src/...`, `fixtures/protocol/`, `tests/conformance/` do not exist and are not expected to. These are legitimate prospective pointers. **No finding.**

### 2b. Spec-page targets — mechanically resolved

Every `**Implements:**` segment was parsed and each `§anchor` matched against the actual `##`/`###` heading set (and `$defs` for the schema):

```
anchors checked: 139   broken: 89   tasks with ≥1 broken anchor: 34 of 46
```

Two targeted files **do not exist at all**:

| Phantom target | Cited by | Sections claimed |
|---|---|---|
| `.specs/03-runtime.md` (21 anchors) | 09, 10, 11, 12, 14, 15, 16, 17, 23, 27, 31, 40 | Runtime scoping, Runtime registry, Turn state machine, Lease generation, Stale lease suppression, Input queue, Coalescing, Soft and hard limits, Turn setup, Resolution order, Agent loop, Step processing, Approval flow, Cancellation, Cooperative and forceful cancellation, Compaction, Tool call lifecycle, Event sequencing (×2), Recovery |
| `.specs/06-configuration-and-telemetry.md` (3 anchors) | 36 | Configuration, Telemetry, Health endpoints |

The real page is `.specs/03-runtime-and-turns.md`, and **not one** of the 21 claimed section names exists on it (its headings are Runtime registry, Lifecycle owner, Input and queue flow, Turn setup, Provider and tool loop, Approvals, Compaction and prompt refresh, Cancellation and terminal behavior, Runtime bounds, Observability). There is no configuration/telemetry page anywhere in the canonical set.

The remaining 65 broken anchors are heading names that do not exist on real pages — e.g. `06-model-providers.md §OpenAI adapter`, `§Ollama`, `§LM Studio`, `§llama.cpp`, `§pi-ai sidecar`, `§auth.json`, `§Provider connections`; `02-app-server-api.md §WebSocket listener`, `§Authentication`, `§HTTP REST API`, `§Event delivery`, `§Sync and reconnect`, `§OpenAI-compatible HTTP API`, `§Health endpoints`; `architecture-principles.md §Port/adapter pattern` (×4), `§Security boundaries` (×2), `§Composition`; `05-tools-and-extensions.md §Permissions`, `§Sandbox`, `§Hooks`, `§Mods`, `§MCP`, `§External tools`, `§Toolset resolution`, `§Provider port`, `§Sidecar lifecycle`; `07-channels-and-operations.md §Cron scheduler`, `§Scheduled messages`, `§Channel management protocol`, `§Control-plane client`, `§Pairing`, `§Slash commands`, `§Deployment`, `§Docker`; `00-overview.md §System overview`, `§SDK compatibility`, `§Desktop compatibility`, `§Compatibility`; `01-domain-model.md §CompactionEntry`, `§ProviderConnection`, `§ChannelAccount`, `§ChannelRoute`.

### 2c. Baseline anchors into the pinned checkout — mechanically resolved

```
letta-code pointers checked: 47   broken: 21
```

| Broken pointer | Cited by | Actual baseline anchor at `300f923f` |
|---|---|---|
| `src/websocket/listener/agent-runner.ts` | 09, 10, 12, 14, 15, 16, 17, 40 | `websocket/listener/runtime.ts`, `conversation-runtime.ts`, `turn-lifecycle.ts`, `turn.ts`, `turn-setup.ts`, `turn-approval.ts`, `turn-terminal.ts`, `send-lease.test.ts` |
| `src/websocket/listener/agent-runner-queue.ts` | 11 | `websocket/listener/queue.ts`, `inbound-queue.ts`, `queue-update-transitions.test.ts`, `queue-no-coalesce.test.ts` |
| `src/providers/base.ts`, `openai.ts`, `anthropic.ts`, `ollama.ts`, `lmstudio.ts`, `auth.ts` | 13, 18, 19, 20 | `backend/dev/pi-*-provider.ts` (`pi-ollama-provider.ts`, `pi-lmstudio-provider.ts`, `pi-llama-cpp-provider.ts`, `pi-local-endpoint-provider.ts`, `pi-model-factory.ts`); `backend/local/local-provider-auth-store.ts`; `providers/provider-connections.ts` |
| `src/tools/base.ts`, `registry.ts`, `permissions.ts`, `sandbox.ts` | 21, 22, 41 | `tools/define-tool.ts`, `tools/manager.ts`, `tools/toolset.ts`, `permissions/` (`checker.ts`, `matcher.ts`, `loader.ts`, `analyzer.ts`, `mode.ts`) |
| `src/tools/impl/file.ts`, `impl/mcp.ts` | 23, 25 | `tools/impl/read.ts`, `edit.ts`, `multi-edit.ts`, `apply-patch.ts`, `glob.ts`, `grep.ts`; `mcp-client.ts`, `mcp-runtime.ts`, `mcp-oauth.ts` |
| `src/http/`, `src/http/openai.ts` | 30, 32, 44 | `websocket/app-server-openai.ts`, `app-server-openai-common.ts` |
| `src/sdk/`, `src/desktop/`, `src/config/`, `src/backend/local/backup.ts` | 42, 43, 36, 45 | `app-server-client.ts` for SDK; no `desktop/`, `config/`, or `backup.ts` module exists |

`src/types/protocol_v2.ts`, `app-server-protocol.ts`, `websocket/app-server.ts`, `backend/local/local-store.ts`, `transcript-migration.ts`, `system-prompt-compilation.ts`, `agent/memory-*.ts`, `cron/`, `channels/`, `mods/`, `hooks/`, `skills/`, `telemetry/` **do** resolve — the correct pointers cluster in tasks 03, 05–08.

### 2d. Structural links

`plan.md ↔ task ↔ certificate` links all resolve; `.specs/README.md` §Plans lists the plan folder correctly. Every certificate links same-directory to its task and `../plan.md`; every task header carries its `**Certificate:**` line. **This layer is correct.**

---

## Checkpoint 3 — Forward coverage (spec → task)

Every non-boilerplate `##`/`###` section of every in-scope page was tested for a task naming it:

```
in-scope sections: 100   named by no task: 57
```

| Page | Uncovered | Notable |
|---|---|---|
| `03-runtime-and-turns.md` | **10 / 10** | entire page |
| `06-model-providers.md` | **9 / 9** | entire page |
| `01-domain-model.md` | 18 / 25 | Agent, Conversation, Run, Approval, Queue item, Turn and lease, Connection, Listener runtime, External tool registration, Relationships, all three State machines, Required query patterns |
| `07-channels-and-operations.md` | 8 / 10 | Account and routing model, Channel command surface, Deployment image, Health and readiness, Shutdown and restart, Observability and security, Operational bounds, Compatibility with letta-app-server-deployment |
| `02-app-server-api.md` | 5 / 8 | Listener configuration, Core lifecycle, HTTP API, Transport bounds, Errors and closure |
| `05-tools-and-extensions.md` | 5 / 9 | Execution pipeline, Permissions and sandbox, External tools and MCP, Hooks and mods, Limits |
| `04-persistence-and-memfs.md` | 2 / 11 | State outside the backend root, Baseline-compatible loading |

Two pages being **100 % uncovered** is mechanical fallout from §2b — tasks 09–17 and 13/18–20 clearly *intend* to cover them but cite phantom targets. That does not soften the finding: the checklist's forward-coverage gate is unrunnable, and a spec-builder agent following `Implements:` has nothing to open.

Beyond the anchor breakage, these are **substantive** gaps with no task at all:

- **Entire WebSocket command groups** from `02 §WebSocket command groups`: Teleport (`teleport_probe`/`teleport_request`/`teleport_failed` + `input.kind = teleport_continue`), Terminal (`terminal_spawn`/`input`/`resize`/`kill`, the two-second Strict-Mode reuse window), Files, Memory commands, Models/providers commands, Schedules commands, Skills enable/disable, Agent management, Conversation management (incl. `fork`, `recompile`, `compact`), Settings (cwd map, reflection, experiments), Device commands (slash/mod command, remove queue item, branch search/checkout, secret list/apply), Introspection (`app_server_info`). Task 31 covers only generic decode/route/emit.
- `01 §Required query patterns` — all 10 required query behaviors.
- `02 §Transport bounds` — 9 named constants, none claimed by any task.
- `05 §Execution pipeline` — the ordered hook→permission→sandbox→secret-substitution→executor→post-hook→scrub→clamp→persist→emit chain.
- `06 §Streaming invariants` and `06 §Context and compaction` (effective window = min of four sources).
- `07 §Channel command surface` — the 12 shared operational commands and 4 push events.
- No task creates `fixtures/persistence/`, `fixtures/providers/`, or `fixtures/reference-traces/` from `architecture-principles.md §Workspace layout`.

---

## Checkpoint 4 — Reverse coverage (task → authorizing spec section)

All 46 tasks carry an `Implements:` line, but 34 of them are authorized only by anchors that do not resolve. Beyond that, the following task content is **not authorized by any canonical section** and in several cases contradicts one:

| Task | Invented scope | Canonical position |
|---|---|---|
| 13 | retry with **jitter**; `ProviderEvent {TextDelta, ToolCallDelta, ToolCallComplete, Usage, Done, Error}`; `ProviderError {rate_limited, timeout, invalid_request, server_error}`; `ModelDescriptor {provider, model_id, display_name, context_window, max_output_tokens}` | `06 §Retry and fallback` "The pinned provider-turn retry path has no jitter"; `06 §Normalized provider port` lists `TextDelta / ReasoningDelta \| RedactedReasoning / ToolCallStart \| ToolCallArgumentsDelta \| ToolCallEnd / Usage / ProviderMetadata / Stop / Error` and **12** error kinds; schema `$defs.ModelDescriptor` = `{handle, provider_id, available, context_window, model_settings}` |
| 14 | parallel tool execution as a decided design; `TOKENS_PER_TURN_MAX`; event names `step-started`/`step-completed` | `03 §Assumptions and open questions`: "*Parallel tools.* Which built-in tools are certified parallel-safe…" is an **open question**; no token-per-turn bound exists; wire events are `stream_delta`/`update_loop_status`/`turn_finished` |
| 15 | approval timeout **auto-denies** | `03 §Runtime bounds` + Decision: "*Approval timeout.* Lotta interrupts after 24 hours … must produce a recoverable terminal state **rather than an implicit denial**" |
| 24 | `core_memory_append/replace/delete`, `archival_memory_insert/search`, `memory_recall`, `create_plan/get_plan/list_plans`, `get_definition/get_references` | `05 §Rust built-ins`: Memory = "memory edit and patch operations with Git commit and path confinement"; Planning = "`update_plan`/`UpdatePlan`, `TodoWrite`, and task create/get/list/update/output/stop"; LSP = "`ReadLSP` diagnostics" only. None of the invented names exist in baseline `tools/impl/` |
| 30 | auth via `Authorization` header (HTTP) and `Sec-WebSocket-Protocol` (WS); blanket loopback exemption | `02 §Listener configuration`: `--ws-auth capability-token\|signed-bearer-token`, `--ws-token-file`, `--ws-token-sha256`, `--ws-shared-secret-file`, `--ws-issuer`, `--ws-audience`, `--ws-max-clock-skew-seconds`; §Responsibilities item 2 requires authenticating **Origin-bearing** clients too |
| 34 | 21 commands (`create-account`, `send-message`, `subscribe`, `health-check`, `get-diagnostics`, …) | `07 §Channel command surface`: "exactly 20 commands", enumerated; confirmed verbatim at `types/service-protocol.ts:7-27` |
| 36 | JSON/TOML config schema, `LOTTA_` env prefix, `REDACTION_FIELDS` | No canonical page defines configuration or telemetry; the task's own target page does not exist |
| 37 | `SIGHUP` config reload | `07 §Shutdown and restart` defines an 8-step SIGTERM/SIGINT sequence and no reload |
| 45 | passphrase-based backup encryption, `BACKUP_ENCRYPTION_ALGORITHM` | `04 §Backup and restore` says "operator-supplied key"; the encryption mechanism is an **open question** (`04 §Open questions`, *Live credential encryption*) |
| 31 | sync = replay missed events from last received event ID | `02 §Event envelopes and ordering`: `idempotency_key` is "**not stable across replay**"; "State updates are **snapshots, not diffs**"; `02 §Core lifecycle`: `sync` "replay authoritative snapshots" |

---

## Checkpoint 5 — Decomposition and reviewability

**Sizing.** `task-decomposition.md` §Sizing: "A package's DoD has more than ~6 acceptance items → probably two packages wearing one id." Measured DoD item counts:

```
min 7 · max 14 · median 11.5 · tasks over 6 items: 46 / 46
```

Worst: 12 (14), 33/36/39/40/41/42/46 (13), fourteen tasks at 12.

**Multi-subsystem packages** (the "touches three unrelated subsystems → split" rule): task 24 bundles memory + planning + worktree + interaction + LSP (five families, five files); task 23 bundles all file tools with all shell/process tools; task 19 bundles four provider adapters plus a sidecar; task 36 bundles config + telemetry + health across three crates; task 12 resolves seven independent subsystems.

**Horizontal slicing.** The graph is a textbook layer cake: domain → protocol/testkit → store → memfs → runtime → provider adapters → tools → extensions → transport → ops → conformance. Nothing is exercisable end to end before task 31, and nothing runs as a binary before task 37 — 37 of 46 tasks land before the first demonstrable path. M1's "demonstrable when complete" is *"`cargo build` and `cargo nextest` pass"*, i.e. a green build, not a reviewable behaviour. `plan.md` §Decisions asserts *"The plan follows a vertical-slice spine"*; the DAG does not support that claim. This is precisely what `spec-planner` §What NOT to do forbids: *"Don't order by layer when you can order by reviewable slice."*

A vertical alternative is available and cheap: a "thin turn" slice (minimal domain + minimal store + transport + `runtime_start`/`input`/`stream_delta`/`turn_finished` against a fake provider) would make **every** later task reviewable through a real client from roughly task 6 onward.

---

## Checkpoint 6 — Dependency and milestone proof

**Mechanically verified clean:**

```
table rows: 46          table edges: 79        mermaid edges: 79
mermaid − table: ∅      table − mermaid: ∅     cycles: none
edges with dep ≥ own number: none
table numbers with no file: none    files with no table row: none
tasks missing a certificate: none   certificates with no task: none
task-file "Depends on" vs table: all 46 agree
```

Graph *mechanics* are the strongest part of this plan.

**Graph *semantics* are not.** Missing edges, each contradicted by the dependent task's own DoD:

| Missing edge | Evidence |
|---|---|
| 11 → 33 | 33 DoD: "Schedule firing enqueues cron_prompt **through the input queue**"; 33 depends on 06, 09 only |
| 20 → 45 | 45 DoD: "auth.json **encrypted** in backup"; 45 depends on 05, 06, 08 only |
| 20, 33, 35 → 39 | 39 DoD covers "Provider auth.json", "Schedule", "Channel account/route" round-trips; 39 depends on 05, 06, 07 only |
| 15, 16, 11 → 31 | 31 must emit `control_request`, `turn_finished(cancelled)`, `update_queue`; depends on 09, 14, 30 only |
| 28, 29 → 40 | 40 DoD: "Sidecar crash: cleanup and error on **mod/subagent** crash"; depends on 16, 31, 37 only |
| 20, 36 → 41 | 41 DoD: "credentials, api_keys, tokens redacted from logs"; depends on 22, 30, 37 only |
| 39–45 → 46 | 46 DoD: RELEASE-CHECKLIST covers "conformance suites green"; depends on 37 only |

**Invalid ordering (unfixable under the append-only rule).** Task 12 (turn setup) DoD includes *"Toolset resolution resolves enabled tools from agent config, toolset, and permissions"* and task 14 (provider/tool loop) DoD includes *"Tool call execution **dispatches to ToolPort**"*. `ToolPort` and `ToolRegistry` are produced by task **21**; permissions by **22**. Both required edges point from a **higher** number to a lower one, so they were silently dropped rather than drawn — and adding them would violate the plan's own stated invariant. The order 12/14 before 21/22 is not a valid topological order of the real dependency graph. Because numbers are append-only *once the plan is shared*, and this plan is still `Draft` with zero tasks executed, renumbering is still permitted (`checklist.md` §When the checklist finds a problem, item 6) — but only now.

**Spurious edge.** 18 → 32: the OpenAI HTTP routes serve *agents* (`02 §HTTP API`: "An agent's unique, non-colliding name is its advertised model ID"), not the OpenAI *provider* adapter. No stated justification.

**Milestone gates.** M1–M5 gates are mechanically checkable and reasonable in form. M6's gate — *"release checklist signed off"* — is not a demonstrable gate. More seriously, M1's demonstrable state is a green build and M2's is a fake-provider turn with no client attached, so the **first milestone at which any compatibility claim from `00 §Compatibility definition` can be exercised is M4**, and the first at which any of the six `00 §Implementation acceptance` criteria can be tested is **M6**.

---

## Checkpoint 7 — Definition-of-done audit (all 46)

**Present and well-formed everywhere:** every task has `Implements`, `Depends on`, `Produces`, `Pointers`, `## Steps`, `## Definition of done`; every DoD ends with a `Reviewable:` line; every DoD carries the repo-baseline item referencing `plan.md`. No `Status:` field anywhere. Voice is clean — zero instances of "easily/simply/just/powerful/robust/seamlessly", no emoji, no exclamation points, no calendar dates or estimates in bodies.

**Positive / negative / boundary coverage:** every task carries explicit positive, negative, and boundary DoD items. This satisfies `development-guidelines.md` §Testing ("Every success path has negative-space tests"; "Every limit has below/at/above tests") in *form*.

**Where the DoDs fail:**

1. **They test invented constants, not spec constants.** Roughly 30 tasks name bounds that exist nowhere in the canonical set, while the spec's actual named bounds go untested:

| Task | DoD constant | Canonical constant it displaces |
|---|---|---|
| 09 | `RUNTIME_IDLE_EVICT_MS` | *(none — spec: evict **immediately** once quiescent)* |
| 10 | `LEASE_GENERATION_MAX` | *(none)* |
| 11 | `CONVERSATION_QUEUE_SOFT_MAX` / `_HARD_MAX` | `QUEUE_ITEMS_SOFT_MAX` (100) / `QUEUE_ITEMS_HARD_MAX` (300) |
| 13 | `PROVIDER_RETRY_MAX`, `PROVIDER_RETRY_BACKOFF_MS`, `PROVIDER_TIMEOUT_MS` | `PROVIDER_RETRIES_MAX`, `PROVIDER_BACKOFF_MS_MAX`, `PROVIDER_TIMEOUT_MS_DEFAULT` |
| 14 | `STEPS_MAX`, `TOKENS_PER_TURN_MAX` | `TURN_STEPS_MAX`, `TURN_TOOL_CALLS_MAX`, `TURN_PROVIDER_RETRIES_MAX`, `TURN_EMPTY_RESPONSE_RETRIES_MAX`, `CONTEXT_OVERFLOW_COMPACTIONS_MAX` |
| 15 | `APPROVAL_TIMEOUT_MS` | `APPROVAL_WAIT_MS_MAX` |
| 16 | `CANCELLATION_COOPERATIVE_TIMEOUT_MS` | `TURN_CANCEL_GRACE_MS` (+ 2,000 ms shell SIGTERM→SIGKILL grace) |
| 17 | `COMPACTION_TOKENS_THRESHOLD`, `COMPACTION_SUMMARY_TOKENS_MAX` | `CONTEXT_OVERFLOW_COMPACTIONS_MAX` |
| 20 | `CONNECTIONS_MAX` | `PROVIDERS_MAX` (128) — `CONNECTIONS_MAX` (1,024) is the **WebSocket** cap |
| 21 | `TOOL_TIMEOUT_MS` | `TOOLS_LOADED_MAX`, `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT` |
| 22 | `SANDBOX_ROOT_MAX_DEPTH`, `PERMISSION_RULES_MAX` | *(none)* |
| 23 | `FILE_SIZE_MAX`, `SHELL_OUTPUT_MAX_BYTES`, `SEARCH_RESULTS_MAX` | `TOOL_INPUT_BYTES_MAX`, `TOOL_RESULT_BYTES_MAX`, `TOOL_RESULT_MODEL_CHARS_MAX`, `CHILD_PROCESS_OUTPUT_BYTES_MAX` |
| 25 | `MCP_SERVERS_MAX`, `MCP_CALL_TIMEOUT_MS` | `MCP_SERVERS_PER_AGENT_MAX`, `MCP_TOOLS_PER_SERVER_MAX` |
| 26 | `SKILLS_MAX`, `SKILL_FILE_MAX_BYTES` | *(none)* |
| 27 | `HOOK_TIMEOUT_MS`, `HOOKS_MAX` | `COMMAND_HOOK_TIMEOUT_MS_DEFAULT`, `PROMPT_HOOK_TIMEOUT_MS_DEFAULT`, `HOOKS_PER_EVENT_MAX` |
| 29 | `SUBAGENTS_MAX` | `SUBAGENTS_PER_PARENT_MAX`, `SUBAGENTS_CONCURRENT_PER_PARENT_MAX` |
| 30 | `CONNECTION_HEARTBEAT_TIMEOUT_MS` | `WS_PING_INTERVAL_MS`, `WS_PONG_TIMEOUT_MS`, `WS_FRAME_BYTES_MAX`, `WS_MESSAGE_FIELDS_MAX`, `REQUEST_ID_BYTES_MAX`, `AUTH_CLOCK_SKEW_SECONDS_MAX` |
| 31 | `CONNECTION_SEND_QUEUE_MAX` | `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` |
| 32 | `HTTP_REQUEST_BODY_MAX_BYTES` | `HTTP_BODY_BYTES_MAX`, `OPENAI_CHAT_KEYS_MAX`, `CHAT_IDEMPOTENCY_OUTCOMES_MAX` |
| 33 | `SCHEDULES_MAX`, `SCHEDULER_POLL_INTERVAL_MS` | `SCHEDULE_RUN_LOG_KEEP_LINES`, `SCHEDULE_RUN_LOG_BYTES_MAX` |
| 35 | `PAIRING_CODE_EXPIRY_MS`, `ROUTES_PER_ACCOUNT_MAX` | `PAIRING_TTL_SECONDS` (900), `PAIRINGS_PENDING_PER_CHANNEL_MAX` (50), `CHANNEL_ROUTES_MAX`, `CHANNEL_ACCOUNTS_MAX` |
| 37, 38 | `SHUTDOWN_TIMEOUT_MS`, `SUPERVISOR_MAX_RESTARTS` | `SHUTDOWN_GRACE_MS`, `SIDECAR_RESTARTS_PER_HOUR_MAX` |

Several also violate `development-guidelines.md` §Naming ("Units come last"): `SHELL_OUTPUT_MAX_BYTES`, `SUBAGENT_STATE_SNAPSHOT_MAX_BYTES`, `HTTP_REQUEST_BODY_MAX_BYTES`, `SANDBOX_ROOT_MAX_DEPTH`, `SKILL_FILE_MAX_BYTES`, `BACKUP_MAX_BYTES`, `SUPERVISOR_MAX_RESTARTS`, `CONFIG_FILE_MAX_BYTES`.

2. **Boundary items are frequently unfalsifiable.** "Boundary: precedence at boundary" (26), "coalesce at boundary" (11), "streaming at boundary" (18, 32, 44), "transition at boundary" (10), "each tool at its specific limits" (24), "sync at event boundary" (31). A validator cannot construct the input from these.

3. **Missing integration/conformance obligations where the spec demands them.** No DoD anywhere requires cross-runtime evidence for tasks 05–08 despite `development-guidelines.md` §Definition of done ("persistence changes pass TypeScript↔Rust round-trip tests"); no DoD requires the protocol fixture regeneration-is-clean check from §Repository hygiene; no DoD in 09–17 requires the six ordering invariants of `02 §Event envelopes and ordering`; no DoD requires an assertion-density or 70-line function check despite it being an explicit baseline item.

4. **`Reviewable:` lines are concrete and runnable** in 44 of 46 (a `cargo nextest` invocation with a filter) — genuinely good — but for 38 (`docker build`/`docker-compose up`) and 46 (`cargo doc`) they are the only end-to-end exercises in the plan. Tasks 42/43 claim "Desktop connects…" / "Agent SDK creates…" but their `Reviewable:` line is `cargo nextest --test desktop_smoke_test`, which does not establish that a real client drove a real binary — the actual criterion in `00 §Compatibility definition`.

---

## Checkpoint 8 — Done-certificate audit (all 46)

**Mechanically correct across all 46:**

```
obligation count == DoD item count:        46/46 (517 obligations, one-to-one, in DoD order)
last obligation is the Reviewable item:    46/46
Status fields all "☐ unverified":          517/517
VERDICT / CONFIDENCE / SUMMARY blank:      46/46
State: "Authored 2026-08-14 — unverified": 46/46
Task ↔ certificate links resolve both ways:46/46
Regression check + Residue sections present:46/46
```

The blank-authored-state and one-to-one-ordering obligations from `done-certificates` are fully met. That is real and worth stating plainly.

**The evidence layer is where the certificates fail — and it fails exactly where the review brief anticipated:**

```
obligations with evidence = "run the tests and checks described
        in task NN's DoD item ON":         408 / 517  (79%)
certificates with zero  *Claim:*  fields:   45 / 46
certificates with zero  *Checks:* fields:   46 / 46
certificates asserting "greenfield —
        no existing callers to regress":    46 / 46
certificates with Residue "None noted":     46 / 46
```

- **Boilerplate evidence.** `13-provider_port-certificate.md:23,27,31,…` and every certificate 09–46 discharge each obligation with *"run the tests and checks described in task NN's DoD item ON"*. That is a pointer back to the DoD, which is the artifact the certificate exists to make checkable. Two validators cannot collect the same evidence from it, because it names no file, no test, and no trace. `done-certificates` §What NOT to do: *"'Verify it works' is not an obligation. Name the file, the test, the trace, the caller."*
- **Sharp quality cliff at task 09.** Certificates 01–08 carry real instructions — `01:24` ("run `cargo build --workspace 2>&1` and confirm exit code 0 … read `rust-toolchain.toml` and confirm it pins stable with edition 2024"), `02:38` ("grep `crates/lotta-domain/src/bounds.rs` for numeric literals; confirm each bound is a `const` … run the `no-magic-numbers` lint check"), `06:22` ("run `cargo nextest -p lotta-store -- manifest` … with schema_version 2, pi-session-entry-jsonl, pi-ai"). From 09 onward every certificate degrades to the template string. The generation clearly ran out of authored reasoning at task 09 and continued producing well-formed shells.
- **`*Claim:*` missing in 45 of 46.** Only `01` has them. The certificate template makes Claim mandatory per obligation ("a **Claim** — the checkable condition the item asserts").
- **`*Checks:*` absent in 46 of 46.** Checkpoint 1 of `semiformal-method.md` — the five-step function-resolution sequence that exists to defeat name shadowing — is instantiated nowhere in this plan. Yet the plan has obvious shadow risks its own tasks create: task 21 defines a `ToolError::timeout` and task 13 a `ProviderError::timeout`; tasks 13/18/19 all define `PROVIDER_TIMEOUT_MS` across three crates; `CONNECTIONS_MAX` is defined in `01 §Resource bounds` for WebSocket connections and re-used in task 20 for provider connections. These are precisely the cases `Checks:` exists for.
- **Regression checks are template defaults everywhere.** All 46 say "greenfield". That is defensible for 01–08 and arguably 13/21/30/34. It is **wrong** for at least: 15, 16, 17 (extend the turn loop built in 14), 22, 23, 24, 25, 27, 28, 29 (extend the registry built in 21), 31, 32 (extend the transport built in 30), 33 (enqueues through 11's queue), 37 (composes every prior adapter), 39–45 (exercise 05–38). `certificate-template.md` §Notes: *"Whenever the task modified existing code, name at least one downstream caller."* Task 16's certificate in particular asserts no callers while its own DoD changes `turn_finished` emission that task 14 already produces.
- **Certificates faithfully inherit every DoD defect.** `13-…-certificate.md:42` obligates a validator to confirm *"exponential backoff **with jitter**"*, and `:34` to confirm a `ModelDescriptor` shape that contradicts `canonical-types.schema.json`. `34-…-certificate.md:22` obligates "All **20** … commands" against a task listing 21. A validator discharging these certificates faithfully would certify a parity break as DONE.

---

## Checkpoint 9 — Feasibility and integration-risk audit

**Fixtures do not precede their dependents.** `00 §Compatibility definition` and `architecture-principles.md §Compatibility architecture` make a checked-in fixture corpus the primary evidence — "Prose and Rust types alone cannot prove wire parity". The plan extracts **only** protocol discriminants early (task 03). Persistence fixtures are not extracted until task **39**; provider stream fixtures are never extracted by any task (task 19 asserts "equivalent normalized traces" with no corpus behind it); OpenAI golden fixtures wait until **44**. So tasks 05–08 (agent/conversation/transcript/migration/MemFS/prompt) and 18–20 (three provider dialects) are built against spec prose, with the artifacts that could falsify them arriving 30+ tasks later. If the `system-prompt.json` shape, the base64url conversation-key forms, or the SSE event ordering is wrong, the plan discovers it at M6.

**Architectural-drift gates arrive too late.** Ordered by the earliest task that could detect each class of drift: cross-runtime persistence → 39; failure injection (queue/lease/cancellation/recovery) → 40; security (auth, path confinement, redaction, sandbox) → 41; SDK conformance → 42; Desktop → 43; OpenAI → 44. Every one of the six `00 §Implementation acceptance` criteria is gated behind M6. Task 30 already encodes a wrong auth model; task 41 is the first thing that would notice, 11 tasks later, after 30/31/32/37 have all been built on it.

**Prospective module boundaries violate `architecture-principles.md` §Dependency graph.**

- The rule: *"`lotta-runtime` depends on domain and port traits, **never concrete adapters**"* and *"Adapter crates … not each other."*
- Task 13 places `ProviderPort` at `crates/lotta-providers/src/port.rs`; tasks 18/19 then add `openai.rs`, `anthropic.rs`, `ollama.rs`, `lmstudio.rs`, `llamacpp.rs`, `piai.rs` to that **same** crate. Task 14 (`crates/lotta-runtime`) depends on 13 → `lotta-runtime` links a crate containing concrete provider adapters.
- Identically, task 21 places `ToolPort` at `crates/lotta-tools/src/port.rs`; tasks 23/24 add `builtin/file.rs`, `builtin/shell.rs`, `builtin/memory.rs`, … to the same crate; task 14's DoD dispatches to `ToolPort`.
- Neither trait is placed in `lotta-domain` or a dedicated ports crate, and `plan.md` §Decisions ("*Provider port placement.* Port trait defined in task 13 before the loop in task 14 … The runtime depends on the port trait, not on concrete adapters") asserts an outcome the crate layout does not produce.

**Composition-root contradiction.** `architecture-principles.md` §Workspace layout puts `src/main.rs` at the workspace root, "composition root only", and task 01 creates it there. Task 37's `Pointers` say `crates/lotta-app-server/src/main.rs`. One of the two is wrong, and task 38 (Dockerfile) inherits the ambiguity.

**Sidecar feasibility is asserted, not scheduled.** `06 §Provider classes` pins the compatibility host to `@earendil-works/pi-ai` `0.82.1` over length-prefixed JSON pipes; `05 §Subagents` requires a versioned bounded sidecar around a pinned Letta Code subprocess; `05 §Hooks and mods` requires a versioned JSON-RPC mod host. Tasks 19, 28, 29 each independently define "a sidecar protocol" with no shared protocol task and no cross-references — three incompatible sidecar contracts by construction, all crossing a process boundary that `development-guidelines.md` §Where to validate ("Child/sidecar → host: frame length, protocol version, owner identity, capability and timeout") treats as a first-class validation boundary.

**Blocking open questions from the spec are not surfaced in the plan.** `plan.md` §Open questions lists five. The canonical set carries roughly fifteen, and at least five of them block scheduled tasks and are silently decided instead: parallel-tool safety (`03`, decided by 14/23), Origin policy and TLS termination (`02`, blocks 30/41), live credential encryption (`04`/`06`, decided by 45), approval-expiry configurability (`03`, decided by 15), Rust-vs-TypeScript schema source of truth (`architecture-principles`, blocks 03/39). Conversely, plan open question *"CI provider … (Blocks task 01 CI configuration.)"* blocks a DoD item that task 01 nonetheless requires ("CI workflow files exist and reference the correct commands").

---

## Checkpoint 10 — Findings

### BLOCKERS

**B1 — 24 `Implements` anchors target two spec files that do not exist.**
*Anchors:* tasks `09,10,11,12,14,15,16,17,23,27,31,40` → `.specs/03-runtime.md`; task `36` → `.specs/06-configuration-and-telemetry.md`.
*Evidence:* filesystem resolution — the canonical set contains `03-runtime-and-turns.md` and `06-model-providers.md`; no `03-runtime.md` or `06-configuration-and-telemetry.md` exists. None of the 21 claimed section names (`§Runtime scoping`, `§Turn state machine`, `§Agent loop`, `§Approval flow`, …) exists on `03-runtime-and-turns.md` either.
*Impact:* the twelve tasks constituting the entire core runtime, plus config/telemetry, have **no** authorizing spec text. A spec-builder agent opening `Implements:` gets a file-not-found and builds from the task prose alone — which is where every parity break in B4/B6/M1–M18 lives.
*Remediation:* repoint tasks 09–17, 23, 27, 31, 40 at `../../../03-runtime-and-turns.md` with real headings (`§Runtime registry`, `§Lifecycle owner`, `§Input and queue flow`, `§Turn setup`, `§Provider and tool loop`, `§Approvals`, `§Compaction and prompt refresh`, `§Cancellation and terminal behavior`, `§Runtime bounds`, `§Observability`). For task 36, either author a canonical configuration/telemetry page via spec-creator or repoint at `architecture-principles.md §Cross-cutting conventions` + `07 §Health and readiness` + `07 §Observability and security` and cut the unauthorized config-format scope.

**B2 — 89 of 139 §anchors (34 of 46 tasks) do not resolve, leaving 57 of 100 in-scope spec sections unmapped.**
*Anchors:* full table in Checkpoint 2b; coverage table in Checkpoint 3.
*Evidence:* mechanical heading match against every `##`/`###` in the canonical set plus `$defs`. `03-runtime-and-turns.md` and `06-model-providers.md` are **100 %** unmapped.
*Impact:* `spec-planner` Phase 5 step 4 and the checklist §Coverage gate cannot be run. Neither forward nor reverse coverage is demonstrable, so the plan cannot be shown to implement the spec.
*Remediation:* rewrite every `Implements:` line against the heading inventory in Checkpoint 2b, then re-run the coverage check until zero broken anchors and zero uncovered in-scope sections remain; record genuine residual gaps in `plan.md` §Open questions.

**B3 — 21 of 47 baseline pointers into the pinned checkout do not exist, including the one cited by ten tasks.**
*Anchors:* `../letta-code/src/websocket/listener/agent-runner.ts` (tasks 09, 10, 12, 14, 15, 16, 17, 40); `agent-runner-queue.ts` (11); `src/providers/{base,openai,anthropic,ollama,lmstudio,auth}.ts` (13, 18, 19, 20); `src/tools/{base,registry,permissions,sandbox}.ts` (21, 22, 41); `src/tools/impl/{file,mcp}.ts` (23, 25); `src/http/`, `src/http/openai.ts` (30, 32, 44); `src/sdk/` (42); `src/desktop/` (43); `src/config/` (36); `src/backend/local/backup.ts` (45).
*Evidence:* filesystem resolution at `300f923f`; real anchors enumerated in Checkpoint 2c.
*Impact:* every task whose job is to reproduce baseline behaviour points at a file that cannot be read. Combined with B1, tasks 09–17 have neither a spec anchor nor a code anchor — the parity claim is unbacked at both ends. This is the direct cause of the fabricated tool names (M12), hook events (M14), and channel commands (B5).
*Remediation:* replace each with the verified anchors in Checkpoint 2c; add a `Pointers`-resolution check to the plan's own review pass, since these are cheap to verify with `test -e`.

**B4 — Task 30 specifies an authentication model that contradicts the spec and the baseline, and drops Origin-based rejection.**
*Anchors:* `backlog/30-transport_auth.md:13-15,26-27`; contradicts `.specs/02-app-server-api.md §Listener configuration` (L 27-41) and §Responsibilities item 2.
*Evidence:* task specifies "API key or token from `Authorization` header for HTTP, `Sec-WebSocket-Protocol` for WS" and "loopback exemption: allow unauthenticated connections from 127.0.0.1". The spec specifies `--ws-auth capability-token|signed-bearer-token`, token file **or** precomputed SHA-256 (never both), HS256 with mandatory `exp`, optional `nbf`, configured issuer/audience, and a 300 s skew ceiling. Baseline confirms: `websocket/app-server-auth.ts:11` `type WebsocketAuthCliMode = "capability-token" | "signed-bearer-token"`; `websocket/app-server.ts:332` rejects **unauthenticated Origin-bearing requests even on loopback**.
*Impact:* the SDK/Desktop conformance suites (42/43) and the security suite (41) will fail against a server built to this task, and the blanket loopback exemption reintroduces the browser-origin attack the baseline explicitly closes — on a server that `00 §Non-goals` describes as exposing shell and filesystem capabilities.
*Remediation:* rewrite task 30 steps and DoD from `02 §Listener configuration` verbatim: both auth modes, the four secret-file/digest flags, issuer/audience/skew validation with `AUTH_CLOCK_SKEW_SECONDS_MAX`, `/ws` default path with URL-path override and `/` acceptance, bare `--listen` → `ws://127.0.0.1:0`, non-loopback-without-auth failing **before** listening, and Origin-bearing rejection. Add `WS_FRAME_BYTES_MAX`, `WS_MESSAGE_FIELDS_MAX`, `WS_PING_INTERVAL_MS`, `WS_PONG_TIMEOUT_MS`, `REQUEST_ID_BYTES_MAX`, `HTTP_BODY_BYTES_MAX` to its DoD.

**B5 — Task 34 fabricates the channel management protocol.**
*Anchors:* `backlog/34-channels_protocol.md:12` (21 invented commands), `:25` (DoD says 20); contradicts `.specs/07-channels-and-operations.md §Channel command surface` (L 87).
*Evidence:* the task lists `create-account, get-account, update-account, delete-account, list-accounts, create-route, get-route, update-route, delete-route, list-routes, send-message, get-message, list-messages, subscribe, unsubscribe, pair-channel, unpair-channel, get-channel-info, list-channels, health-check, get-diagnostics` — 21 kebab-case names. The baseline defines exactly 20, snake_case, at `types/service-protocol.ts:7-27`: `channels_list, channel_accounts_list, channel_account_create/update/bind/unbind/delete/start/stop, channel_get_config, channel_set_config, channel_start, channel_stop, channel_pairings_list, channel_pairing_bind, channel_routes_list, channel_targets_list, channel_target_bind, channel_route_update, channel_route_remove`. Not one invented name appears in the baseline. The task is also internally inconsistent (steps enumerate 21, DoD asserts 20).
*Impact:* `00 §Compatibility definition` requires "Existing ChannelGateway client drives a Rust App Server runtime through pairing, routing, turn, and reply fixtures". A server built to task 34 shares zero command names with that client. Task 35 inherits the fabrication.
*Remediation:* replace the step list with the 20 verbatim command names; add the four push events (`channels_updated`, `channel_accounts_updated`, `channel_pairings_updated`, `channel_targets_updated`) as messages, not commands; add the `publish_runtime_tools` / `release_runtime_tools` / `/channels` slash-command control-plane surface from `07 §Process topology`.

**B6 — Task 13 mandates retry jitter, a labelled parity break, on the port every provider inherits.**
*Anchors:* `backlog/13-provider_port.md:17,30`; `13-…-certificate.md:42`; contradicts `.specs/06-model-providers.md §Retry and fallback` (L 108) and §Decisions *Retry timing* (L 154), and `.specs/03-runtime-and-turns.md §Provider and tool loop` (L 129).
*Evidence:* task DoD: "Runtime-owned retry policy: exponential backoff **with jitter**, max retries, timeout". Spec: "The pinned provider-turn retry path has no jitter"; "*Retry timing.* **No jitter at the pinned baseline.** Exact retry traces use retry-after, exponential transient/busy delay, linear empty-response delay, and the 60-second cap." `development-guidelines.md` §Errors: "Jitter appears only when the owning component spec requires it." Baseline corroborates: `grep -rn jitter` finds it only in `websocket/listen-register.ts:214,253` (Cloud register), never in the provider-turn path.
*Impact:* `00 §Compatibility definition` requires reliability traces to match ("Queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery traces match"). Jitter makes retry traces nondeterministic and unmatchable; because it is specified at the **port**, tasks 14, 18, 19, 32 all inherit it. Task 13's certificate obligates a validator to *confirm* the break.
*Remediation:* strike "with jitter"; specify the four-part policy from `06 §Retry and fallback` (retry-after handling, capped exponential for transient/busy, linear empty-response delay, total deadline) with `PROVIDER_RETRIES_MAX` = 3 and `PROVIDER_BACKOFF_MS_MAX` = 60,000; add a DoD item asserting deterministic retry timing under a fake clock.

**B7 — 79 % of certificate obligations carry pointer-back-to-the-DoD evidence; the function-resolution checkpoint is absent from all 46.**
*Anchors:* `backlog/*-certificate.md` for tasks 09–46; representative `13-…-certificate.md:23,27,31,35,39,43,47,51,55,59,63`; `34-…-certificate.md:23-64`; `16-…-certificate.md:23,27,31,35`.
*Evidence:* 408 of 517 obligations read *"run the tests and checks described in task NN's DoD item ON"*; 45 of 46 certificates contain zero `*Claim:*` fields; 46 of 46 contain zero `*Checks:*` fields; 46 of 46 declare "no existing callers in scope — greenfield"; 46 of 46 Residue read "None noted at authoring". Certificates 01–08 by contrast name concrete commands, files, and expected results (`01:24,29,34,39,44,49,54,59`; `02:38`; `06:22,45`).
*Impact:* a done certificate exists so a validator "cannot skip an obligation or call a task done without producing the evidence the certificate demands". A certificate that says "see the DoD" delegates the question back to the artifact under test — two validators will not collect the same evidence, and `validate-done-certificate` cannot discharge it. The absent `Checks:` field means the name-shadowing defence is nowhere applied, despite the plan creating real shadow candidates (`timeout` on both `ToolError` and `ProviderError`; `PROVIDER_TIMEOUT_MS` in three crates; `CONNECTIONS_MAX` overloaded across two meanings). The uniform "greenfield" regression claim is provably wrong for ~30 tasks that extend earlier tasks' code.
*Remediation:* re-author certificates 09–46 with done-certificates at the quality bar of 01–08: per obligation a `*Claim:*`, a `*Evidence to collect:*` naming the exact test filter / file:line / trace input, and `*Checks:*` wherever the claim depends on a resolvable call. Replace the regression stanza on every task whose `Depends on` is non-empty with at least one named downstream unit from the depended-on task.

**B8 — The stated implementation order is not a valid topological order: tasks 12 and 14 require tasks 21 and 22.**
*Anchors:* `backlog/12-turn_setup.md:19,34` ("Toolset resolution resolves enabled tools from agent config, toolset, and permissions"); `backlog/14-provider_tool_loop.md:14,27` ("Tool call execution dispatches to **ToolPort**"); `ToolPort`/`ToolRegistry` produced by `backlog/21-tool_registry.md:12,16`; permissions by `22`. `plan.md` dependency table rows 12 and 14 list only 08/09/11 and 10/11/12/13.
*Evidence:* the required edges 21→12, 22→12, 21→14 all point from a higher number to a lower one and are absent from both the table and the Mermaid graph. `plan.md` L 204-206 asserts the invariant "`Depends on` references **lower** task numbers … if a row depends on a higher number, either the order or the dependency is wrong."
*Impact:* task 12 and 14 cannot be built as scheduled; a spec-builder walking the DAG in waves will dispatch them into a wave where their dependency does not exist. Also true of `03 §Turn setup` steps 7–9, which require skills (26), mods and hooks (27/28) during setup.
*Remediation:* the plan is `Draft` with every task still in `backlog/`, so renumbering is still permitted under `checklist.md` §When the checklist finds a problem, item 6. Move the tool registry and permissions ahead of turn setup and the provider/tool loop, then re-derive the order. Renumber **now**, before any task leaves `backlog/`.

### MAJORS

**M1 — Task 15 auto-denies on approval timeout.** `15-approvals.md:14,27` vs `03 §Runtime bounds` (`APPROVAL_WAIT_MS_MAX` 86,400,000, "interrupt the turn and replay an explicit expired approval state") and §Decisions *Approval timeout* ("must produce a recoverable terminal state **rather than an implicit denial**"). An implicit denial silently changes conversation content in a persisted transcript. Rewrite to interrupt-and-replay; rename the constant.

**M2 — Task 09 invents idle-based runtime eviction.** `09-runtime_registry.md:14,16,26` (`RUNTIME_IDLE_EVICT_MS`) vs `03 §Runtime registry` ("Once quiescent, it is evicted **immediately**; only its worktree watcher receives a separate 30-minute idle stop") and §Decisions *Runtime eviction*. Resident-but-idle runtimes change observable `update_device_status` and queue behaviour. Replace with immediate quiescent eviction plus the residency predicate (lifecycle, queue, approval, interrupted-result, sandbox-subscription state), and give the 30-minute timer to the worktree watcher.

**M3 — Task 10 drops the `Command` turn state.** `10-turn_state_and_leases.md:12,23` ("Idle, Active, Cancelling") vs `03 §Lifecycle owner` (`Idle`, `Command { lease }`, `Active { … }`, `Cancelling { … }`) and `canonical-types.schema.json $defs.ConversationRuntimeSnapshot.turn_state` (`["idle","command","active","cancelling"]`). Task 02 correctly defines four. Device/slash/mod commands would have no state to occupy, so the snapshot enum is unrepresentable. Restore `Command`.

**M4 — Task 11 rewrites queue semantics.** `11-input_queue.md:13-15,25-26` vs `01 §Resource bounds` and `03 §Input and queue flow`. Four distinct breaks: (a) soft limit "warns" instead of replacing the oldest coalescable item; (b) coalescing keyed on "same client" instead of coalescability, losing barrier pass-through; (c) `buffer_limit` / `stale_generation` drop reasons omitted — real baseline values at `types/protocol.ts:492`; (d) `dequeued`/`cancelled` wire dispositions and snapshot-on-every-mutation omitted. Rewrite from `03 §Input and queue flow` with `QUEUE_ITEMS_SOFT_MAX`/`_HARD_MAX`.

**M5 — Task 13's four data shapes all diverge from spec and schema.** `13-provider_port.md:13-15,26-29`. `ProviderEvent` omits `ReasoningDelta`, `RedactedReasoning`, `ProviderMetadata`, and splits tool-call events wrongly (spec: `ToolCallStart | ToolCallArgumentsDelta | ToolCallEnd`; task: `ToolCallDelta | ToolCallComplete`); `ProviderError` has 4 of the spec's 12 stable kinds (missing authentication, authorization, quota, context overflow, overloaded, unavailable, protocol, cancelled); `ProviderRequest` omits image parts, reasoning controls, tool choice, and cancellation/deadline; `ModelDescriptor` contradicts `canonical-types.schema.json $defs.ModelDescriptor` **and** duplicates an entity task 02 already owns. Reasoning-stream loss alone breaks `03 §Provider and tool loop` and `06 §Streaming invariants`.

**M6 — Task 14 replaces the spec's turn bounds and event names, and decides an open question.** `14-provider_tool_loop.md:15,18,29-30`. Invents `STEPS_MAX`/`TOKENS_PER_TURN_MAX`; omits `TURN_PROVIDER_RETRIES_MAX`, `TURN_EMPTY_RESPONSE_RETRIES_MAX`, `CONTEXT_OVERFLOW_COMPACTIONS_MAX`, `TURN_TOOL_CALLS_MAX`, `TURN_STEPS_MAX`, `QUEUE_PUMP_BATCH_MAX`; invents `step-started`/`step-completed` events not in the protocol; omits the deny-by-policy / needs-approval / external branches and the distinct typed stop reasons; and hard-codes parallel tool execution that `03 §Open questions` leaves undecided.

**M7 — Task 16 renames the cancel grace and drops two required behaviours.** `16-cancellation.md:14,26` vs `03 §Cancellation and terminal behavior` steps 2 and 5 and `TURN_CANCEL_GRACE_MS` (10,000, plus a 2,000 ms SIGTERM→SIGKILL grace for shell children). Missing: normalization of unfinished local tool calls to interrupted results, and the two-stage child-kill.

**M8 — Task 17 omits both compaction modes and all three triggers.** `17-compaction.md:12,25` vs `03 §Compaction and prompt refresh`. No `all` / `sliding_window`; only the threshold trigger (spec: manual request, pre-call pressure, provider-reported overflow); no mod lifecycle callbacks; no before/after token and message counts; invented thresholds displace `CONTEXT_OVERFLOW_COMPACTIONS_MAX`.

**M9 — Task 21 omits the toolset contract.** `21-tool_registry.md` vs `05 §Tool registry`. Missing the six toolset IDs (`default`, `codex`, `codex_snake`, `gemini`, `gemini_snake`, `none` — confirmed at `tools/toolset.ts:198,204`), `auto` resolution, per-toolset model-facing names, `Task`→`Agent` global exposure, the allowlist (incl. empty-allowlist-exposes-none), approval policy, parallel-safety classification, and secret-bearing field redaction policy.

**M10 — Task 22 omits the permission model.** `22-permissions_sandbox.md` vs `05 §Permissions and sandbox`. Missing the four modes (`standard`, `acceptEdits`, `unrestricted`, `strict`) and the `unrestricted` default; the three file-backed scopes plus legacy XDG path; session and mod layering; shell-analysis bypass rejection; macOS Seatbelt / Linux Bubblewrap / explicit-unsupported adapters; workspace-sandbox peer-hiding.

**M11 — Task 23 covers a fraction of the file/shell built-ins.** `23-file_shell_executors.md:12-16` vs `05 §Rust built-ins`. Missing edit, multi-edit, apply-patch, glob, image view, artifact read/write; missing PTY/session, background output, stdin, monitor, stop. Invented bounds displace `TOOL_INPUT_BYTES_MAX`, `TOOL_RESULT_BYTES_MAX`, `TOOL_RESULT_MODEL_CHARS_MAX` (with the 30,000/10,000 per-family clamps), `CHILD_PROCESS_OUTPUT_BYTES_MAX`, `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT`. Baseline anchors exist for all of them under `tools/impl/`.

**M12 — Task 24 fabricates a Letta-Cloud tool surface.** `24-memory_planning_executors.md:12-16,25-29`. `core_memory_append/replace/delete`, `archival_memory_insert/search`, `memory_recall`, `create_plan/get_plan/list_plans`, `get_definition/get_references` appear in neither `05 §Rust built-ins` nor baseline `tools/impl/`. The spec's actual surface is memory edit/patch with Git commit and path confinement (`tools/impl/memory.ts`, `memory-apply-patch.ts`), `update_plan`/`UpdatePlan`/`TodoWrite`/task lifecycle, `ReadLSP` diagnostics only (`tools/impl/read-lsp.ts`). Building this ships tools the model has never been trained to call and omits the ones it will.

**M13 — Task 26 inverts skill precedence.** `26-skills.md:12,24` ("agent, project, system") vs `05 §Skills` ("1. project (`.agents/skills`, legacy `.skills` fallback), 2. agent (`~/.letta/agents/<id>/memory/skills`, `$MEMORY_DIR/skills` fallback), 3. global (`~/.letta/skills`), 4. bundled"). Also omits the optional-frontmatter fallbacks (ID/name from path, description from first body paragraph or `No description available`) that the spec explicitly preserves. Project skills would be shadowed by agent skills — a silent behaviour change.

**M14 — Task 27 fabricates the hook event set.** `27-hooks.md:13,25` (`before_command`, `after_command`, `before_prompt`, `after_prompt`) vs `05 §Hooks and mods` (pre/post tool, tool failure, permission request, user prompt, notification, stop, subagent stop, pre-compact, session start/end) and the prompt-hook subset restriction (notification, pre-compact, session remain command-hook-only). Existing hooks would never fire.

**M15 — Task 31 replaces snapshot sync with event-id replay and drops the ordering contract.** `31-ws_routing_events.md:16,29` vs `02 §Event envelopes and ordering` and §Core lifecycle. The spec's `idempotency_key` is explicitly "not stable across replay" and state updates are "snapshots, not diffs". Also missing: the `runtime{agent_id, conversation_id, acting_user_id?}` envelope, per-connection `event_seq`, `emitted_at`, the six ordering invariants, and `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX`.

**M16 — Task 32 omits the entire OpenAI statefulness contract.** `32-openai_http_routes.md` vs `02 §HTTP API`. Missing `X-Letta-Chat-Key` and `X-OpenWebUI-Chat-Id` (both real: `websocket/app-server-openai-common.ts:331,344`), `Idempotency-Key`/`X-Idempotency-Key` caching with in-flight sharing and failed-outcome eviction (`:366`), the unsigned `resp_letta_` base64url cursor, `previous_response_id` hidden-fork with `501 unsupported_backend`, `model_not_found` under `invalid_request_error`, agent-name-as-model-ID resolution, and `OPENAI_CHAT_KEYS_MAX`/`CHAT_IDEMPOTENCY_OUTCOMES_MAX`. Invents header auth in place of the shared listener policy.

**M17 — Task 33 omits the Schedule contract and is not wired to the queue.** `33-scheduler.md` vs `01 §Schedule` and `canonical-types.schema.json $defs.Schedule` (25 required fields). Missing IANA timezone semantics, `jitter_offset_ms`, the `active/fired/missed/cancelled` lifecycle, fire/miss counters, `cancel_reason`, one-shot timestamps, and `runs/<schedule-id>.jsonl` with `SCHEDULE_RUN_LOG_KEEP_LINES`/`_BYTES_MAX` rotation. Its DoD requires enqueue through the input queue but the graph has no 11→33 edge.

**M18 — Task 35 omits the access-control model.** `35-channels_access_control.md` vs `07 §Access control` and §Channel command surface. Missing `dm_policy` (`pairing`/`allowlist`/`open`), `group_policy`, `admin_users`, `user_allowed_commands`, central sender gating before commands or routes, "pairing approval adds permission; it never removes a stricter global deny", the one-unexpired-code-per-sender rule, the 50-code cap with expired-first pruning, and the 12 shared operational commands. Renames `PAIRING_TTL_SECONDS`.

**M19 — Task 36 is unauthorized scope.** `36-config_telemetry_health.md:5,12-14`. Its target page does not exist (B1), and no canonical page defines a configuration format, env-var prefix, or telemetry schema. The task nonetheless specifies JSON/TOML loading, `LOTTA_` env override, and a config schema. Either author the canonical page first (spec-creator) or reduce the task to what `architecture-principles.md §Cross-cutting conventions`, `07 §Health and readiness`, and `07 §Observability and security` authorize.

**M20 — Ports are co-located with concrete adapters, inverting the dependency graph.** `13-provider_port.md:8` + `18/19` pointers put `ProviderPort` and six vendor adapters in `crates/lotta-providers`; `21-tool_registry.md:8` + `23/24` put `ToolPort` and all built-in executors in `crates/lotta-tools`; `14-provider_tool_loop.md` (in `lotta-runtime`) depends on both. Violates `architecture-principles.md §Dependency graph` ("`lotta-runtime` depends on domain and port traits, **never concrete adapters**"). Move the port traits into `lotta-domain` or a dedicated ports crate, or split `lotta-providers`/`lotta-tools` into `-port` and `-adapters` crates, and update the graph.

**M21 — Composition-root location contradicts itself.** `architecture-principles.md §Workspace layout` and `01-workspace_bootstrap_toolchain_ci.md:8,17` put `src/main.rs` at the workspace root ("composition root only"); `37-composition_root.md:8` points at `crates/lotta-app-server/src/main.rs`. Task 38's Dockerfile depends on which is true. Pick one and correct the other; if the crate-local binary wins, update `architecture-principles.md §Workspace layout` via a change spec.

**M22 — The decomposition is horizontal, contradicting the plan's own stated spine.** Nothing is exercisable end to end before task 31; nothing runs as a binary before 37; M1's demonstrable state is a green build. `spec-planner` §Core principle rule 1 and §What NOT to do both forbid this, and `plan.md` §Decisions claims the opposite ("vertical-slice spine"). Re-cut a thin `runtime_start` → `input` → `stream_delta` → `turn_finished` slice (minimal domain + minimal store + transport + fake provider) as an early milestone so every later task is reviewed through a live client.

**M23 — Every task exceeds the DoD sizing guideline.** 46/46 have 7–14 DoD items against `task-decomposition.md` §Sizing "more than ~6 → probably two packages". Split at minimum 12 (14 items, seven independent resolutions), 24 (five tool families), 23 (files + shell/process), 19 (four adapters + sidecar), 36 (three crates), 39/40/41/42/46 (13 each).

**M24 — Seven dependency edges required by task DoDs are missing.** Table in Checkpoint 6. Add 11→33, 20→45, 20/33/35→39, 28/29→40, 15/16/11→31, 20/36→41, 39–45→46, and re-verify the Mermaid graph.

**M25 — Compatibility fixtures arrive after the code they must constrain.** No persistence fixture extraction before tasks 05–08 (first at 39); no provider stream fixture extraction anywhere despite `06 §Conformance` requiring replay of captured sanitized baseline streams; OpenAI goldens at 44. Add fixture-extraction tasks immediately after 03 covering `fixtures/persistence/`, `fixtures/providers/`, and `fixtures/reference-traces/` from `architecture-principles.md §Workspace layout`, and make 05–08 and 18–20 depend on them.

**M26 — Blocking spec open questions are silently decided.** `03` *Parallel tools* (decided by 14/23), `02` *Origin policy* and *TLS termination* (block 30/41), `04`/`06` *Live credential encryption* (decided by 45), `03` *Approval compatibility* (decided by 15), `architecture-principles` *Schema generation* (blocks 03/39). None appears in `plan.md` §Open questions. Surface each with the task it blocks, per `spec-planner` Phase 1 step 4.

**M27 — `CONNECTIONS_MAX` is overloaded across two meanings.** `20-provider_connections.md:18,31` reuses the WebSocket connection cap (`01 §Resource bounds`: 1,024) as the provider-connection cap; the spec names `PROVIDERS_MAX` = 128 (`06 §Limits`). A single constant with two meanings will be resolved to whichever crate is imported first — the exact case `Checks:` exists to catch, and no certificate has one.

**M28 — Twelve WebSocket command groups have no task.** Teleport, Terminal, Files, Memory, Models/providers, Schedules, Skills, Agent management, Conversation management, Settings, Device commands, Introspection (`app_server_info`) — see Checkpoint 3. `00 §Compatibility definition` requires the existing `app-server-client` to complete "the same command fixtures". Add tasks per group with `02 §WebSocket command groups` as the anchor; the terminal group in particular carries specified behaviour (two-second Strict-Mode reuse window, no-op input/resize for absent sessions, connection-cleanup kill) that will not be inferred.

**M29 — Constant naming diverges from both the spec and the guidelines across ~30 tasks.** Table in Checkpoint 7. Beyond parity, `SHELL_OUTPUT_MAX_BYTES`, `HTTP_REQUEST_BODY_MAX_BYTES`, `SUBAGENT_STATE_SNAPSHOT_MAX_BYTES`, `SANDBOX_ROOT_MAX_DEPTH`, `SKILL_FILE_MAX_BYTES`, `BACKUP_MAX_BYTES`, `SUPERVISOR_MAX_RESTARTS`, `CONFIG_FILE_MAX_BYTES` violate `development-guidelines.md` §Naming ("Units come last: `latency_ms_max`, not `max_latency_ms`"), which every task's baseline DoD item incorporates.

**M30 — Task 01's DoD requires an artifact the plan records as blocked.** `01-…:30` requires CI workflow files; `plan.md:252` lists *"CI provider … (Blocks task 01 CI configuration.)"* as unresolved. The first task in the plan cannot be discharged until an open question is answered. Resolve the question or move CI to a follow-on task.

### MINORS

**m1** — Zero-byte `plan-review-claude.md.tmp` in the plan folder root; `plan-template.md` defines `plan.md` + status subfolders only. Delete it.
**m2** — `Implements:` values are plain text, not relative links. `checklist.md` §Cross-links requires spec-page links resolving from the task subfolder (`../../../foo.md`). Converting them would have surfaced B1/B2 at authoring time.
**m3** — Task 39's three `§Compatibility` anchors are near-misses for `00 §Compatibility definition`; the 04 and 07 pages have no such heading. Repoint at `04 §Migration` / `§State outside the backend root` and `07 §Compatibility with letta-app-server-deployment`.
**m4** — Edge 18→32 is unjustified: OpenAI HTTP routes serve agents, not the OpenAI provider adapter. Drop it or state the rationale.
**m5** — Task 33 narrows to "standard 5-field cron"; `01 §Schedule` specifies a cron expression plus IANA timezone, and the baseline also parses intervals (`cron/parse-interval.ts`).
**m6** — M6's review gate "release checklist signed off" is not mechanically checkable; name the suites that must be green.
**m7** — Evidence specificity varies within certificates 01–08 ("run the transcript write tests" at `06:28` vs the named filter at `06:22`). Normalize to named test filters.
**m8** — Task 46 cites three whole pages with no section anchors, contributing nothing to coverage. Name the sections it documents.
**m9** — `plan.md` row 34 Produces "channel management protocol (20 commands)" while no task file lists the real 20; the plan's summary is more accurate than its task.
**m10** — Task 04 requires fakes that "satisfy the same contract tests as real adapters will", but no task creates the shared port-contract suite that `architecture-principles.md §Testing architecture` defines as its own tier. Add it, or make 04 own it explicitly.

---

## Questions (not findings)

1. Was `.specs/03-runtime.md` an earlier filename for `03-runtime-and-turns.md`, or were tasks 09–17 authored from an outline that was never reconciled with the canonical page? The answer determines whether B1 is a rename sweep or a re-authoring.
2. Is `06-configuration-and-telemetry.md` a planned canonical page? If so, task 36 should block on it; if not, task 36's config scope needs an authorizing decision.
3. `plan.md` §Open questions asks which crates should begin as modules, yet task 01's DoD hard-codes all 13 crates. Should task 01 create fewer crates and defer splits, or should the open question be closed first?
4. `01 §ID scheme` reserves `ui-msg-` for transcript/provider-facing messages and `letta-msg-` for API projections. No task names the projection mapping (`01 §Required query patterns`, "Resolve every projection key to the same source local message"). Intentional deferral or gap?
5. Tasks 19, 28, and 29 each define an independent "sidecar protocol". Should there be one shared bounded-sidecar-framing task (frame length, protocol version, owner identity, capability, timeout — `development-guidelines.md` §Where to validate) that all three depend on?
6. `04 §Backup and restore` says "operator-supplied key" while task 45 specifies a passphrase and `BACKUP_ENCRYPTION_ALGORITHM`. Is the mechanism decided, or does it remain the open question the spec records?

---

## What is correct, stated precisely

So the remediation is scoped rather than a rewrite: the graph mechanics, kanban layout, certificate structure, voice, and the `Already built` baseline are sound and verified.

- **Baseline pin is exact:** `letta-code@300f923f`, version `0.30.20`, deployment file `0.30.20` — all three verified.
- **`Already built` is accurate:** the repo contains only `.specs/`, `README.md`, `LICENSE`, `NOTICE`.
- **Graph mechanics are provably clean:** 79 Mermaid edges ≡ 79 table edges, no cycles, all `Depends on` reference lower numbers, 46 tasks ↔ 46 rows ↔ 46 certificates with zero orphans, and all 46 task-file `Depends on` lines agree with the table.
- **Kanban layout is exactly per template:** `plan.md` at root, everything in `backlog/`, no `Status:` fields, no `certificates/` subfolder, correct header with `**Layout:** kanban`, `Source and definition-of-done baseline` section present, closing block with all three subheadings.
- **Certificate scaffolding is exactly per template:** 517 obligations one-to-one with DoD items in DoD order, `Reviewable:` last in all 46, every status `☐ unverified`, every verdict blank, both link directions resolving.
- **DoD baseline derivation is correct and specific:** `plan.md` L 13 draws it from `development-guidelines.md` §Definition of done and §Limits and bounds with the actual commands.
- **Voice is clean:** zero banned words, no emoji, no exclamation points, no dates or estimates in bodies.
- **Tasks 01–08 are close to publishable:** correct baseline anchors, mostly resolvable spec anchors, spec-faithful constants, and certificates with real evidence. Task 06 in particular reproduces the baseline-compatible loading tolerances and the orphan-repair path faithfully.

The failure is concentrated: **the plan degrades sharply at task 09** and never recovers, in both task authoring (anchors, constants, semantics) and certificate authoring (evidence). That boundary is the same in both artifact families, which points at a single generation pass that exhausted its grounding rather than at scattered errors.

---

VERDICT: REVISE
CONFIDENCE: high
BLOCKERS: 8
MAJORS: 30
MINORS: 10
SUMMARY: Graph mechanics, kanban layout, and certificate scaffolding are provably correct (79≡79 edges, no cycles, 517 one-to-one blank obligations), but 89 of 139 spec anchors and 21 of 47 baseline pointers do not resolve — including a `.specs/03-runtime.md` and an `agent-runner.ts` that do not exist — leaving 57 of 100 spec sections unmapped, and from task 09 onward the plan substitutes invented constants, tool names, hook events, auth mechanisms, a 21-command channel protocol against a baseline-verified 20, and jitter the spec explicitly forbids, while 79% of certificate obligations discharge to "see the DoD".
