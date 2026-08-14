# Done Certificate — Task 05: Domain error taxonomy and resource bounds

**Task:** [05-domain_errors_and_resource_bounds.md](05-domain_errors_and_resource_bounds.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-14 — independent clean verifier, remediation loop 2

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
  - *Fresh loop-2 evidence (2026-08-14):* `bounds.rs:77-163` defines exactly the nine public spec-named immutable `ResourceBound` constants in specification order with values 1,024; 4,096; 256; 100; 300; 128; 256; 2,000; and 2,000,000. `RESOURCE_BOUNDS` contains those nine rows and no others. Repository/spec search resolves `CONNECTIONS_MAX` only to the WebSocket cap; provider capacity remains the distinct future `PROVIDERS_MAX` (128). The exact selector `test(bounds::matches_spec_table)` selected **1 intended case** and passed.
  - *Status:* SATISFIED

- **O2 — Every bound name puts units last and no numeric bound literal appears outside `bounds.rs`**
  - *Claim:* The exported constant names satisfy `development-guidelines.md` §Naming, and grep finds no magic bound literals in other modules.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(bounds::units_last_naming)'` — expect PASS and read the test to confirm it rejects `MAX_*` prefixed names. Run the companion bound-literal lint and confirm it scans production bound declarations and limit checks outside `bounds.rs`, while ordinary IDs, fixture values, type names such as `u128`, and non-bound arithmetic are not treated as declarations.
  - *Fresh loop-2 evidence (2026-08-14):* The exact selector `test(bounds::units_last_naming)` selected **1 intended case** and passed. `bounds.rs:165-557` now uses dev-only `syn` full AST visitation rather than line scanning. `rust_sources` recursively visits every `.rs` production module under `src`, excluding only `bounds.rs` and filenames exactly `tests.rs`/`*_tests.rs`; item visitation skips real `#[cfg(test)]` items and continues afterward. Const/static checks are syntax-aware and catch attributed, multiline, qualified-type, nested-expression declarations. Binary/call/method visitors cover forward/reverse length or capacity comparisons and numeric capacity/limit operations; assertion macros are inspected rather than exempted. Seven scanner tests passed, including repository scan and fixtures that assert exact diagnostic rule lists for multiline/qualified declarations, multiline method/function capacities, reversed comparisons, assertions, cfg(test) continuation, and benign IDs/u128/`checked_add(1)`/named bounds. Independent bypass inspection confirms casts, parentheses, negative/suffixed/underscored integer literals, associated constants, and nested expressions are either traversed and caught when raw numeric or correctly accepted when named; qualified assertion macros resolve by final segment. No simple false negative or material false positive remains for the stated lint scope.
  - *Status:* SATISFIED

- **O3 — Reaching a bound produces a structured event and counter identifying the bound by name, and no bound silently drops input**
  - *Claim:* Each bound exposes an observable identifier used when the limit is reached; the soft queue tier is the only admission bound permitted to replace pending work, while the two schedule-log bounds explicitly rotate old log data.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(bounds::at_limit_is_observable)'` — expect PASS. Read the test and confirm it asserts a named event and counter for each of the nine bounds, classifies only `QUEUE_ITEMS_SOFT_MAX` as coalescing pending work, classifies the schedule-log pair as rotation, and makes every other limit reject or fail observably.
  - *Fresh loop-2 evidence (2026-08-14):* `ResourceBound` immutably carries name/value/event/counter/action (`bounds.rs:14-27`), and `observe` returns `None` below the bound plus a complete `BoundObservation` at/equal and over (`bounds.rs:29-61`). The exact selector `test(bounds::at_limit_is_observable)` selected **1 intended case** and passed. Its explicit nine-row table verifies exact name/value/event/counter/action tuples, below/at/over semantics, and action counts **5 Reject / 1 CoalescePendingWork / 1 FailInvariant / 2 RotateLog**. Therefore the coalescer, approval invariant failure, five rejects, and both schedule rotations are row-exact and no silent-drop or metadata drift path exists.
  - *Status:* SATISFIED

- **O4 — `Secret<T>` redacts through `Debug` and `Display`, and the error enum exposes a stable code per variant**
  - *Claim:* Formatting a `Secret` never emits its inner value, and every error variant maps to a stable string code.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(secret::redacts) + test(errors::stable_codes)'` — expect both PASS. Read the redaction test and confirm it formats a `Secret::new("sk-live-123")` through `{:?}` and `{}` and asserts the literal `sk-live-123` is absent from both outputs.
  - *Fresh loop-2 evidence (2026-08-14):* Mechanical API audit finds exactly one public lotta-domain error type: `DomainError` (`errors.rs:9-60`), exported alone at `lib.rs:20`; there are no aliases, legacy error exports, or leaked alternate error types. All Task 02-04 public domain failure APIs return `DomainError`. Its 14 contextual variants are exhaustively matched to 14 explicit, unique stable code literals (`errors.rs:65-81`), and the exact stable-code test passed. `Secret<T>` exposes only `new`, keeps storage private, implements no accessor/Deref/AsRef/Borrow/expose/into-inner conversion, and writes the redaction literal without formatting `T`; panicking `Debug` and `Display` sentinels prove inner formatters are never invoked. The exact combined selector selected **2 intended cases** and passed both.
  - *Status:* SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Fresh loop-2 evidence (2026-08-14):* Full workspace nextest passed **78/78**. `cargo fmt --all --check`, Clippy workspace/all-targets/all-features with `-D warnings`, `cargo deny check`, rustdoc workspace/all-features/no-deps with `RUSTDOCFLAGS=-D warnings`, doc tests, and changed-file whitespace checks all exited 0. Source audit found no production unsafe or direct clock, no new forbidden external-input panic/unwrap/expect path, no over-100-column line, and no unreasoned production hard-limit violation. The ten pre-existing semantic/schema bounds are centralized at `bounds.rs:141-150`; all consumers import them, resource const-generics use canonical metadata `.value`, and canonical maxima remain tags 1,024, permission suggestions 128, diffs 1,024, subscriptions 256, and queue 300. `syn` is a necessary test scanner dependency, declared only under `[dev-dependencies]`; normal dependency tree excludes syn 2. The AST scanner is proportionate to the syntax claim and introduces no production dependency or behavior.
  - *Status:* SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(bounds::) + test(errors::) + test(secret::)'` and sees the spec-table match, units-last naming, at-limit observability, and secret redaction pass**
  - *Claim:* The bounds, errors, and secret test modules pass with the spec-table comparison included.
  - *Evidence to collect:* Run the filter and confirm zero failures and a passing `bounds::matches_spec_table` case.
  - *Fresh loop-2 evidence (2026-08-14):* The exact broad review selector ran **12 nonzero intended cases** and passed all: 10 bounds/scanner cases, 1 errors case, and 1 secret case. It includes the exact spec-table, units-last, observation, repository scanner, stable-code, and redaction checks.
  - *Status:* SATISFIED

## Regression check

- Task 02–04 ID, scalar, scope, entity, and runtime/lifecycle behavior is consolidated behind the new boundary enum; corrected exact selector ran **65 nonzero cases**, all passed: **PRESERVED**. Source traces confirm all public domain failure APIs return `DomainError` while state atomicity, wire names, schema maxima, serialization, and lifecycle regressions remain green.

## Residue

Bounds owned by other pages (`02` transport, `03` runtime, `04` durability, `05` tools, `06` providers, `07` operations) are defined by the tasks that own those surfaces, each in its own crate, not here.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: DONE
CONFIDENCE: high
SUMMARY: O1-O6 are SATISFIED with fresh loop-2 evidence, every exact selector selects a nonzero intended set and passes, the syn scanner closes the demonstrated syntax/assertion bypasses with diagnostic fixtures and a green recursive repository scan, all workspace gates pass, and Task 02-04 regression is PRESERVED.

## Independent correctness review

- **Public error count:** 1 public `Error` type (`DomainError`), 14 variants, 14 unique explicit stable codes; no aliases, legacy exports, or public leaks.
- **Resource-bound count:** 9 public canonical immutable resource constants plus 10 centralized private domain/schema safety bounds, with consumers and canonical maxima aligned.
- **Ponytail full necessity/simplicity:** one error enum, compact immutable bound metadata/observation, and formatter-independent `Secret<T>` are direct minimal abstractions. The dev-only syn AST visitor is justified by Rust syntax and is substantially safer than duplicating a partial lexer; focused visitors and fixtures keep it bounded to the lint claim. No production overengineering or dependency impact was found.
- **Execution traces:** empty ID reaches `DomainError::EmptyId { kind }` and code `domain.id.empty`; each resource returns no observation below, an exact structured row at equality, and `exceeded=true` above; queue const-generics resolve to `QUEUE_ITEMS_HARD_MAX.value == 300`; `Secret::new(PanicsIfFormatted)` formats to `[REDACTED]` without invoking `T`.
- **Correctness verdict:** CORRECT (high confidence). Independent full-diff, API, bounds, secret, scanner-bypass, hard-limit, forbidden-call, dependency, selector, regression, and repository-gate audits found no remaining Task 05 defect. Minimum remediation: none.
