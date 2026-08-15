# Task 22 — Store paths, atomic replacement, and conflict detection

**Plan:** [plan.md](../plan.md) · **Certificate:** [22-store_paths_and_atomic_writes-certificate.md](22-store_paths_and_atomic_writes-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Local-backend directory layout](../../../04-persistence-and-memfs.md#local-backend-directory-layout) · [04-persistence-and-memfs.md §Durability and bounds](../../../04-persistence-and-memfs.md#durability-and-bounds) · [04-persistence-and-memfs.md §Responsibilities](../../../04-persistence-and-memfs.md#responsibilities)
**Depends on:** 06, 09
**Produces:** base64url path encoding for both baseline key forms plus atomic temp-flush-rename writes that return `storage_conflict` on an observed external change
**Pointers:** `crates/lotta-store/src/paths.rs`, `src/atomic.rs`, `src/lock.rs`, `src/bounds.rs`; reference: `../letta-code/src/backend/local/paths.ts`, `../letta-code/src/backend/local/local-store.ts`

## Steps

- [x] Implement the `${LETTA_LOCAL_BACKEND_DIR:-~/.letta/lc-local-backend}` root resolution and the `agents/`, `conversations/`, `memfs/`, `providers/`, `indexes/` layout
- [x] Encode both conversation directory key forms exactly: `base64url("default:" + agent-id)` and `base64url("conversation:" + conversation-id)`
- [x] Implement the five-step atomic write of `04-persistence-and-memfs.md` §Agent and conversation records: temp file, flush contents, atomic rename, flush parent directory, compare source mtime/revision immediately before replacement
- [x] Return `storage_conflict` when a lock-ignorant process changed the source between comparison and rename
- [x] Implement the Lotta advisory lock with `LOTTA_STORAGE_LOCK_WAIT_MS = 0`, documented as serializing Lotta writers only
- [x] Create the `providers/` directory mode `0700` and force `auth.json` mode `0600`; enforce `ATOMIC_WRITE_RETRIES_MAX`

## Definition of done

- [x] Both baseline conversation key forms encode and decode exactly, and the Task 11 corpus round-trips through the path encoder
- [x] Writes are atomic in all five steps and a concurrent external modification yields `storage_conflict` rather than a silent overwrite
- [x] Disk-full, permission, parse, checksum, conflict, and Lotta-lock failures are distinct typed errors, and error logs contain paths but never contents or credentials
- [x] `providers/` is created mode `0700`, `auth.json` is forced to `0600`, and `ATOMIC_WRITE_RETRIES_MAX` bounds retry
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(paths::) + test(atomic::) + test(errors::)'` and sees both key forms, five-step atomic writes, conflict detection, distinct error kinds, and secure modes pass
