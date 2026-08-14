# Done Certificate — Task 02: Domain IDs, scalar newtypes, and runtime scope

**Task:** [02-domain_ids_and_scalars.md](02-domain_ids_and_scalars.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Status:* ☐ unverified

- **O2 — `default` is agent-scoped: two `RuntimeScope` values sharing `conversation_id = "default"` but differing in `agent_id` are distinct keys**
  - *Claim:* `RuntimeScope` hashes and compares on the `(agent_id, conversation_id)` pair, so `default` never collides across agents.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(scope::default_is_agent_scoped)'` — expect PASS, and read the test to confirm it inserts two scopes with the same `default` conversation and different agents into a `HashMap` and asserts `len() == 2`.
  - *Status:* ☐ unverified

- **O3 — `RuntimeScope` carries optional `acting_user_id` and serializes to the `canonical-types.schema.json` `$defs.RuntimeScope` shape**
  - *Claim:* Serializing a `RuntimeScope` yields exactly `agent_id`, `conversation_id`, and (when present) `acting_user_id`, with the first two required.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(scope::schema_shape)'` — expect PASS. Read the test and confirm it validates the serialized JSON against `$defs.RuntimeScope` in `.specs/canonical-types.schema.json` rather than against a hand-written literal.
  - *Status:* ☐ unverified

- **O4 — `Timestamp` is RFC3339 UTC, constructed only through a `Clock` value, and `NonEmptyString` rejects the empty string**
  - *Claim:* There is no constructor on `Timestamp` that reads the system clock, and `NonEmptyString::new("")` returns an error.
  - *Evidence to collect:* Grep `crates/lotta-domain/src/` for `SystemTime::now`, `Utc::now`, and `Instant::now` — expect zero matches. Run `cargo nextest run -p lotta-domain -E 'test(scalars::)'` — expect `rejects_empty` and `rfc3339_round_trip` to pass.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(ids::) + test(scope::) + test(scalars::)'` and sees generation validation, arbitrary-ID acceptance, agent-scoped `default`, and clock-free timestamps pass**
  - *Claim:* The three filtered test modules pass with no ignored cases.
  - *Evidence to collect:* Run the named filter and confirm a non-zero test count with zero failures and zero skips in the nextest summary.
  - *Status:* ☐ unverified

## Regression check

No existing callers in scope — this task introduces new units that nothing else calls yet, and modifies no unit produced by a task it depends on.

## Residue

`01-domain-model.md` §Assumptions leaves open whether sequential `local-conv-<n>` generation is kept indefinitely; this task implements it and leaves the question in `plan.md`. The `letta-msg-` projection mapping itself is Task 28's obligation, not this one's.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
