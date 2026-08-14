# Lotta plan remediation ledger

**Date:** 2026-08-14 · **Reviewer:** clean Letta verification/remediation pass

This ledger reproduces every finding in `plan-review-claude.md` against the current canonical set, pinned `../letta-code` checkout (`300f923f16cc8eee50656d7da732902c1dea2b65`, version `0.30.20`), and current plan artifacts. `CONFIRMED` means the original defect was valid and required remediation. `MODIFIED` means the underlying concern was valid but the completed remedy differs from the review's suggested cut or numbering. `REJECTED` means the original claim did not hold. Finding identifiers retain their original task numbers; the final task numbers are shown where useful.

## Original blockers

| Finding | Classification | Current evidence and completed remediation |
|---|---|---|
| B1 — phantom canonical files and headings | CONFIRMED | All final `Implements` links target existing canonical pages and exact headings. The runtime work now points to `03-runtime-and-turns.md`; configuration was reduced to canonical CLI surface in Task 83 and telemetry/health moved to Task 84. Mechanical result: 0 broken spec targets/anchors. |
| B2 — broken anchors and missing forward coverage | CONFIRMED | All task anchors were rewritten as relative links. All 107 substantive in-scope headings are covered; only overview framing headings Problem, Goals, and Non-goals are intentionally excluded by `plan.md`'s coverage rule. |
| B3 — broken baseline pointers | CONFIRMED | Pointers now resolve to the pinned files, including listener runtime/turn files, `backend/dev/pi-*`, tool implementations, OpenAI App Server files, and `app-server-client.ts`. Mechanical result: 0 broken concrete baseline pointers. |
| B4 — invented auth and lost Origin rule | CONFIRMED | Task 14 now specifies `capability-token` and `signed-bearer-token`, exact flags, HS256 claims/skew, pre-listen non-loopback rejection, and authentication of every Origin-bearing client including loopback. Task 15 owns transport bounds/closure; Task 89 proves the security matrix. |
| B5 — fabricated channel protocol | CONFIRMED | Task 79 lists the exact 20 snake_case service commands and four push events. Task 78 owns `publish_runtime_tools`, `release_runtime_tools`, and `/channels`; Task 82 owns the 12 shared commands and four shared events. |
| B6 — provider retry jitter | CONFIRMED | Task 48 owns deterministic retry/fallback with retry-after, capped exponential transient/busy delay, linear empty-response delay, deadline, `PROVIDER_RETRIES_MAX = 3`, `PROVIDER_BACKOFF_MS_MAX = 60_000`, and explicitly no provider-turn jitter. Schedule jitter remains because it is a separate canonical schedule contract. |
| B7 — generic certificates and absent resolution checks | CONFIRMED | All 95 certificates were reauthored. Each obligation contains a concrete claim and evidence; 105 targeted `Checks:` clauses cover symbol/call resolution where applicable. Generic “see task/DoD” evidence is absent. Regression stanzas name downstream units for tasks extending prior work. |
| B8 — invalid topological order | CONFIRMED | The plan was renumbered before execution. All 273 dependencies point to lower task numbers; task/table dependencies equal the Mermaid edges; the graph is acyclic. Tools, skills, hooks, mods, model resolution, and permissions precede full turn setup. |

## Original majors

