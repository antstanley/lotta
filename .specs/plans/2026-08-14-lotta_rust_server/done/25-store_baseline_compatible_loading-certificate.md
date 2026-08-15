# Done Certificate — Task 25: Baseline-compatible transcript loading and active-projection repair

**Task:** [25-store_baseline_compatible_loading.md](25-store_baseline_compatible_loading.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-15 — DONE

> Verification protocol for Task 25. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 25) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a loader that accepts everything the reference accepts and repairs the active projection without mutating the source transcript.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not reject state the baseline accepts: `04-persistence-and-memfs.md` §Assumptions *Transcript tolerance* requires loading what the baseline loads and verifying more strictly only out of band.

## Obligations

- **O1 — The loader accepts all four baseline tolerances: missing session header, duplicate entry IDs, unverified parent links, and compaction references without a retained-entry graph**
  - *Claim:* Each tolerance fixture loads without error and produces the expected active projection.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::load::tolerances)'` — expect four passing cases named for the four tolerances, each driven by its `fixtures/persistence/` case.
  - *Status:* ☑ SATISFIED — exact selector passed **4/4 twice** after final remediation.
    The four independently mutated, mutation-sensitive cases derive from Task 11 corpus case 5
    (`baseline_tolerated_versioned_rows`) and prove missing session, duplicate outer entry ID,
    unverified parent, and compaction-without-retained-graph behavior without source mutation.

- **O2 — Orphan tool results are removed from the active projection, `in_context_message_ids` is repaired, `conversation.json` is persisted, and the source transcript is byte-unchanged**
  - *Claim:* The repair changes only the conversation snapshot.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::repair::orphan_tool_results)'` — expect PASS; read the test and confirm it hashes `messages.jsonl` before and after and asserts equality.
  - *Checks:* Resolve the write call in the repair path — confirm it targets `conversation.json` only. A write to the transcript writer here would contradict §Baseline-compatible loading.
  - *Status:* ☑ SATISFIED — exact selector passed **2/2 twice**. Corpus case 6 proves exact active
    IDs, public conversation reload, unknown-field preservation, transcript SHA-256/bytes/inode
    equality, unchanged manifest/listing, and conversation-only persistence. The second case forces
    an external conversation replacement at the repair boundary and proves typed conflict with the
    external bytes preserved. `repair::persist_context` writes only `conversation.json`.

- **O3 — An unversioned non-empty transcript maps to `transcript_migration_required` and a versioned transcript with legacy UI-message rows maps to `transcript_repair_required`**
  - *Claim:* The two baseline error classes produce the two named Lotta codes and no mutation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::load::error_mapping)'` — expect both cases to pass and to assert the on-disk bytes are unchanged.
  - *Status:* ☑ SATISFIED — exact selector passed **3/3 twice**: unversioned non-empty maps to
    `transcript_migration_required`, embedded versioned legacy UI maps to
    `transcript_repair_required`, and whitespace-only unversioned state loads empty; every case
    proves complete source snapshot equality.

- **O4 — A versioned legacy `pi-ai-message-jsonl` transcript is upgraded on its next non-empty persistence, and oversized tool results follow the baseline clipping path**
  - *Claim:* The legacy-format upgrade happens automatically on persistence, and an oversized tool result is clipped rather than rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(transcript::load::legacy_upgrade) + test(transcript::repair::oversized_tool_result)'` — expect both PASS, the first asserting the manifest's `message_format` changed only after a non-empty write.
  - *Status:* ☑ SATISFIED — legacy-upgrade selector passed **4/4 twice** and oversized-result
    selector passed **1/1 twice**. Corpus case 4 proves load/empty-persistence no-op, durable exact
    timestamped backup, schema-2 rewrite, context coherence, and public reload. Manifest conflict
    and post-rename parent-sync failures restore a loadable exact legacy pair while retaining the
    recovery backup. Public projection tests prove below/at/above 40,000 UTF-16 units plus
    scalar-safe Unicode clipping and unchanged source files.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — final gates all exited 0: format; strict all-target/all-feature
    workspace Clippy; Rustdoc with `-D warnings`; `cargo deny check`; source/dependency audits
    **4/4**; `lotta-store` **110/110 twice**; workspace **766/766 twice**; and the required
    all-feature workspace run **766/766**. Task-added production modules are below 1,000 lines,
    functions at most 70 lines, lines at most 100 columns, and constants use unit-last names.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(transcript::load::) + test(transcript::repair::)'` and sees the four tolerances, orphan repair with an unchanged source, both error mappings, and the legacy upgrade pass**
  - *Claim:* The loading and repair modules pass against the corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and four tolerance cases.
  - *Status:* ☑ SATISFIED — exact broad selector passed **21/21 twice** after final remediation,
    including all four tolerance identities, both orphan paths, all three mappings, all four
    legacy-upgrade paths, clipping, and Task 11 interrupted/corrupt corpus recovery.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/transcript/append.rs` (Task 24) writes the entries this loader reads;
  `transcript::append` passed **6/6 twice**, complete `transcript::` passed **53/53**, and Task 24's
  migration/fork/loaded rewrite-authority tests remain green: ☑ **PRESERVED**

## Residue

The stricter diagnostics are Task 26's `verify` command, which must not tighten load-time rejection.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: All six obligations and the Task 24 regression check have execution and source evidence.
The final independent clean OpenAI review (`task_199`) returned `VERDICT: CORRECT / DONE` after
the recoverable two-file legacy-upgrade protocol remediated the only final review blockers.
