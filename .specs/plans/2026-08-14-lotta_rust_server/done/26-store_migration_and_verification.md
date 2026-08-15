# Task 26 — Transcript migration and strict verification

**Plan:** [plan.md](../plan.md) · **Certificate:** [26-store_migration_and_verification-certificate.md](26-store_migration_and_verification-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Migration](../../../04-persistence-and-memfs.md#migration) · [04-persistence-and-memfs.md §Strict verification](../../../04-persistence-and-memfs.md#strict-verification)
**Depends on:** 11, 25
**Produces:** an idempotent, backup-first migration command and a non-mutating `verify` command reporting the six diagnostic classes
**Pointers:** `crates/lotta-store/src/migration.rs`, `src/verify.rs`, `src/cli.rs`; reference: `../letta-code/src/backend/local/transcript-migration.ts`

## Steps

- [ ] Implement `lotta local-backend migrate-transcripts --storage-dir <path> [--dry-run]`
- [ ] Preserve the baseline conversion and `in_context_message_ids` remapping
- [ ] Write a timestamped source backup, make it durable, then replace atomically; leave the original active on failure
- [ ] Make a completed migration idempotent on re-run and bound scanning and output validation
- [ ] Implement `lotta local-backend verify` reporting duplicate entry IDs, invalid/backward parent links, absent/multiple headers, orphan tool results, invalid compaction references, and unsupported fields without mutating state
- [ ] Rotate Lotta backups at `MIGRATION_BACKUPS_PER_FILE_MAX`

## Definition of done

- [ ] Unversioned and versioned-legacy fixtures migrate to schema v2 with `in_context_message_ids` correctly remapped
- [ ] A timestamped backup is durable before replacement, a failed conversion leaves the original active, and re-running a completed migration is a no-op
- [ ] `--dry-run` produces no file change, and `MIGRATION_BACKUPS_PER_FILE_MAX` rotates the oldest completed Lotta backup
- [ ] `verify` reports all six diagnostic classes and mutates nothing, and does not tighten what the loader accepts
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `lotta local-backend migrate-transcripts --storage-dir <fixture-copy> --dry-run`, then without `--dry-run`, then `lotta local-backend verify`, and sees no change from the dry run, a backup-first conversion, and six diagnostic classes reported without mutation
