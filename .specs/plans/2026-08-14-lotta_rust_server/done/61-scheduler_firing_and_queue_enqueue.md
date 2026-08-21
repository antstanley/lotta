# Task 61 — Scheduler firing, missed handling, and queue enqueue

**Plan:** [plan.md](../plan.md) · **Certificate:** [61-scheduler_firing_and_queue_enqueue-certificate.md](61-scheduler_firing_and_queue_enqueue-certificate.md)

**Implements:** [01-domain-model.md §Schedule](../../../01-domain-model.md#schedule) · [03-runtime-and-turns.md §Input and queue flow](../../../03-runtime-and-turns.md#input-and-queue-flow)
**Depends on:** 18, 60
**Produces:** a lease-aware scheduler that enqueues `cron_prompt` through the conversation queue and never bypasses it
**Pointers:** `crates/lotta-runtime/src/schedule/scheduler.rs`, `schedule/fire.rs`; reference: `../letta-code/src/cron/scheduler.ts`, `../letta-code/src/cron/prompt.ts`, `../letta-code/src/cron/scheduled-task-prompt.ts`

## Steps

- [x] Evaluate due schedules on a bounded tick, applying `jitter_offset_ms` to the scheduled instant
- [x] Enqueue a `cron_prompt` through the Task 18 admission chain, never mutating a turn directly
- [x] Mark a schedule `missed` when its window passed without firing, incrementing the miss counter
- [x] Cancel pending jitter-delayed timers on stop or lease loss
- [x] Record each fire, miss, and failure into the Task 60 run log
- [x] Retire a one-shot schedule after firing and keep a recurring one active

## Definition of done

- [x] A due schedule enqueues a `cron_prompt` through the conversation queue rather than starting a turn directly
- [x] A schedule whose window passed without firing is marked `missed` with its miss counter incremented
- [x] Pending jitter-delayed timers are cancelled on stop or lease loss, leaving no late fire
- [x] A one-shot schedule retires after firing while a recurring schedule stays active, and each fire, miss, and failure is written to the run log
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(scheduler::)'` and sees queue-routed firing, missed marking, timer cancellation on stop and lease loss, and one-shot retirement pass
