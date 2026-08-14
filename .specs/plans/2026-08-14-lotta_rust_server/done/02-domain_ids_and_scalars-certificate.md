# Done Certificate — Task 02: Domain IDs, scalar newtypes, and runtime scope

**Task:** [02-domain_ids_and_scalars.md](02-domain_ids_and_scalars.md) · **Plan:** [plan.md](../plan.md)
**State:** Final independent validation 2026-08-14 — done

> Verification protocol for Task 02. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 02) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** typed, opaque ID newtypes and validated scalars that accept every baseline ID form and never rewrite a client-supplied ID.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not narrow the set of conversation IDs the baseline accepts; `01-domain-model.md` §ID scheme requires arbitrary non-empty conversation IDs to survive unchanged.

## Obligations

- **O1 — Generated IDs are validated against the six `01-domain-model.md` §ID scheme prefixes, while arbitrary non-empty conversation IDs are accepted on input and returned byte-identical**
  - *Claim:* `ConversationId::generate` produces `local-conv-<n>`; `ConversationId::accept("anything")` succeeds and round-trips unchanged; `ConversationId::accept("")` fails.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(ids::)'` — expect the cases `generate_matches_prefix`, `accepts_arbitrary_baseline_id`, and `rejects_empty_id` to pass. Trace `ConversationId::accept("conv/../x")` and confirm it returns the same bytes rather than a normalized value.
  - *Checks:* Resolve the validation call inside `accept` — confirm it is the input-side `is_non_empty` check, not the generation-side `validate_prefix`. `NAME SHADOWING` risk: both are free functions in `ids.rs`.
  - *Evidence:* Final clean review ran `cargo nextest run --manifest-path /Volumes/Delorean/code/five-letters/lotta/Cargo.toml -p lotta-domain -E 'test(ids::tests::) + test(scope::tests::) + test(scalars::tests::)'`: 14 passed, 0 skipped. `ids.rs:75-111,210-242` shows every accepted ID newtype rejects only empty and retains the owned string unchanged, while generated forms use separate prefix helpers. `ids.rs:113-125` requires `Uuid::get_version() == Some(Version::Random)` for local agents; `ids.rs:128-208` maps all six prefixes to the correct entity APIs. Tests at `ids.rs:249-320` cover all forms, UUID-v4 rejection, arbitrary `"conv/../x\0with spaces"` preservation, empty rejection, and sequence boundaries; property tests at `ids.rs:322-347` cover generated sequence forms and byte-identical arbitrary non-empty conversation IDs.
  - *Status:* ☑ SATISFIED

- **O2 — `default` is agent-scoped: two `RuntimeScope` values sharing `conversation_id = "default"` but differing in `agent_id` are distinct keys**
  - *Claim:* `RuntimeScope` hashes and compares on the `(agent_id, conversation_id)` pair, so `default` never collides across agents.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(scope::default_is_agent_scoped)'` — expect PASS, and read the test to confirm it inserts two scopes with the same `default` conversation and different agents into a `HashMap` and asserts `len() == 2`.
  - *Evidence:* The final targeted run passed `scope::tests::default_is_agent_scoped`. `scope.rs:48-60` defines equality and hashing over exactly `(agent_id, conversation_id)`; `scope.rs:84-100` inserts two `default` scopes with different agents into a `HashMap` and observes two keys. This matches `01-domain-model.md` §ID scheme and the reference scope normalization's agent-scoped `default`.
  - *Status:* ☑ SATISFIED

- **O3 — `RuntimeScope` carries optional `acting_user_id` and serializes to the `canonical-types.schema.json` `$defs.RuntimeScope` shape**
  - *Claim:* Serializing a `RuntimeScope` yields exactly `agent_id`, `conversation_id`, and (when present) `acting_user_id`, with the first two required.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(scope::schema_shape)'` — expect PASS. Read the test and confirm it validates the serialized JSON against `$defs.RuntimeScope` in `.specs/canonical-types.schema.json` rather than against a hand-written literal.
  - *Evidence:* The final targeted run passed `scope::tests::schema_shape` and `acting_user_is_not_runtime_identity`. `scope.rs:10-19` carries optional `acting_user_id` and omits it only when absent. `scope.rs:119-156` loads the checked-in canonical schema and applies an actual `#/$defs/RuntimeScope` reference before validating the serialized value; it also asserts the three-field present form. `scope.rs:102-115` proves attribution does not alter identity, matching the reference `types/runtime-scope.ts`.
  - *Status:* ☑ SATISFIED