| Finding | Classification | Current evidence and completed remediation |
|---|---|---|
| M1 — approval timeout auto-denial | CONFIRMED | Task 56 interrupts at `APPROVAL_WAIT_MS_MAX`, persists/replays explicit expiry, and forbids implicit denial. |
| M2 — idle runtime eviction | CONFIRMED | Task 16 uses the five-part residency predicate and immediate quiescent eviction; only the watcher receives its canonical idle timer. |
| M3 — missing `Command` state | CONFIRMED | Tasks 04 and 17 use exactly Idle, Command, Active, Cancelling and test legal/illegal transitions. |
| M4 — incorrect queue semantics | CONFIRMED | Task 18 implements serialized admission, oldest-coalescable replacement, barriers, hard rejection, `buffer_limit`/`stale_generation`, dispositions, and snapshot-on-mutation. |
| M5 — divergent provider shapes | CONFIRMED | Task 07 defines the exact normalized events, 12 error kinds, request controls, cancellation/deadline, and canonical `ModelDescriptor` use; Task 47 owns handle/settings resolution. |
| M6 — invented turn bounds/events/parallelism | CONFIRMED | Tasks 19, 55, and 59 split the loop, branches/stop reasons, and exact bounds. Wire events remain canonical; tools default sequential pending certification. |
| M7 — cancellation omissions | CONFIRMED | Task 57 uses `TURN_CANCEL_GRACE_MS`, normalizes unfinished calls, and includes SIGTERM→SIGKILL child handling. |
| M8 — incomplete compaction | CONFIRMED | Task 58 covers `all` and `sliding_window`, manual/pre-call/provider-overflow triggers, callbacks, counts, transcript append, and prompt refresh. |
| M9 — incomplete toolset contract | CONFIRMED | Task 32 covers six IDs, `auto`, model-facing names, global Task→Agent exposure, allowlist including empty-none, approval, parallel safety, and redaction. |
| M10 — incomplete permissions/sandbox | MODIFIED | The scope was split: Task 34 owns modes, defaults, file-backed/session/mod layering and shell-analysis bypass rejection; Task 35 owns Seatbelt/Bubblewrap/explicit unsupported and workspace peer hiding. |
| M11 — partial file/shell tools | MODIFIED | Split into Task 37 file/image/artifact tools and Task 38 PTY/session/background/stdin/monitor/stop, with exact family clamps and process bounds. |
| M12 — invented built-ins | CONFIRMED | Tasks 39–40 use memory edit/patch + Git, worktree tools, `update_plan`/`UpdatePlan`, `TodoWrite`, task lifecycle, interaction tools, and `ReadLSP`; invented Cloud-memory/planning/LSP names were removed and negative tests forbid them. |
| M13 — reversed skill precedence | CONFIRMED | Task 36 implements project → agent → global → bundled, both legacy fallbacks, and optional-frontmatter fallback semantics. |
| M14 — fabricated hooks | CONFIRMED | Task 44 implements the canonical eleven events and exact seven-event prompt-hook subset. |
| M15 — event replay instead of snapshot sync | CONFIRMED | Task 73 replays authoritative snapshots, not event IDs/diffs; Task 20 owns runtime envelope, per-connection sequence, emitted time, idempotency key, and ordering invariants. |
| M16 — missing OpenAI statefulness | MODIFIED | Split among Tasks 74–77. Chat keys, both idempotency headers, in-flight sharing/failed eviction, unsigned `resp_letta_` cursor, hidden fork, 501 unsupported backend, error shape, model resolution, cursors, and limits are explicit. |
| M17 — incomplete Schedule and queue wiring | MODIFIED | Task 03 defines all 25 fields; Task 60 owns persistence, cron+interval parsing, timezone, canonical schedule jitter and run logs; Task 61 fires through Task 18's queue. |
| M18 — incomplete channel access control | CONFIRMED | Task 81 covers DM/group policy, admins/users/commands, sender gating first, monotonic pairing permissions, one live code/sender, expiry-first pruning, and exact bounds; Task 82 covers shared commands/events. |
| M19 — unauthorized config scope | CONFIRMED | Task 83 contains only canonical CLI and environment paths. File format and prefix remain a non-blocking open question; health/telemetry is Task 84. |
| M20 — inverted ports/adapters direction | MODIFIED | Port traits live in `lotta-runtime::ports`; adapter crates depend on those interfaces, while runtime core never depends on concrete adapters. This follows the canonical 13-crate graph without adding crates. |
| M21 — conflicting composition root | CONFIRMED | Tasks 01, 85, and 86 consistently use workspace-root `src/main.rs`; no crate-local binary is planned. |
| M22 — horizontal decomposition | CONFIRMED | Tasks 14–21 form the early fixture-constrained real-client → fake ports → turn vertical slice before real stores/providers/tools. |
| M23 — oversized tasks | MODIFIED | The plan grew from 46 to 95 packages. Most DoDs have six items; the few seven-item tasks retain one cohesive boundary where further splitting would separate one contract from its required review trace. |
| M24 — missing dependencies | CONFIRMED | Dependencies were rederived. Table, Mermaid, and task headers are equal; all data, contract, build, and review edges are explicit and lower-numbered. |
| M25 — late compatibility fixtures | CONFIRMED | Tasks 10–13 create protocol, persistence, provider-stream, and reliability/reference corpora before constrained implementation. |
| M26 — silently decided open questions | CONFIRMED | `plan.md` now records Origin/TLS, parallel tools, approval expiry, recovery depth, live credential encryption, schema source, config format, subagents, channel ownership, deployment repository, baseline window, and coverage policy as non-blocking questions with affected tasks. |
| M27 — overloaded `CONNECTIONS_MAX` | CONFIRMED | Provider capacity uses canonical `PROVIDERS_MAX`; WebSocket capacity remains the transport connection bound. Resolution checks guard similarly named symbols. |
| M28 — missing WebSocket command groups | CONFIRMED | Tasks 62–73 cover external tools, teleport, terminal, files, memory, models/providers, schedules, skills/settings, agent, conversation, device/introspection/outbound groups, and snapshot recovery. |
| M29 — noncanonical constant names | CONFIRMED | Tasks use canonical names and units-last naming. Certificates explicitly inspect newly introduced bounds; invented names were removed. |
| M30 — CI provider blocked Task 01 | CONFIRMED | GitHub Actions is a reality-backed decision, no longer an open question; Task 01's CI obligation is dischargeable. |

