# Done Certificate — Task 23: Agent and conversation record persistence

**Task:** [23-store_agent_and_conversation_records.md](23-store_agent_and_conversation_records.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 23. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 23) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** agent JSON and conversation snapshots that round-trip against the checked-in corpus, preserve unknown fields, and refresh on external mtime change.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not drop unknown fields: `04-persistence-and-memfs.md` §Agent and conversation records requires preserving them so a newer reference runtime does not lose data when Lotta touches a known field.

## Obligations

- **O1 — Every agent and conversation fixture in `fixtures/persistence/` reads, re-serializes, and compares equal to its source**
  - *Claim:* Round-tripping each corpus record produces byte-equal JSON apart from documented formatting normalization.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(agent::corpus_round_trip) + test(conversation::corpus_round_trip)'` — expect one case per fixture record and zero failures.
  - *Status:* ☐ unverified

- **O2 — Unknown compatible fields survive read-modify-write on both record types**
  - *Claim:* A field absent from the Rust type is present and unchanged after a known field is modified and the record rewritten.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(agent::preserves_unknown) + test(conversation::preserves_unknown)'` — expect both PASS. Trace: read fixture with `future_flag` → set `name` → write → re-read → assert `future_flag` unchanged.
  - *Status:* ☐ unverified

- **O3 — An external write is detected by mtime and refreshes the loaded snapshot**
  - *Claim:* After another writer changes the file, the next read returns the new content rather than a cached snapshot.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(refresh::detects_external_write)'` — expect PASS; the test advances the fake clock and rewrites the file out of band.
  - *Checks:* Resolve the mtime source — confirm it is the filesystem metadata read, not a cached value captured at load. A cached comparison would make the refresh a no-op.
  - *Status:* ☐ unverified

- **O4 — Archiving sets `archived_at`, unarchiving clears it, a conversation model override never mutates the agent, and `AGENTS_MAX`/`CONVERSATIONS_PER_AGENT_MAX` reject at the limit**
  - *Claim:* The archival machine matches `01-domain-model.md` §Conversation archival and the two creation bounds reject above the limit while accepting below it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(conversation::archival) + test(bounds::creation_caps)'` — expect the archive/unarchive pair, the override-isolation case, and below/at/above cases for both caps.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(agent::) + test(conversation::) + test(refresh::)'` and sees corpus round-trips, unknown-field preservation, external-write refresh, archival, and creation caps pass**
  - *Claim:* The agent, conversation, and refresh modules pass against the corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and that the round-trip case count equals the corpus record count.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) performs these writes; confirm `atomic::external_change_yields_storage_conflict` still passes when driven through the record writers : ☐ (PRESERVED / REGRESSION)

## Residue

Query behaviors over these records (ordering, filters, cursors) are Task 28.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
