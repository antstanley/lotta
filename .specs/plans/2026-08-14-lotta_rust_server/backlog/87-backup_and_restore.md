# Task 87 — Consistent backup and validated restore

**Plan:** [plan.md](../plan.md) · **Certificate:** [87-backup_and_restore-certificate.md](87-backup_and_restore-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Backup and restore](../../../04-persistence-and-memfs.md#backup-and-restore)
**Depends on:** 22, 23, 24, 27, 29, 52
**Produces:** a quiesced backup of the backend root, side stores, and MemFS repositories, encrypted with an operator-supplied key and restored only after full validation
**Pointers:** `crates/lotta-store/src/backup/snapshot.rs`, `backup/encrypt.rs`, `backup/restore.rs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/paths.ts`

## Steps

- [ ] Pause new Lotta durable admissions, wait for active commits, then snapshot the local-backend root and the selected external state
- [ ] Bundle MemFS repositories so each agent's Git history is preserved
- [ ] Sample source metadata before and after copying and abort on an observed lock-ignorant write
- [ ] Encrypt backup archives containing `providers/auth.json` with an operator-supplied key, leaving the live file plaintext
- [ ] Restore by decrypting into a new directory, validating the complete snapshot, applying secure modes, and swapping roots only after validation
- [ ] Document that operators must quiesce TypeScript writers because metadata checks cannot close every race

## Definition of done

- [ ] Backup quiesces durable admissions and waits for active commits before snapshotting, and aborts on an observed external write
- [ ] The snapshot covers the backend root, every side store, and every MemFS repository with its Git history
- [ ] Archives containing `providers/auth.json` are encrypted with an operator-supplied key while the live file stays plaintext
- [ ] Restore validates the complete snapshot and applies secure modes before swapping roots, leaving the existing root untouched on failure
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer takes a backup of a populated root with an operator key file, deletes the root, restores from the archive, and sees agents, conversations, transcripts, side stores, and MemFS history return with `auth.json` at mode `0600`
