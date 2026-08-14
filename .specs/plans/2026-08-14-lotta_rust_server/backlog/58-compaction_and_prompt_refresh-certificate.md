# Done Certificate — Task 58: Compaction modes, triggers, and prompt refresh

**Task:** [58-compaction_and_prompt_refresh.md](58-compaction_and_prompt_refresh.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 58. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 58) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** both compaction modes with all three triggers, mod lifecycle callbacks, before/after counts, and revision-driven prompt refresh.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not rewrite the transcript: `04-persistence-and-memfs.md` §Transcript contract states compaction appends one entry and does not back up or rewrite the active transcript.

## Obligations

- **O1 — Both compaction modes exist and produce their documented shape: `all` yields one summary message; `sliding_window` retains the configured recent percentage**
  - *Claim:* The mode enum has two variants and each produces the documented retained set.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(compaction::modes)'` — expect two cases; the sliding-window case asserts the retained percentage against the configured value.
  - *Status:* ☐ unverified

- **O2 — All three triggers fire compaction: manual request, pre-call pressure, and provider-reported overflow**
  - *Claim:* Each trigger independently produces a compaction, and `CONTEXT_OVERFLOW_COMPACTIONS_MAX` bounds the overflow-driven path.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(compaction::triggers)'` — expect three trigger cases plus a bound case naming `CONTEXT_OVERFLOW_COMPACTIONS_MAX` from `03-runtime-and-turns.md` §Runtime bounds.
  - *Checks:* Resolve the threshold used by the pre-call pressure trigger — confirm it derives from the Task 53 effective context window (the minimum of four sources), not an independent compaction-token constant.
  - *Status:* ☐ unverified

- **O3 — Compaction is serialized with the turn lease, emits mod lifecycle callbacks, writes one transcript compaction entry, updates in-context IDs, and recompiles the prompt**
  - *Claim:* A compaction under an active lease performs all five effects, and a stale lease performs none.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(compaction::effects)'` — expect five effect assertions plus `stale_lease_compaction_is_suppressed`.
  - *Status:* ☐ unverified

- **O4 — Before/after token and message counts are recorded on the compaction entry**
  - *Claim:* The stored entry carries `tokensBefore` and the details needed to compare before and after message counts.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(compaction::records_counts)'` — expect PASS asserting the persisted entry matches `$defs.CompactionEntry` with populated counts.
  - *Status:* ☐ unverified

- **O5 — A committed MemFS revision change recompiles the prompt before the next turn, and an uncommitted change does not**
  - *Claim:* Committing a memory edit forces recompilation; editing without committing does not.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(compaction::prompt_refresh)'` — expect `committed_change_recompiles` and `uncommitted_change_does_not`.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(compaction::)'` and sees both modes, all three triggers, the five lease-serialized effects, recorded counts, and revision-driven refresh pass**
  - *Claim:* The compaction module passes with both modes covered.
  - *Evidence to collect:* Run the filter and confirm zero failures and three `triggers` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/transcript/append.rs` (Task 24) writes the compaction entry; confirm `transcript::no_rewrite_on_compaction` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-extensions/src/mods/host.rs` (Task 45) receives the lifecycle callbacks; confirm `mods::lifecycle` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Repeated overflow terminality and the four-source window minimum are Task 53; this task consumes them.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