- **O4 — `Timestamp` is RFC3339 UTC, constructed only through a `Clock` value, and `NonEmptyString` rejects the empty string**
  - *Claim:* There is no constructor on `Timestamp` that reads the system clock, and `NonEmptyString::new("")` returns an error.
  - *Evidence to collect:* Grep `crates/lotta-domain/src/` for `SystemTime::now`, `Utc::now`, and `Instant::now` — expect zero matches. Run `cargo nextest run -p lotta-domain -E 'test(scalars::)'` — expect `rejects_empty` and `rfc3339_round_trip` to pass.
  - *Evidence:* Final source audit found zero `SystemTime::now`, `Utc::now`, `Instant::now`, `Local::now`, or `std::time::SystemTime` uses under `crates/**/*.rs`. `scalars.rs:21-60` rejects exactly the empty string, does not trim, and preserves bytes; concrete whitespace-only tests and the non-empty property test passed. `scalars.rs:62-152` keeps `Timestamp`'s field private, exposes only Clock-mediated runtime `now`/`parse`, accepts RFC3339 zero offset (`Z` and `+00:00`), rejects nonzero offsets/malformed values, and serializes via `SecondsFormat::AutoSi` with canonical `Z`. The final targeted run passed all four scalar tests.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Final clean PASS with explicit manifest path: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; full `cargo nextest run --workspace --all-features` (15 passed, 0 skipped); `cargo deny check` (`advisories`, `bans`, `licenses`, `sources` all ok); and `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`. (`cargo audit` is not installed, but the required deny advisory gate passed.) Programmatic audits found zero changed Rust lines over 100 columns and every function at or below 70 lines. New constants are named `const`s; production has no `unwrap`, `expect`, `panic!`, unsafe block, or direct clock. Production assertions are documented invariant checks. Runtime dependencies are workspace-managed and narrow; schema/property/JSON dependencies are dev-only.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(ids::) + test(scope::) + test(scalars::)'` and sees generation validation, arbitrary-ID acceptance, agent-scoped `default`, and clock-free timestamps pass**
  - *Claim:* The three filtered test modules pass with no ignored cases.
  - *Evidence to collect:* Run the named filter and confirm a non-zero test count with zero failures and zero skips in the nextest summary.
  - *Evidence:* The exact task selector, run with explicit manifest path, selected 14 tests: 14 passed, 0 skipped. The unambiguous selector `test(ids::tests::) + test(scope::tests::) + test(scalars::tests::)` independently selected the same 14 tests: seven ID tests, three scope tests, and four scalar tests, including both property tests and all named behaviors.
  - *Status:* ☑ SATISFIED

## Regression check

Independent `@-` → `@` inspection covered the entire Task 02 change: task/certificate move, workspace and domain dependency declarations, lockfile, domain exports, and all new ID/scalar/scope code. No protocol, runtime, adapter, filesystem, network, random-source, or direct-clock implementation was introduced; UUID/time values remain injected. Existing Task 01 code is otherwise unchanged.

**Result:** PRESERVED. Final full workspace nextest ran 15 tests with 15 passed and 0 skipped; fmt, Clippy, deny (including advisories), rustdoc, `git diff --check`, function/column audits, and prohibited-construct audits passed.

## Residue

`01-domain-model.md` §Assumptions leaves open whether sequential `local-conv-<n>` generation is kept indefinitely; this task implements it and leaves the question in `plan.md`. The `letta-msg-` projection mapping itself is Task 28's obligation, not this one's.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: The third, final, clean combined gate independently SATISFIED O1–O6 and found regression PRESERVED. NonEmptyString rejects only empty and preserves whitespace/non-empty bytes; Timestamp accepts UTC `Z` and `+00:00`, rejects malformed/nonzero offsets, emits canonical `Z`, and has no public non-Clock runtime construction; AgentId generation requires UUID v4 while all six generated prefix forms remain entity-correct and accepted opaque IDs are unchanged; RuntimeScope identity/default/acting-user and checked-in-schema behavior are verified. Exact targeted and full workspace gates pass. Correctness verdict: **CORRECT**; completeness verdict: **DONE**.
