# Done Certificate — Task 05: Domain error taxonomy and resource bounds

**Task:** [05-domain_errors_and_resource_bounds.md](05-domain_errors_and_resource_bounds.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 05. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 05) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one typed error enum per crate boundary plus every `01-domain-model.md` resource bound as a units-last named constant that is observable when reached.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not rename a spec-named bound: `01-domain-model.md` §Resource bounds names are load-bearing for the boundary fixtures every later task writes.

## Obligations

- **O1 — All nine `01-domain-model.md` §Resource bounds constants exist with the spec's exact names and default values**
  - *Claim:* `bounds.rs` exports the nine names verbatim with the nine documented defaults.
  - *Evidence to collect:* Read `crates/lotta-domain/src/bounds.rs` and compare each `pub const` name and value against the `01-domain-model.md` §Resource bounds table row by row — expect nine exact matches. Run `cargo nextest run -p lotta-domain -E 'test(bounds::matches_spec_table)'` — expect PASS.
  - *Checks:* Resolve `CONNECTIONS_MAX` — confirm it is the WebSocket connection cap of `01-domain-model.md` §Resource bounds (1,024). `NAME SHADOWING` risk: `06-model-providers.md` §Limits defines a distinct provider cap `PROVIDERS_MAX` (128); confirm no second `CONNECTIONS_MAX` is introduced for provider connections.
  - *Status:* ☐ unverified

- **O2 — Every bound name puts units last and no numeric bound literal appears outside `bounds.rs`**
  - *Claim:* The exported constant names satisfy `development-guidelines.md` §Naming, and grep finds no magic bound literals in other modules.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(bounds::units_last_naming)'` — expect PASS and read the test to confirm it rejects `MAX_*` prefixed names. Grep `crates/lotta-domain/src/` excluding `bounds.rs` for the nine default values as bare literals — expect zero matches.
  - *Status:* ☐ unverified

- **O3 — Reaching a bound produces a structured event and counter identifying the bound by name, and no bound silently drops input**
  - *Claim:* Each bound exposes an observable identifier used when the limit is reached, and the soft queue tier is the only bound permitted to replace work.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(bounds::at_limit_is_observable)'` — expect PASS. Read the test and confirm it asserts a named event for each of the nine bounds, and that only `QUEUE_ITEMS_SOFT_MAX` is classified as a coalescing soft limit per `01-domain-model.md` §Assumptions *Queue compatibility*.
  - *Status:* ☐ unverified

- **O4 — `Secret<T>` redacts through `Debug` and `Display`, and the error enum exposes a stable code per variant**
  - *Claim:* Formatting a `Secret` never emits its inner value, and every error variant maps to a stable string code.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(secret::redacts) + test(errors::stable_codes)'` — expect both PASS. Read the redaction test and confirm it formats a `Secret::new("sk-live-123")` through `{:?}` and `{}` and asserts the literal `sk-live-123` is absent from both outputs.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(bounds::) + test(errors::) + test(secret::)'` and sees the spec-table match, units-last naming, at-limit observability, and secret redaction pass**
  - *Claim:* The bounds, errors, and secret test modules pass with the spec-table comparison included.
  - *Evidence to collect:* Run the filter and confirm zero failures and a passing `bounds::matches_spec_table` case.
  - *Status:* ☐ unverified

## Regression check

- Task 02 scalar validation errors are wrapped by the new boundary enums; run `cargo nextest run -p lotta-domain -E 'test(scalars::) | test(errors::)'` and confirm original scalar cases retain their exact outcomes : ☐ (PRESERVED / REGRESSION)

## Residue

Bounds owned by other pages (`02` transport, `03` runtime, `04` durability, `05` tools, `06` providers, `07` operations) are defined by the tasks that own those surfaces, each in its own crate, not here.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
