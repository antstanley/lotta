# Done Certificate — Task 27: State outside the backend root

**Task:** [27-store_side_stores.md](27-store_side_stores.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 27. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 27) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** read/write access to `settings.json`, `crons.json`, schedule run logs, the channel store tree, and project-local settings at their exact baseline paths.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not relocate baseline side-store paths: `04-persistence-and-memfs.md` §State outside the backend root fixes them and cross-runtime backup coverage depends on them.

## Obligations

- **O1 — Every path named in §State outside the backend root resolves and round-trips against its fixture**
  - *Claim:* All eleven listed paths read and write at the exact locations, including the `LETTA_HOME` override.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(side::paths)'` — expect one case per path in the §State outside the backend root listing, plus a `letta_home_override` case.
  - *Status:* ☐ unverified

- **O2 — Plugin-owned files below a channel directory are preserved opaquely across a read-modify-write of a known file**
  - *Claim:* Writing `accounts.json` leaves an unrecognized sibling file untouched and still enumerated.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(side::channels::preserves_plugin_files)'` — expect PASS; the test creates `plugin-state.bin`, rewrites `accounts.json`, and asserts the sibling's bytes and mtime survive.
  - *Status:* ☐ unverified

- **O3 — Project-local settings are read only when their workspace is in scope, and never merged from an out-of-scope workspace**
  - *Claim:* An out-of-scope `<workspace>/.letta/settings.json` is not read.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(side::project::scope_gate)'` — expect PASS; the test asserts zero reads of the out-of-scope path via the fake filesystem's access log.
  - *Status:* ☐ unverified

- **O4 — Every side-store write goes through the atomic writer, so an external change yields `storage_conflict` rather than a silent overwrite**
  - *Claim:* No side-store module calls a direct filesystem write.
  - *Evidence to collect:* Grep `crates/lotta-store/src/side/` for `fs::write` and `File::create` — expect zero matches outside the atomic module. Run `cargo nextest run -p lotta-store -E 'test(side::conflict)'` — expect PASS.
  - *Checks:* Resolve the write call in each side module — confirm it resolves to `crate::atomic::write_atomic`, not to `std::fs::write`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(side::)'` and sees every side-store path round-trip, plugin files preserved, project scope gating, and atomic writes pass**
  - *Claim:* The side-store module passes against the corpus.
  - *Evidence to collect:* Run the filter and confirm zero failures and one case per listed path.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) now serves side stores; confirm `atomic::modes` still passes for the `providers/` directory : ☐ (PRESERVED / REGRESSION)

## Residue

Schedule semantics over `crons.json` are Task 60; channel semantics over the channel tree are Tasks 79–82. This task owns only the paths, encodings, and durability.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
