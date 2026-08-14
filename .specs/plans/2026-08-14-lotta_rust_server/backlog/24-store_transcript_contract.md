# Task 24 — Transcript manifest, session header, and entry append

**Plan:** [plan.md](../plan.md) · **Certificate:** [24-store_transcript_contract-certificate.md](24-store_transcript_contract-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Transcript contract](../../../04-persistence-and-memfs.md#transcript-contract) · [01-domain-model.md §Transcript entry](../../../01-domain-model.md#transcript-entry)
**Depends on:** 03, 11, 22
**Produces:** schema-v2 transcript JSONL with the exact manifest, one session header, and append-only message and compaction entries
**Pointers:** `crates/lotta-store/src/transcript/mod.rs`, `transcript/manifest.rs`, `transcript/append.rs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/local-message.ts`, `../letta-code/src/backend/local/compaction.ts`

## Steps

- [ ] Write `manifest.json` with `schema_version: 2`, `message_format: "pi-session-entry-jsonl"`, `provider_stack: "pi-ai"`, and `created_at`, plus optional `migrated_from`, `migrated_at`, `backup_path`
- [ ] Write one session header row `{"type":"session","version":3,"id":…,"timestamp":…,"cwd":…}` on a newly created transcript
- [ ] Append message entries with `id`, `parentId`, RFC3339 `timestamp`, and an embedded `LocalMessage` whose `timestamp` is numeric milliseconds
- [ ] Append compaction entries with `summary`, `firstKeptEntryId`, `tokensBefore`, `message`, and optional `details`, without rewriting or backing up the active transcript
- [ ] Restrict full rewrites to explicit migration, conversation fork, and initial/full persistence of an already loaded conversation
- [ ] Enforce `TRANSCRIPT_LINE_BYTES_MAX` and `TRANSCRIPT_BYTES_MAX` with below/at/above tests

## Definition of done

- [ ] `manifest.json` is written and read with the exact four required fields and the three optional migration fields
- [ ] A new transcript starts with exactly one session header at version 3, and subsequent rows are message or compaction entries only
- [ ] Message and compaction entries append one complete line each with correct parent linkage, and the embedded `LocalMessage` timestamp is numeric milliseconds while the entry timestamp is RFC3339
- [ ] Compaction appends without rewriting or backing up the active transcript, and full rewrites occur only for migration, fork, and initial/full persistence
- [ ] `TRANSCRIPT_LINE_BYTES_MAX` and `TRANSCRIPT_BYTES_MAX` are named constants rejecting append and read at the limit
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(transcript::)'` and sees manifest shape, single session header, append linkage, no-rewrite-on-compaction, and both byte bounds pass