## Original minors

| Finding | Classification | Current evidence and completed remediation |
|---|---|---|
| m1 — temporary review artifact | CONFIRMED | No `.tmp` or superseded artifact remains. `plan-review-claude.md` is unchanged. |
| m2 — plain-text `Implements` targets | CONFIRMED | Every target is a relative Markdown link from the task folder. |
| m3 — near-miss compatibility anchors | CONFIRMED | Final tasks use exact `Compatibility definition`, `Migration`, `State outside the backend root`, and `Compatibility with letta-app-server-deployment` headings. |
| m4 — unjustified provider→OpenAI edge | CONFIRMED | OpenAI routes depend on model/runtime/store contracts, not a concrete OpenAI provider adapter. |
| m5 — schedule narrowed to five-field cron | CONFIRMED | Task 60 includes cron expressions, baseline interval forms, and IANA timezone/DST behavior. |
| m6 — non-executable milestone gate | CONFIRMED | Every milestone names exact commands and expected green outputs. |
| m7 — uneven certificate specificity | CONFIRMED | Every certificate now names prospective files/symbols, test filters or concrete traces, commands, and expected results. |
| m8 — whole-page task anchors | CONFIRMED | Final tasks name exact sections; no whole-page task link is used for coverage. |
| m9 — plan/task mismatch for 20 commands | CONFIRMED | Task 79 itself lists all exact 20 names and the four push events. |
| m10 — absent shared port-contract suite | CONFIRMED | Task 09 creates the generic suite and fakes; every concrete adapter is required to reuse it. |

## Classification counts

- **Original findings:** 48 total — **CONFIRMED 42**, **MODIFIED 6**, **REJECTED 0**.
- Blockers: 8 confirmed, 0 modified, 0 rejected.
- Majors: 25 confirmed, 5 modified, 0 rejected.
- Minors: 9 confirmed, 1 modified, 0 rejected.

## Additional findings from the clean pass

| ID | Finding | Remediation |
|---|---|---|
| A1 | All 95 certificate obligation titles had trailing periods absent from their paired DoD text, so exact one-to-one text equality failed despite equal counts/order. | Removed only the certificate-title terminal periods. Exact DoD↔obligation text/count/order now passes for all 95 pairs. |
| A2 | Nine certificates still used the blanket “no existing callers” regression stanza despite depending on and consuming earlier task units. | Replaced those stanzas with named downstream regression suites for entity/state/error/protocol/fixture/auth/sidecar/model-resolution integrations. The three genuinely independent greenfield certificates (Tasks 01, 02, 06) retain the statement. |
| A3 | `$MEMORY_DIR` was unset in this subagent shell, so the literal requested shared-skill path could not resolve. | Located and read the complete read-only shared skill source under `/Volumes/Delorean/code/skills/skills/` (spec-planner, done-certificates, spec-reviewer, spec-creator and all Markdown companions). This did not block remediation. |

## Structural edits and verification scope

- Final plan: 95 task files and 95 co-located certificates, all initially in `backlog/`.
- `plan.md` has one 95-row dependency table and a 273-edge Mermaid DAG; task headers, table, and graph agree exactly.
- `.specs/README.md` indexes the plan.
- No canonical spec was altered to make the plan pass.
- No commit, bookmark, or push was created.
