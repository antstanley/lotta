# Task 18 Verification Certificate — Input Admission and Queue

**Task:** [18-input_admission_and_queue.md](18-input_admission_and_queue.md) · **Plan:** [plan.md](../plan.md)
**Validated:** 2026-08-15 — adversarial review loop 1
**Implementation:** `/Volumes/Delorean/code/five-letters/lotta-workspaces/task-18`

## Definition

DONE(Task 18) ≡ O1…O7 are SATISFIED and the regression check is PRESERVED.

## Review scope and method

Fresh full review of the task, this certificate, `01-domain-model.md`, `03-runtime-and-turns.md`, the pinned queue semantics named by the task, and the complete Jujutsu working-copy diff (21 files, +1632/−31). The implementation and VCS were not modified. Typed `QueueMutation` return values are accepted as the Task 18 core emission boundary because Task 20 owns wire framing. `PumpDirective::YieldBeforeNextBatch` is accepted as the Task 18 scheduling contract because the core has no scheduler yet, the result is `#[must_use]`, and the directive explicitly forbids the next batch before a yield.

## Obligations

### O1 — Soft tier: **SATISFIED**

- `queue.rs:76-91,167-184,260-268` checks hard capacity before soft handling, classifies exactly four coalescable kinds independent of client/source, removes the oldest retained coalescable item amid barriers, and appends the replacement at the tail. An all-barrier queue falls through and may reach 101.
- `VecDeque::remove` failures return `queue_storage_invariant`; there is no silent repair. Revision is prechecked, then exactly one `finish` commits one mutation/snapshot/revision. Duplicate item IDs below hard capacity are atomic.
- Exact nonzero selector: 5/5 PASS: `cargo nextest run -p lotta-runtime -E 'test(queue::soft_tier)'`.
- Coverage includes boundary 99→100, replacement length/order/reason/revision, barrier 101, kind-only classification, and all-barrier fallthrough.

### O2 — Hard tier and stale generation: **SATISFIED**

- `queue.rs:77-84` rejects at actual length 300 before duplicate-ID and soft-tier checks for every kind, with exact `buffer_limit`, unchanged retained contents, and one post-state snapshot.
- `queue.rs:126-140` removes the exact named stale item with exact `stale_generation`; missing IDs return `None` without mutation.
- Exact nonzero selector: 4/4 PASS: `cargo nextest run -p lotta-runtime -E 'test(queue::hard_tier)'`.

### O3 — Dispositions and authoritative snapshots: **SATISFIED**

- Domain JSON spellings are exactly `dequeued`, `cancelled`, `buffer_limit`, and `stale_generation`.
- Every enqueue, replacement, hard/stale rejection or removal, cancel, dequeue, and pump returns exactly one `QueueMutation` containing exactly one typed event and one full post-state `QueueSnapshot`; no queue-event accumulator exists.
- `QueueSnapshot` and `QueueMutation` constructors are crate-private. Snapshot vectors originate only from the private queue and remain bounded by the hard-cap invariant (≤300). Payloads are complete queue items, with bounded JSON content; no credentials or unrelated secret-bearing state is introduced.
- Revision exhaustion is checked before all queue mutations, including soft replacement. Duplicate item ID failure below hard capacity is atomic. Queue item IDs and timestamps enter through already-complete validated items, enabling injected generators/clocks.
- Exact nonzero selector: 5/5 PASS: `cargo nextest run -p lotta-runtime -E 'test(queue::wire_transitions)'`.

### O4 — Serialized admission and out-of-band routing: **SATISFIED**

