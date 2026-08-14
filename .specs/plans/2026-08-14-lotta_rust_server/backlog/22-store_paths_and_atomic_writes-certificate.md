# Done Certificate — Task 22: Store paths, atomic replacement, and conflict detection

**Task:** [22-store_paths_and_atomic_writes.md](22-store_paths_and_atomic_writes.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 22. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 22) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** base64url path encoding for both baseline key forms plus atomic temp-flush-rename writes that return `storage_conflict` on an observed external change.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not present the Lotta advisory lock as protection from a concurrently running TypeScript process: `04-persistence-and-memfs.md` §Agent and conversation records states that process does not participate.

## Obligations

- **O1 — Both baseline conversation key forms encode and decode exactly, and the Task 11 corpus round-trips through the path encoder**
  - *Claim:* The default key includes the agent (`default:<agent-id>`) and the named key includes only the conversation ID, matching the fixture directory names byte for byte.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(paths::key_forms)'` — expect PASS, driven by `fixtures/persistence/` directory names rather than by hand-written strings.
  - *Checks:* Resolve the base64 alphabet used — confirm it is URL-safe base64url without padding differences from `../letta-code/src/backend/local/paths.ts`. `NAME SHADOWING` risk: a generic `encode` helper must not resolve to standard base64.
  - *Status:* ☐ unverified

- **O2 — Writes are atomic in all five steps and a concurrent external modification yields `storage_conflict` rather than a silent overwrite**
  - *Claim:* The write path performs temp-write, content flush, rename, parent flush, and a pre-replacement mtime/revision comparison; an out-of-band change produces `storage_conflict`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(atomic::)'` — expect `writes_via_temp_and_rename`, `flushes_parent_directory`, and `external_change_yields_storage_conflict` to pass. Read the conflict case and confirm it mutates the file between the comparison and the rename using a test seam.
  - *Status:* ☐ unverified

- **O3 — Disk-full, permission, parse, checksum, conflict, and Lotta-lock failures are distinct typed errors, and error logs contain paths but never contents or credentials**
  - *Claim:* Six distinct error variants exist and the log line for each contains the path and no file body.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(errors::distinct_kinds) + test(errors::logs_exclude_contents)'` — expect both PASS; the second captures tracing output for a failed write of a file containing a marker string and asserts the marker is absent.
  - *Status:* ☐ unverified

- **O4 — `providers/` is created mode `0700`, `auth.json` is forced to `0600`, and `ATOMIC_WRITE_RETRIES_MAX` bounds retry**
  - *Claim:* Directory and file modes match `04-persistence-and-memfs.md` §Local-backend directory layout, and a persistently failing write stops after three attempts.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(atomic::modes) + test(atomic::retry_bound)'` — expect both PASS; the mode test reads the on-disk permission bits, and the retry test asserts exactly `ATOMIC_WRITE_RETRIES_MAX` attempts.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(paths::) + test(atomic::) + test(errors::)'` and sees both key forms, five-step atomic writes, conflict detection, distinct error kinds, and secure modes pass**
  - *Claim:* The path, atomic, and error modules pass against the checked-in fixture corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and that the key-form case sourced its inputs from `fixtures/persistence/`.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-testkit/src/contract/` (Task 09) suites now run against this real adapter as well as the fake; confirm the store contract suite passes for both implementations : ☐ (PRESERVED / REGRESSION)

## Residue

Concurrent mixed-runtime writes to one root remain unsupported by design; §Agent and conversation records defines cross-runtime compatibility as sequential round trips or a quiesced handoff.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
