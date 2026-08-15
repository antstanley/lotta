# Task 20 verification certificate — CLEAN final review

**Task:** [20-ws_runtime_commands_and_event_envelopes.md](20-ws_runtime_commands_and_event_envelopes.md)  
**Reviewed workspace:** `/Volumes/Delorean/code/five-letters/lotta-workspaces/task-20`  
**Review method:** independent source/diff review plus fresh execution; implementer summaries were not trusted. Only this certificate was modified.

## Definition

DONE(Task 20) requires O1–O6 all SATISFIED and regressions PRESERVED.

## O1 — five Runtime commands and pre-service validation: SATISFIED

Source review of `ws/command.rs`, `ws/router.rs`, `ws/service.rs`, and live-listener failure framing confirms:

- exactly `runtime_start`, `input`, `sync`, `abort_message`, and `change_device_state` decode to typed commands;
- `runtime_start` keeps both agent choices and both conversation choices optional, rejects each conflicting pair, validates bounded structural extensions, and performs those checks before `RuntimeCommandService::runtime_start`;
- malformed and conflicting starts therefore make no service call/write;
- `abort_message.run_id` is optional and JSON null decodes as absent;
- `change_device_state` routes its typed `payload`;
- input without `request_id` emits no acknowledgement; rejected input omits `disposition`;
- typed failure responses are targeted to the origin and unstamped.

Execution, twice: `test(ws::runtime_group)` = **11/11** each run. Named cases include both conflicts, malformed structures/no service call, optional/null abort run ID, payload routing, no-request no-ack, physical input ordering, and a blocking proof that the router mutex is obtainable while `runtime_start` service work is awaited.

## O2 — envelopes and transactional sequencing: SATISFIED

`ws/event.rs`, `ws/envelope.rs`, and `ws/connection.rs` define exactly seven stamped event variants: `control_request`, `update_device_status`, `update_loop_status`, `update_queue`, `stream_delta`, `turn_finished`, and `update_subagent_state`, with event-specific fields serialized alongside `runtime`, `event_seq`, `emitted_at`, and `idempotency_key`. Runtime/input/sync/abort and management responses are separate unstamped types.

Each subscriber is stamped independently from its `RuntimeConnection.event_seq`; target collection is sorted by stable ordinal. Keys are `<type>:<seq>:<non-nil UUID-v4>`. Overflow is checked before clock/ID generation. Deliveries and sequence increments are committed only after every target stamp succeeds, so second-generator failure leaves all target counters unchanged.

Execution, twice: `test(ws::envelope)` = **9/9** each run, including seven exact event shapes, three unstamped response classes, independent sequences/replay keys, non-nil v4 key form, overflow transactionality, and second-generator failure transactionality.

## O3 — service/lock and true two-phase ordering: SATISFIED

All async service calls occur before the short synchronous router lock. `handle_text` dispatches the phase-one `RouteOutput` first; only after successful physical queue dispatch does it invoke `continue_input`, whose `RouterEventSink` stamps and dispatches continuation events. Outbound peer writes use bounded `tokio::mpsc`; router and outbound mutexes are not held across awaits.

All six `ws::ordering_invariants` tests build server frames from the actual router/service/sink/delivery paths and each calls Task 13 `assert_invariant`. The stale case uses Task 17 `LeaseGuard` over a real N→N+1 replacement and asserts an empty sink recording.

Execution, twice: `test(ws::ordering_invariants)` = **6/6** each run: increasing per-connection sequence, input acknowledgement before caused events, tool start/end order, exactly-once terminal after final delta, stale-lease suppression, and ascending fanout ordinals.

## O4 — subscriptions, connection domain, and live fanout: SATISFIED

The implementation uses domain `CONNECTIONS_MAX` and exact domain `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX = 256`. Duplicate subscriptions return before capacity/allocation. A distinct 257th request is rejected before allocation and mutation. HashMap iteration cannot affect delivery order because targets are explicitly sorted by connection ordinal.

The listener has an actual bounded-mpsc writer per live peer, origin-targeted responses, cloned senders outside the outbound mutex, and visible `Unavailable` overflow. Listener tests preserve Ping/Pong, binary close 1003, oversized close 1009, exact-cap protocol-error/socket reuse, and real two-peer broadcast fanout.

Execution, twice: `test(ws::subscriptions)` = **2/2** each run. `frame_cap_plus_one_closes_with_1009` passed **10/10** repeated runs. Both app-server runs included the live two-peer fanout and listener behavior tests.

## O5 — repository gates and source hardening: SATISFIED

Fresh results:

| Gate | Result |
|---|---:|
| `cargo nextest run -p lotta-app-server` run 1 | 175/175 |
| same, run 2 | 175/175 |
| `cargo nextest run -p lotta-protocol` | 15/15 |
| `cargo nextest run --workspace --all-features` run 1 | 640/640 |
| same, run 2 | 640/640 |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo doc --workspace --all-features --no-deps` | pass |
| `cargo deny check` | advisories/bans/licenses/sources all pass |
| `cargo tree -d` | inspected; only expected transitive duplicate shown |

Source review found no eager 262 KiB reserve. Route/service batches are bounded, large allocations use fallible reservation, production task sources contain no panic/unwrap/expect/unsafe/allow escape, and no mutex guard spans an await. Existing source-hard-limit and structural production scans passed in both 640-test workspace runs.

UUID generation uses `uuid` backed by reachable `getrandom 0.3.4`; Cargo reported no `getrandom 0.4` package, and the lock/tree review found no newly reachable 0.4 duplicate. The attempted version-qualified probe `getrandom@0.3.3` correctly failed because the resolved version is 0.3.4; this is not a product failure.

## O6 — reviewability: SATISFIED

Exact broad selector run twice: `test(ws::)` = **28/28** each run, comprising runtime group 11, envelope 9, ordering invariants 6, and subscriptions 2. Counts are nonzero and exact.

## Regression table

| Surface | Evidence | Status |
|---|---|---|
| Task 14 listener/auth | app-server 175/175 twice; origin-bearing loopback and listener transport/websocket tests pass | PRESERVED |
| Task 19 turn loop | both workspace runs 640/640 include turn tests, including bounded 257th-step behavior | PRESERVED |
| Task 18 queue wire transitions | both workspace runs include runtime queue/wire transition coverage | PRESERVED |
| Task 16 scanner/runtime scoping | both workspace runs include structural source scanner and runtime ownership/scoping coverage | PRESERVED |
| Task 13 comparator | every one of six Task 20 invariant tests invokes `assert_invariant`; testkit comparator suite passes in both workspace runs | PRESERVED |
| Frame-cap stability | 10/10 focused repetitions plus both app-server and workspace runs | PRESERVED |

A separately attempted ad-hoc selector `test(turn::)` selected zero tests because nextest matching is fully-qualified; the required turn coverage is nevertheless nonzero in both full 640-test workspace executions. No LEAK or SLOW annotations appeared in any run.

## Concerns and residue

- No load-bearing defect found.
- Other command groups and full sync replay semantics remain assigned to later tasks, as the canonical task states.
- Bounded-mpsc overflow is intentionally surfaced as `Unavailable`; Task 20 does not require a retry/backpressure policy beyond visible bounded failure.

## Conclusion

- O1: SATISFIED
- O2: SATISFIED
- O3: SATISFIED
- O4: SATISFIED
- O5: SATISFIED
- O6: SATISFIED
- Regression check: PRESERVED

**Correctness verdict: CORRECT**  
**Completion verdict: DONE**  
**Confidence: high**
