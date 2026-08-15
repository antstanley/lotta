# Done Certificate — Task 26: Transcript migration and strict verification

**Task:** [26-store_migration_and_verification.md](26-store_migration_and_verification.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 26. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 26) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** an idempotent, backup-first migration command and a non-mutating `verify` command reporting the six diagnostic classes.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let `verify`'s stricter checks become startup rejection: §Strict verification states they are diagnostics, not stricter rejection that would break import compatibility.

## Obligations

- **O1 — Unversioned and versioned-legacy fixtures migrate to schema v2 with `in_context_message_ids` correctly remapped**
  - *Claim:* Both migration fixtures convert and their remapped ID lists match the expected output recorded in the corpus.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(migration::converts)'` — expect two passing cases driven by `fixtures/persistence/`; compare the remapped lists against the corpus expectation files.
  - *Status:* ☑ SATISFIED — exact `migration::converts` selector ran twice with 2/2;
    the cases assert full pinned UI conversion, mixed PI/UI repair, exact IDs/order/context
    remapping, backups, `migrated_at`, and public Task 25 reload.

- **O2 — A timestamped backup is durable before replacement, a failed conversion leaves the original active, and re-running a completed migration is a no-op**
  - *Claim:* The backup exists and is fsynced before the rename; an injected conversion failure leaves the source unchanged; a second run writes nothing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(migration::durability)'` — expect `backup_durable_before_replace`, `failure_leaves_original`, and `rerun_is_noop` to pass; the last asserts zero writes on the second invocation.
  - *Checks:* Resolve the replacement call — confirm it is the Task 22 atomic writer, not a direct `fs::write`. A direct write would lose the conflict check §Migration inherits.
  - *Status:* ☑ SATISFIED — exact `migration::durability` selector ran twice with 3/3;
    observed evidence covers backup file/parent fsync before replacement, six actual
    pre/post-publication failure seams, ownership-safe rollback, and a zero-callback,
    inode-aware no-op rerun.

- **O3 — `--dry-run` produces no file change, and `MIGRATION_BACKUPS_PER_FILE_MAX` rotates the oldest completed Lotta backup**
  - *Claim:* A dry run leaves the tree byte-identical; a fourth backup evicts the first.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(migration::dry_run) + test(migration::backup_rotation)'` — expect both PASS; the dry-run case hashes the tree before and after.
  - *Status:* ☑ SATISFIED — exact combined selector passed 2/2; dry run preserves the full
    tree and a real fourth migration backup rotates 3→3 without following symlinks.

- **O4 — `verify` reports all six diagnostic classes and mutates nothing, and does not tighten what the loader accepts**
  - *Claim:* Each of the six classes is reported on a fixture that exhibits it, the tree is unchanged, and the same fixture still loads through Task 25.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(verify::)'` — expect six diagnostic cases plus `does_not_mutate` and `does_not_narrow_loading`, the last asserting the loader still accepts every fixture `verify` flags.
  - *Status:* ☑ SATISFIED — `verify::` passed 14/14, including all six classes, exact
    deterministic findings, non-mutation, bounds/confinement negatives, and public-loader
    compatibility/repair evidence for every flagged tolerated state.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — formatting, strict all-target/all-feature Clippy, strict Rustdoc,
    `cargo deny`, source audits, module/function/column limits, and the all-feature workspace
    suite passed; the final workspace count is 797/797.

- **O6 — Reviewable: a reviewer runs `lotta local-backend migrate-transcripts --storage-dir <fixture-copy> --dry-run`, then without `--dry-run`, then `lotta local-backend verify`, and sees no change from the dry run, a backup-first conversion, and six diagnostic classes reported without mutation**
  - *Claim:* The two CLI commands behave as specified against a real fixture copy.
  - *Evidence to collect:* Run the three commands against a copy of `fixtures/persistence/unversioned`; confirm the dry run leaves the directory hash unchanged, the real run creates a timestamped backup before `messages.jsonl` changes, and `verify` exits reporting diagnostics with the tree hash unchanged.
  - *Status:* ☑ SATISFIED — compiled-binary integration passed 3/3: dry-run snapshot unchanged,
    real backup exact and schema-v2 publication, rerun byte/inode/mtime no-op, strict verify
    exact safe output and unchanged tree, plus pre-bind CLI negatives/server compatibility.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/transcript/load.rs` (Task 25) must still accept every fixture `verify` flags; confirm `transcript::load::tolerances` still passes : ☑ PRESERVED (`transcript::load::tolerances` 4/4; broad Task 25 transcript regressions 53/53)

## Residue

Bounded scanning, output validation, and atomic replacement are Lotta hardening; the pinned TypeScript migration writes converted files in place after copying the backup.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: DONE
CONFIDENCE: high
SUMMARY: O1–O6 are satisfied by exact selectors and real binary traces. The distinct final
review returned `VERDICT: CORRECT / DONE`; Task 25 loading/repair compatibility is preserved.
