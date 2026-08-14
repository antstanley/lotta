# Done Certificate — Task 24: Transcript manifest, session header, and entry append

**Task:** [24-store_transcript_contract.md](24-store_transcript_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 24. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 24) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** schema-v2 transcript JSONL with the exact manifest, one session header, and append-only message and compaction entries.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not rewrite the active transcript on compaction: `04-persistence-and-memfs.md` §Transcript contract states compaction appends one entry and does not back up or rewrite.

## Obligations

- **O1 — `manifest.json` is written and read with the exact four required fields and the three optional migration fields**
  - *Claim:* A newly created transcript's manifest matches `$defs.TranscriptManifest` with `schema_version` 2, `pi-session-entry-jsonl`, and `pi-ai`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::manifest)'` — expect `writes_exact_shape` and `reads_corpus_manifests` to pass, the second driven by `fixtures/persistence/`.
  - *Status:* ☐ unverified

- **O2 — A new transcript starts with exactly one session header at version 3, and subsequent rows are message or compaction entries only**
  - *Claim:* Row 0 is the session header; no second header is ever appended.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::session_header)'` — expect `written_once_at_row_zero` and `never_appended_again` to pass.
  - *Status:* ☐ unverified

- **O3 — Message and compaction entries append one complete line each with correct parent linkage, and the embedded `LocalMessage` timestamp is numeric milliseconds while the entry timestamp is RFC3339**
  - *Claim:* Appending N entries yields N lines, each parseable independently, with the two timestamp encodings distinct.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::append)'` — expect `one_line_per_entry`, `parent_chain_links`, and `timestamp_encodings_differ` to pass.
  - *Checks:* Resolve the timestamp serializer used for the embedded message — confirm it is the numeric-millisecond encoder, not the RFC3339 entry encoder. `NAME SHADOWING` risk: both are `Timestamp` values from Task 02 but serialize differently.
  - *Status:* ☐ unverified

- **O4 — Compaction appends without rewriting or backing up the active transcript, and full rewrites occur only for migration, fork, and initial/full persistence**
  - *Claim:* A compaction leaves preceding lines byte-identical; a rewrite is reachable only from the three named paths.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::no_rewrite_on_compaction)'` — expect PASS with a byte comparison of the prefix. Grep the crate for the rewrite entry point and confirm exactly three callers.
  - *Status:* ☐ unverified

- **O5 — `TRANSCRIPT_LINE_BYTES_MAX` and `TRANSCRIPT_BYTES_MAX` are named constants rejecting append and read at the limit**
  - *Claim:* An 8 MiB + 1 line is rejected and a transcript at 16 GiB requires archive/export before append.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::bounds)'` — expect below/at/above cases for both bounds.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(transcript::)'` and sees manifest shape, single session header, append linkage, no-rewrite-on-compaction, and both byte bounds pass**
  - *Claim:* The transcript module passes against the corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and that manifest cases were driven by `fixtures/persistence/`.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) backs the manifest write; confirm `atomic::writes_via_temp_and_rename` still passes for manifest replacement : ☐ (PRESERVED / REGRESSION)

## Residue

Loading tolerances and orphan repair are Task 25; migration is Task 26.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
