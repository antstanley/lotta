# Task 18 — Input admission chain and the bounded conversation queue

**Plan:** [plan.md](../plan.md) · **Certificate:** [18-input_admission_and_queue-certificate.md](18-input_admission_and_queue-certificate.md)

**Implements:** [03-runtime-and-turns.md §Input and queue flow](../../../03-runtime-and-turns.md#input-and-queue-flow) · [01-domain-model.md §Input disposition](../../../01-domain-model.md#input-disposition) · [01-domain-model.md §Queue item](../../../01-domain-model.md#queue-item)
**Depends on:** 05, 17
**Produces:** a serialized admission chain and per-runtime FIFO queue with baseline coalescing at the soft tier, visible rejection at the hard tier, and a snapshot on every mutation
**Pointers:** `crates/lotta-runtime/src/admission.rs`, `src/queue.rs`, `src/queue_snapshot.rs`; reference: `../letta-code/src/websocket/listener/inbound-queue.ts`, `../letta-code/src/websocket/listener/queue.ts`, `../letta-code/src/websocket/listener/queue-update-transitions.test.ts`, `../letta-code/src/websocket/listener/queue-no-coalesce.test.ts`, `../letta-code/src/types/protocol.ts`

## Steps

- [x] Implement the serialized admission chain with the four outcomes of `03-runtime-and-turns.md` §Input and queue flow: duplicate replay, continuation/control on the active lease, idle start, occupied enqueue
- [x] At `QUEUE_ITEMS_SOFT_MAX`, admit a coalescable item by replacing the oldest coalescable item; let barrier items pass the soft level
- [x] At `QUEUE_ITEMS_HARD_MAX`, reject every admission and record the `buffer_limit` drop reason
- [x] Emit wire dispositions `dequeued` and `cancelled` for remove/cancel operations, and retain `buffer_limit`/`stale_generation` as internal reasons
- [x] Emit an authoritative queue snapshot on every mutation and route task notifications, cron prompts, approval results, overlay actions, and mod continuations through the same queue
- [x] Pump only from an `Idle` snapshot, in batches bounded by `QUEUE_PUMP_BATCH_MAX`

## Definition of done

- [x] The soft tier replaces the oldest coalescable item rather than warning, and barrier items pass the soft level unreplaced
- [x] The hard tier rejects every admission with the `buffer_limit` reason, and internal drops record `stale_generation` where applicable
- [x] Wire removal dispositions are exactly `dequeued` and `cancelled`, and a queue snapshot is emitted on every mutation
- [x] A duplicate `client_message_id` returns the prior disposition without executing twice, and every out-of-band source enters through the queue
- [x] The pump consumes only an `Idle` snapshot and yields after `QUEUE_PUMP_BATCH_MAX` items
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(queue::) + test(admission::)'` and sees soft-tier replacement, barrier pass-through, hard-tier `buffer_limit`, wire dispositions, duplicate replay, and bounded pumping pass
