# Task 58 — Compaction modes, triggers, and prompt refresh

**Plan:** [plan.md](../plan.md) · **Certificate:** [58-compaction_and_prompt_refresh-certificate.md](58-compaction_and_prompt_refresh-certificate.md)

**Implements:** [03-runtime-and-turns.md §Compaction and prompt refresh](../../../03-runtime-and-turns.md#compaction-and-prompt-refresh) · [06-model-providers.md §Context and compaction](../../../06-model-providers.md#context-and-compaction)
**Depends on:** 24, 47, 55
**Produces:** both compaction modes with all three triggers, mod lifecycle callbacks, before/after counts, and revision-driven prompt refresh
**Pointers:** `crates/lotta-runtime/src/compaction/mod.rs`, `compaction/modes.rs`, `compaction/triggers.rs`; reference: `../letta-code/src/backend/local/compaction.ts`, `../letta-code/src/backend/dev/context-window-overflow.ts`, `../letta-code/src/websocket/listener/memfs-sync.ts`

## Steps

- [x] Implement `all` mode summarizing the eligible history into one summary message
- [x] Implement `sliding_window` mode summarizing an oldest prefix and retaining a configured recent percentage
- [x] Trigger on manual API request, pre-call context pressure, and provider-reported context overflow
- [x] Serialize compaction with the turn lease, emit mod lifecycle callbacks, store a transcript compaction entry, update in-context IDs, and recompile the prompt
- [x] Record before/after token and message counts
- [x] Detect MemFS change by committed revision and recompile before the next turn, honouring `CONTEXT_OVERFLOW_COMPACTIONS_MAX`

## Definition of done

- [x] Both compaction modes exist and produce their documented shape: `all` yields one summary message; `sliding_window` retains the configured recent percentage
- [x] All three triggers fire compaction: manual request, pre-call pressure, and provider-reported context overflow
- [x] Compaction is serialized with the turn lease, emits mod lifecycle callbacks, writes one transcript compaction entry, updates in-context IDs, and recompiles the prompt
- [x] Before/after token and message counts are recorded on the compaction entry
- [x] A committed MemFS revision change recompiles the prompt before the next turn, and an uncommitted change does not
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(compaction::)'` and sees both modes, all three triggers, the five lease-serialized effects, recorded counts, and revision-driven refresh pass
