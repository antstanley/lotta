# Done Certificate — Task 87: Consistent backup and validated restore

**Task:** [87-backup_and_restore.md](87-backup_and_restore.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 87. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 87) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a quiesced backup of the backend root, side stores, and MemFS repositories, encrypted with an operator-supplied key and restored only after full validation.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not encrypt the live `auth.json`: `04-persistence-and-memfs.md` §Local-backend directory layout states the TypeScript runtime cannot read a `credentials.enc` replacement.

## Obligations

- **O1 — Backup quiesces durable admissions and waits for active commits before snapshotting, and aborts on an observed external write**
  - *Claim:* No durable write is admitted during the snapshot, and a lock-ignorant write during copy aborts the backup.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(backup::consistency)'` — expect `admissions_paused_during_snapshot` and `aborts_on_observed_external_write`, the second mutating a source file mid-copy through a test seam.
  - *Status:* ☐ unverified

- **O2 — The snapshot covers the backend root, every side store, and every MemFS repository with its Git history**
  - *Claim:* A restored backup contains all three classes and each agent's memory repository has its commit history.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(backup::coverage)'` — expect a case per class plus `memfs_history_preserved`, the last asserting the restored repository's commit count.
  - *Status:* ☐ unverified

- **O3 — Archives containing `providers/auth.json` are encrypted with an operator-supplied key while the live file stays plaintext**
  - *Claim:* The archive is unreadable without the operator key and the live `auth.json` remains baseline-compatible plaintext at mode `0600`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(backup::encryption)'` — expect `archive_requires_operator_key`, `wrong_key_fails`, and `live_auth_json_unchanged`.
  - *Checks:* Resolve the key source — confirm it is an operator-supplied key read from a file reference (Task 83), not a passphrase derived in-process. `04-persistence-and-memfs.md` §Backup and restore says operator-supplied key, and §Assumptions records live credential encryption as still open.
  - *Status:* ☐ unverified

- **O4 — Restore validates the complete snapshot and applies secure modes before swapping roots, leaving the existing root untouched on failure**
  - *Claim:* A corrupt archive fails validation and the live root is unchanged; a valid archive swaps only after validation with `0700`/`0600` applied.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(backup::restore)'` — expect `corrupt_archive_leaves_root_untouched` (asserting an unchanged root hash), `validates_before_swap`, and `applies_secure_modes`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer takes a backup of a populated root with an operator key file, deletes the root, restores from the archive, and sees agents, conversations, transcripts, side stores, and MemFS history return with `auth.json` at mode `0600`**
  - *Claim:* The backup and restore round trip is observable end to end.
  - *Evidence to collect:* Run the backup and restore commands against a fixture-populated root; confirm the restored tree hashes match the original for the backend root and side stores, `git -C memfs/<agent>/memory log` shows the original commits, and `stat` reports `0600` on `providers/auth.json`.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) writes the restored files; confirm `atomic::modes` still passes after restore : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-memfs/src/repo.rs` (Task 29) supplies the bundled repositories; confirm `ops::` still passes on a restored tree : ☐ (PRESERVED / REGRESSION)

## Residue

Operators must quiesce TypeScript writers; §Backup and restore states metadata checks cannot close every race.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
