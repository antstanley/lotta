# Task 25 — Baseline-compatible transcript loading and active-projection repair

**Plan:** [plan.md](../plan.md) · **Certificate:** [25-store_baseline_compatible_loading-certificate.md](25-store_baseline_compatible_loading-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Baseline-compatible loading](../../../04-persistence-and-memfs.md#baseline-compatible-loading)
**Depends on:** 11, 24
**Produces:** a loader that accepts everything the reference accepts and repairs the active projection without mutating the source transcript
**Pointers:** `crates/lotta-store/src/transcript/load.rs`, `transcript/projection.rs`, `transcript/repair.rs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/local-message.ts`, `../letta-code/src/backend/local/transcript-migration.ts`

## Steps

- [ ] Accept valid JSONL with a supported manifest version, format, and provider stack
- [ ] Accept message and compaction rows when a session header is absent, and accept duplicate entry IDs and unverified parent links exactly as the baseline does
- [ ] Accept compaction references without enforcing a retained-entry graph at load
- [ ] Skip non-message/compaction rows during active-message projection
- [ ] Remove orphan tool results from the active projection, repair `in_context_message_ids`, persist `conversation.json`, and leave the source transcript unchanged
- [ ] Map the two baseline errors to `transcript_migration_required` and `transcript_repair_required`, and upgrade a versioned legacy `pi-ai-message-jsonl` transcript on its next non-empty persistence

## Definition of done

- [ ] The loader accepts all four baseline tolerances: missing session header, duplicate entry IDs, unverified parent links, and compaction references without a retained-entry graph
- [ ] Orphan tool results are removed from the active projection, `in_context_message_ids` is repaired, `conversation.json` is persisted, and the source transcript is byte-unchanged
- [ ] An unversioned non-empty transcript maps to `transcript_migration_required` and a versioned transcript with legacy UI-message rows maps to `transcript_repair_required`
- [ ] A versioned legacy `pi-ai-message-jsonl` transcript is upgraded on its next non-empty persistence, and oversized tool results follow the baseline clipping path
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(transcript::load::) + test(transcript::repair::)'` and sees the four tolerances, orphan repair with an unchanged source, both error mappings, and the legacy upgrade pass