- `ListenerRuntime::admit(&mut self, …)` resolves the exact current entry before effects, checks prior `client_message_id` first, performs one outcome, then records history only after success. Started, queued, and hard-rejected duplicate replays add no second queue mutation, revision, item, or history admission.
- Current continuation/control routes require the live lease and proceed directly; stale routes return a `stale_generation` rejection/snapshot without lifecycle mutation. Ordinary direct start is limited to `Message + Idle + empty`; idle with pending work enqueues FIFO.
- Task notification, cron prompt, approval result, overlay action, and mod continuation all enqueue even while Idle. Five named source tests plus FIFO coverage prove exact kind/source retention and unchanged Idle lifecycle.
- Queue and admission history are owned once per `RuntimeEntry`; the registry exposes immutable queue inspection and owned mutation methods, not `&mut ConversationQueue`. The standalone public queue type does not create a second per-entry owner.
- `AdmissionHistory::admit` assertions are internal bounded-container consistency assertions and are not realistically fallible through the private fields; recording after queue success is acceptable. Kind/source pairing is intentionally an inbound-adapter validation responsibility, consistent with `AdmissionRequest` being documented as fully validated.
- Exact selector: duplicate+sources 9/9 PASS; full admission flow is also included in O7.

### O5 — Bounded live-state pump: **SATISFIED**

- `ListenerRuntime::pump_queue` derives the live lifecycle projection from the exact current entry. Non-Idle returns `None` without mutation or repair.
- Pumping selects at most 64 contiguous coalescable items, or exactly one barrier, never crossing a barrier. Both 65 coalescables and 64+barrier return `YieldBeforeNextBatch` after the first 64. No internal loop can consume beyond one bounded batch.
- Public named `QUEUE_PUMP_BATCH_MAX = 64` is in `bounds.rs`; it is correctly excluded from the exact nine resource rows because it bounds a runtime operation rather than resident resources.
- Exact nonzero selector: 5/5 PASS: `cargo nextest run -p lotta-runtime -E 'test(queue::pump)'`.

### O6 — Repository definition of done: **SATISFIED**

- PASS: `cargo fmt --all --check`.
- PASS: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- PASS: `cargo nextest run --workspace --all-features` — 585/585.
- PASS: `cargo deny check` — advisories, bans, licenses, and sources all OK.
- No new dependency appears in the complete diff. Task 18 production files remain below 1000 lines, use ≤100 columns, and the reviewed Task 18 production modules contain no panic/unwrap/expect/unsafe/suppression/await/spawn. The workspace retains one runtime `RuntimeError` enum.
- Structural source gates pass. The robust recursive Syn/WalkDir architecture scanner is restored: all production files are scanned, cfg-test code and test files are excluded, nested/static/thread-local cases are visited, helper/appended spawn mutations are rejected, and exactly two audited production spawn functions remain. Queue ownership checks are loaded separately from `registry/tests/queue_ownership.rs`; there is no scanner weakening.

### O7 — Reviewable aggregate: **SATISFIED**

- Exact nonzero selector: `cargo nextest run -p lotta-runtime -E 'test(queue::) + test(admission::)'` — 34/34 PASS, run twice.
- The inventory has no `tests::` indirection and covers soft replacement/barrier pass-through, hard rejection and exact reasons, wire transitions and snapshots, all four admission outcomes, duplicate replay, all five out-of-band sources, and bounded live-state pumping.

## Regression check: **PRESERVED**

- `lotta-runtime`: 142/142 PASS, run twice.
- Task 16 registry aggregate: 28/28 PASS = keying 7, eviction 8, ambient 7, architecture 2, queue ownership 4; watcher 6/6 also PASS separately in the runtime suite. The expected aggregate is therefore 34 when watcher tests are included.
- Task 17 lifecycle: 13/13 PASS.
- `lotta-domain`: 61/61 PASS.
- `lotta-app-server`: 145/145 PASS.
- Workspace gate: 585/585 PASS.
- Residency remains exactly live lifecycle OR live queue length OR the three auxiliary terms (approval, interrupted result, sandbox subscription); callers no longer publish contradictory queue residency.

## Residue

Task 20 remains responsible for framing returned queue mutations/snapshots onto the wire and for implementing the scheduler that obeys `YieldBeforeNextBatch`. No Task 18 blocker remains.

## Conclusion

**VERDICT: CORRECT**
**CONFIDENCE: high**
**STATUS: DONE**

All O1…O7 are SATISFIED and regression is PRESERVED.
