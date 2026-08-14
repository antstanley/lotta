# Done Certificate — Task 03: Domain persistent entities and schema conformance

**Task:** [03-domain_persistent_entities.md](03-domain_persistent_entities.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 03. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 03) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** every persisted entity as a Rust type that round-trips through the exact canonical-types schema shape, including the 25-field Schedule and the snake_case channel records.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add, rename, or drop a field relative to `canonical-types.schema.json`; `development-guidelines.md` §Guidelines for AI agents forbids inventing wire fields without updating the schema first.

## Obligations

- **O1 — Every entity round-trips through its `canonical-types.schema.json` `$defs` shape with the exact required-field set**
  - *Claim:* For each of the 14 entity `$defs`, a serialized value validates against the schema and deserializes back to an equal value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::schema_conformance)'` — expect one passing case per `$defs` entity and no skipped entity. Read the test's entity list and confirm it is derived by reading `.specs/canonical-types.schema.json` at test time, so a new `$defs` entry fails the test rather than being silently unlisted.
  - *Status:* ☐ unverified

- **O2 — `Schedule` carries all 25 required fields including IANA `timezone`, `jitter_offset_ms`, the four-state lifecycle, fire/miss counters, `cancel_reason`, and the one-shot timestamps**
  - *Claim:* `Schedule`'s serialized form contains every name in the `$defs.Schedule` `required` array, and `status` accepts exactly `active`, `fired`, `missed`, `cancelled`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::schedule)'` — expect `all_required_fields_present` and `status_enum_is_exhaustive` to pass. Read `crates/lotta-domain/src/entities/schedule.rs` and confirm `timezone` is a validated IANA identifier type, not a bare `String`, and that `jitter_offset_ms` is a signed value so the baseline's negative one-shot offset (`../letta-code/src/cron/cron-file.ts` `computeJitter`) is representable.
  - *Checks:* Resolve the `jitter` field used by this entity — confirm it is the schedule's own `jitter_offset_ms`, not any retry-backoff jitter. `06-model-providers.md` §Retry and fallback forbids jitter in the provider-turn retry path; the two must not share a type or a constant.
  - *Status:* ☐ unverified

- **O3 — `ChannelAccount` and `ChannelRoute` serialize snake_case for `accounts.json` and camelCase for runtime projections, with `group_policy`, `admin_users`, and `user_allowed_commands` present**
  - *Claim:* The same account value serializes with snake_case keys through the canonical writer and camelCase keys through the runtime projection.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::channel)'` — expect `accounts_json_is_snake_case` and `runtime_projection_is_camel_case` to pass. Read the test and confirm both assertions inspect key names, not only round-trip equality.
  - *Status:* ☐ unverified

- **O4 — Unknown compatible fields on `Agent` and `Conversation` survive a read-modify-write cycle**
  - *Claim:* Deserializing an agent JSON carrying an unrecognized key, mutating one known field, and reserializing preserves the unrecognized key and its value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::preserves_unknown_fields)'` — expect PASS. Trace the case: input `{"id":…,"future_flag":true}` → deserialize → set `name` → serialize → assert `future_flag` is still `true`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(entities::)'` and sees schema conformance, the 25-field Schedule, dual-case channel records, and unknown-field preservation pass**
  - *Claim:* The `entities::` module tests pass with a case for every `$defs` entity.
  - *Evidence to collect:* Run the filter and confirm the summary reports at least 14 schema-conformance cases and zero failures.
  - *Status:* ☐ unverified

## Regression check

- `AgentId` and `ConversationId` from Task 02 are embedded by the new entity constructors; run `cargo nextest run -p lotta-domain -E 'test(ids::) | test(entities::)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

Persisted encoding of these entities to disk is Tasks 23–24; this task owns only the in-memory type and its serde shape. Transcript entry loading tolerances are Task 25.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
