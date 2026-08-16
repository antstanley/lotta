# Done Certificate — Task 31: Cross-runtime persistence conformance

**Task:** [31-cross_runtime_persistence_conformance.md](31-cross_runtime_persistence_conformance.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-15 — done

> Verification protocol for Task 31. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 31) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** proof that TypeScript and Rust round-trip the backend root, provider auth, schedules, settings, and channel side stores in both directions without loss.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not test concurrent mixed-runtime writes as supported: `04-persistence-and-memfs.md` §Agent and conversation records states they are unsupported and compatibility means sequential round trips or a quiesced handoff.

## Obligations

- **O1 — Every backend-root artifact round-trips in both directions with no lost field**
  - *Claim:* Rust-written state loads in TypeScript and vice versa for agent JSON, conversation JSON, transcript JSONL, manifest, and `system-prompt.json`.
  - *Evidence to collect:* Run `cargo nextest run --test cross_runtime -E 'test(backend_root::)'` — expect two directions per artifact (ten cases). On failure the harness must name the lost field and its JSON path.
  - *Checks:* Resolve which implementation performs each phase's writes — confirm the TypeScript phase uses `../letta-code/src/backend/local/local-store.ts` and the Rust phase uses `crates/lotta-store`. If both phases resolve to the same implementation, the round trip is vacuous.
  - *Status:* ☒ SATISFIED

- **O2 — Every side store round-trips in both directions, including `providers/auth.json` v1 and a channel tree**
  - *Claim:* The five side-store classes survive both handoff directions.
  - *Evidence to collect:* Run `cargo nextest run --test cross_runtime -E 'test(side_stores::)'` — expect two directions per class (ten cases).
  - *Status:* ☒ SATISFIED

- **O3 — All seven §Migration cross-runtime cases pass, including corrupt/unsupported manifests rejected without mutation**
  - *Claim:* Each numbered case behaves as §Migration specifies, and the corrupt case leaves the tree byte-identical.
  - *Evidence to collect:* Run `cargo nextest run --test cross_runtime -E 'test(migration_cases::)'` — expect seven cases; read case 7 and confirm it hashes the tree before and after.
  - *Status:* ☒ SATISFIED

- **O4 — The harness performs a quiesced sequential handoff and fails loudly if both runtimes would write concurrently**
  - *Claim:* The harness stops one runtime before starting the other and asserts no overlap.
  - *Evidence to collect:* Read `tests/conformance/harness/ts_runner.mjs` and confirm it waits for process exit before the Rust phase begins. Run `cargo nextest run --test cross_runtime -E 'test(harness::no_concurrent_writers)'` — expect PASS.
  - *Status:* ☒ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test cross_runtime` and sees all backend-root, side-store, and migration cases pass in both directions**
  - *Claim:* The cross-runtime suite is green with both directions covered for every artifact class.
  - *Evidence to collect:* Run the command and confirm zero failures and at least 27 cases in the summary.
  - *Status:* ☒ SATISFIED

## Verification record

- `cargo nextest run --test cross_runtime -E 'test(backend_root::)'`: 10/10.
- `cargo nextest run --test cross_runtime -E 'test(side_stores::)'`: 10/10.
- `cargo nextest run --test cross_runtime -E 'test(migration_cases::)'`: 7/7.
- `cargo nextest run --test cross_runtime -E 'test(harness::)'`: 4/4.
- `cargo nextest run --test cross_runtime`: 35/35 twice.
- `cargo nextest run --workspace --all-features`: 968/968.
- Store regressions: 28/28; Task 30 `prompt::record_shape`: 1/1.
- `cargo fmt --all --check`, strict all-target/all-feature Clippy, strict private Rustdoc, and `cargo deny check`: pass.
- Pinned TypeScript commit: `300f923f16cc8eee50656d7da732902c1dea2b65`; tracked tree clean; frozen Bun install and source/lock/package SHA checks pass.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/` writers (Tasks 22–27) are exercised here; confirm `agent::corpus_round_trip`, `transcript::append`, and `side::paths` still pass : ☒ PRESERVED
- `crates/lotta-memfs/src/prompt/` (Task 30) records are round-tripped; confirm `prompt::record_shape` still passes : ☒ PRESERVED

## Residue

This suite satisfies `00-overview.md` §Implementation acceptance criterion 3. Criteria 1, 2, 4, 5, and 6 are Tasks 01, 10, 90/91, 88, and 89 respectively, gathered in Task 94.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: All six obligations and both regressions are satisfied. Actual pinned TypeScript and public Rust APIs round-trip all backend and side-store classes; all seven canonical migration groups pass; live two-way handoff exclusion, bounded process/tree safety, exact pinned provenance, and frozen dependency reproducibility are proven. Evidence: backend 10/10, side stores 10/10, migration 7/7, harness 4/4, broad cross-runtime 35/35 twice, workspace 968/968, store regressions 28/28, Task 30 record 1/1, and every strict gate passes.
