# Done Certificate — Task 24: Transcript manifest, session header, and entry append

**Task:** [24-store_transcript_contract.md](24-store_transcript_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-15 — DONE

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
  - *Evidence:* The exact manifest selector passed 6/6 twice. `writes_exact_shape` drives public initialization and public read for the exact required-only and all-optional shapes. `reads_corpus_manifests` classifies all 12 authoritative Task 11 manifests exactly: 9 accepted current/legacy and 3 unsupported rejected as `Parse`. Public canonical-path tests reject duplicate keys, unknown keys, and explicit null for every non-nullable optional field. Oversized initialization returns `Limit` without residue.
  - *Status:* ☒ SATISFIED

- **O2 — A new transcript starts with exactly one session header at version 3, and subsequent rows are message or compaction entries only**
  - *Claim:* Row 0 is the session header; no second header is ever appended.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::session_header)'` — expect `written_once_at_row_zero` and `never_appended_again` to pass.
  - *Evidence:* The exact session selector passed 2/2 twice. `written_once_at_row_zero` independently parses one compact LF-terminated row with exactly the five canonical session fields and version 3. `never_appended_again` proves a second public initializer is `StorageConflict`, a public session append is `Parse`, full-rewrite sequence validation rejects another header, and manifest/message bytes remain unchanged.
  - *Status:* ☒ SATISFIED

- **O3 — Message and compaction entries append one complete line each with correct parent linkage, and the embedded `LocalMessage` timestamp is numeric milliseconds while the entry timestamp is RFC3339**
  - *Claim:* Appending N entries yields N lines, each parseable independently, with the two timestamp encodings distinct.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::append)'` — expect `one_line_per_entry`, `parent_chain_links`, and `timestamp_encodings_differ` to pass.
  - *Checks:* Resolve the timestamp serializer used for the embedded message — confirm it is the numeric-millisecond encoder, not the RFC3339 entry encoder. `NAME SHADOWING` risk: both are `Timestamp` values from Task 02 but serialize differently.
  - *Evidence:* The exact append selector passed 6/6 twice. The three named tests independently parse session, message, and compaction rows; prove null then linked `parentId`; prove optional compaction details present and absent; and parse the entry timestamp as RFC3339 while asserting the embedded `LocalMessage.timestamp` is the exact numeric millisecond value. Public below/at/above line and total-boundary tests pass, and a schema-1 manifest rejects append before mutation.
  - *Status:* ☒ SATISFIED

- **O4 — Compaction appends without rewriting or backing up the active transcript, and full rewrites occur only for migration, fork, and initial/full persistence**
  - *Claim:* A compaction leaves preceding lines byte-identical; a rewrite is reachable only from the three named paths.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::no_rewrite_on_compaction)'` — expect PASS with a byte comparison of the prefix. Grep the crate for the rewrite entry point and confirm exactly three callers.
  - *Evidence:* The exact no-rewrite selector passed twice. It proves the preceding byte prefix, manifest bytes, file identity, and directory listing are unchanged, with exactly one independently parseable compaction row added and no backup/temp residue. The private rewrite authority has exactly three real public callers: owned loaded persistence, fork persistence, and migrated persistence with an existing confined regular backup. Current manifests are required before mutation; legacy manifests reject all three paths. Staged rewrites stream one row at a time, use transcript-scale revision conflict detection, sync stage and parent, and clean only precommit residue. Deterministic boundary observers prove write/sync/conflict/rename/parent-sync behavior and no postcommit retry or deletion.
  - *Status:* ☒ SATISFIED

- **O5 — `TRANSCRIPT_LINE_BYTES_MAX` and `TRANSCRIPT_BYTES_MAX` are named constants rejecting append and read at the limit**
  - *Claim:* An 8 MiB + 1 line is rejected and a transcript at 16 GiB requires archive/export before append.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::bounds)'` — expect below/at/above cases for both bounds.
  - *Evidence:* The exact bounds selector passed 4/4 twice. The payload bound excludes LF and accepts 8 MiB exactly while rejecting +1; the total bound includes every LF, permits a public append reaching 16 GiB exactly, then rejects another append unchanged. The reusable production reader accepts below/exact payload rows, rejects +1, and rejects 16 GiB + 1 metadata before scanning. Transcript revision hashing accepts the exact total metadata bound, rejects +1 pre-scan, and publicly rewrites valid JSONL sources immediately below and above the shared atomic 8 MiB record limit using fallible constant 64 KiB checksum memory.
  - *Status:* ☒ SATISFIED

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Final workspace passed 744/744 twice; store passed 88/88 twice. Format, warning-denied workspace Clippy and Rustdoc, `cargo deny check`, all five source/dependency/hard-limit audits, pinned clean baseline, zero production `allow` attributes, 100-column lines, sub-1,000-line production modules, and 70-line functions passed. Task 22 atomic 14/14, paths 3/3, errors 2/2; Task 23 broad 31/31; Task 09 transcript contracts 2/2; Task 11 persistence 30/30; and Task 21 client-turn 8/8 were preserved.
  - *Status:* ☒ SATISFIED

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(transcript::)'` and sees manifest shape, single session header, append linkage, no-rewrite-on-compaction, and both byte bounds pass**
  - *Claim:* The transcript module passes against the corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and that manifest cases were driven by `fixtures/persistence/`.
  - *Evidence:* The broad transcript selector passed 31/31 twice, covering the 12-manifest authoritative corpus table, exact manifest/session/append shapes, no-rewrite compaction, public byte boundaries, reusable bounded reads, all three private rewrite authorities, legacy and backup negatives, symlink/contention safety, valid transcripts across the 8 MiB record threshold, 16 GiB metadata bounds, and deterministic staged-rewrite failure paths. Independent final reviewer `task_194` returned `VERDICT: CORRECT / DONE`.
  - *Status:* ☒ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) backs the manifest write; confirm `atomic::writes_via_temp_and_rename` still passes for manifest replacement : ☒ PRESERVED — Task 22 atomic passed 14/14, including the real manifest-path replacement regression without Task 24 test fixtures. Shared record writes remain capped at 8 MiB while the crate-private transcript sampler uses the explicit 16 GiB bound and constant-memory hashing.

## Residue

Loading tolerances and orphan repair are Task 25; migration is Task 26.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: O1–O7 and the Task 22 regression are satisfied by exact corpus-backed manifest compatibility, one canonical session header, append-only linked message/compaction rows with distinct timestamp encodings, structurally restricted durable streaming rewrites, exact public/read/revision byte boundaries, complete negative and failure-path evidence, full quality/regression gates, and clean independent certification.
