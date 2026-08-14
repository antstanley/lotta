# Task 03 · Domain persistent entities — done certificate

**Task:** [03-domain_persistent_entities.md](03-domain_persistent_entities.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-14 — final independent combined gate passed; done

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

- **O1 — Every task-owned entity round-trips through its `canonical-types.schema.json` `$defs` shape with the exact required-field set**
  - *Claim:* For each of the 14 entity `$defs`, a serialized value validates against the schema and deserializes back to an equal value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::schema_conformance)'` — expect one passing case for each of the 14 task-owned `$defs` names and no skipped entity. Read the test and confirm it loads each named definition from `.specs/canonical-types.schema.json` at test time rather than copying its constraints into Rust; assert the tested-name set equals the task's 14-name entity contract.
  - *Final-gate fresh evidence (2026-08-14):* The exact selector ran one nonzero case and passed: `entities::schema_conformance::all_fourteen_minimal_and_complete_pairs`. `schema_conformance.rs:7-22` maintains the exact 14-name contract. Its fixture macro records each literal name and proves set equality at `:92-99,332`; `check` at `:44-75` derives live required/property sets from the checked-in schema, requires independently authored minimal/complete key equality, validates both samples against the real self-contained `$defs`, deserializes each into the exact concrete Rust type, serializes, revalidates, re-deserializes, and compares semantic typed equality. All 28 fixtures passed. `tests.rs:726-801` separately drives each of the 16 presence-aware fields through missing/null/value on its owning type and verifies deserialize→serialize preservation: MemoryBlockInput 1, Agent 3, Conversation 4, Run 3, Schedule 4, and ChannelRoute 1, for 48 real state cases. The targeted selector ran this test and passed.
  - *Status:* ☒ SATISFIED.

- **O2 — `Schedule` carries all 25 canonical fields with the exact 19-required/6-optional split, including IANA `timezone`, `jitter_offset_ms`, the four-state lifecycle, fire/miss/fail counters, `cancel_reason`, and the one-shot timestamps**
  - *Claim:* `Schedule` models all 25 names in `$defs.Schedule.properties`; its serialized form always contains the 19 names in `$defs.Schedule.required`, its six remaining properties follow the schema's optional/null semantics, and `status` accepts exactly `active`, `fired`, `missed`, `cancelled`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::schedule)'` — expect `all_canonical_fields_and_required_split` and `status_enum_is_exhaustive` to pass. Read the test and confirm it derives the 25-property and 19-required sets from the checked-in schema. Read `crates/lotta-domain/src/entities/schedule.rs` and confirm `timezone` is a validated IANA identifier type, not a bare `String`, and that `jitter_offset_ms` is a signed value so the baseline's negative one-shot offset (`../letta-code/src/cron/cron-file.ts` `computeJitter`) is representable.
  - *Checks:* Resolve the `jitter` field used by this entity — confirm it is the schedule's own `jitter_offset_ms`, not any retry-backoff jitter. `06-model-providers.md` §Retry and fallback forbids jitter in the provider-turn retry path; the two must not share a type or a constant.
  - *Final-gate fresh evidence (2026-08-14):* The exact selector ran two nonzero tests and passed both. `schedule.rs:96-171` models all 25 properties with the schema's exact 19-required/6-optional split. The independently authored minimal/complete schema fixtures prove both live key sets, including all six optionals. `IanaTimezone` privately wraps `chrono_tz::Tz`, accepts real IANA names and rejects abbreviations/unknown zones; `jitter_offset_ms` is signed `i64`. Status accepts exactly active/fired/missed/cancelled; cancel reasons exactly conversation_not_found/expired; outcomes exactly queued/missed/failed/skipped; counters are unsigned; required nullable and one-shot timestamps match the schema. The presence test independently passes missing/null/value deserialize→serialize checks for all four Schedule presence-aware fields. Source search found schedule jitter only and no provider retry coupling.
  - *Status:* ☒ SATISFIED.

- **O3 — `ChannelAccount` and `ChannelRoute` serialize snake_case for `accounts.json` and camelCase for runtime projections, with `group_policy`, `admin_users`, and `user_allowed_commands` present**
  - *Claim:* The same account value serializes with snake_case keys through the canonical writer and camelCase keys through the runtime projection.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::channel)'` — expect `accounts_json_is_snake_case` and `runtime_projection_is_camel_case` to pass. Read the test and confirm both assertions inspect key names, not only round-trip equality.
  - *Final-gate fresh evidence (2026-08-14):* The exact selector ran one nonzero test and passed. `tests.rs:529-588` derives both complete canonical property sets from the live schema, compares them to complete canonical serializations, independently converts every schema key to camelCase, and compares both complete runtime key sets. `channel.rs:40-196` explicitly projects every account and route field, including display/group/admin/command policy fields and all route optionals. Canonical serialization is snake_case; runtime serialization is camelCase; `thread_id` separately passes missing/null/value owning-type round trips.
  - *Status:* ☒ SATISFIED.

- **O4 — Unknown compatible fields on `Agent` and `Conversation` survive a read-modify-write cycle**
  - *Claim:* Deserializing an agent JSON carrying an unrecognized key, mutating one known field, and reserializing preserves the unrecognized key and its value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(entities::preserves_unknown_fields)'` — expect PASS. Trace the case: input `{"id":…, "future_flag":true}` → deserialize → set `name` → serialize → assert `future_flag` is still `true`.
  - *Final-gate fresh evidence (2026-08-14):* The exact selector ran one nonzero test and passed. Agent and Conversation flatten private, deterministic `EntityExtras`; read-modify-write preserves scalar and nested future fields (`tests.rs:442-461`). Private `BoundedMap` storage, safe constructors, and streaming deserialization enforce the 128-field direct bound plus recursive JSON array/object/depth limits. `EntityExtras::new`, `append_to`, and `serialize_with_extras` reject canonical-field collisions with typed errors; production source contains no panic/assert/unwrap/expect/unsafe/direct-clock paths.
  - *Status:* ☒ SATISFIED.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Final-gate fresh evidence (2026-08-14):* All repository gates passed: `cargo fmt --all --check`; workspace Clippy with `-D warnings`; full workspace nextest 48/48; `cargo deny check`; rustdoc with `RUSTDOCFLAGS='-D warnings'`; changed-file whitespace audit; <=100-column scan; and <=70-line production-function audit. Production entity source is clean of panic/assert/unwrap/expect/todo/unimplemented/unsafe/direct-clock use. Every retained collection/map uses private bounded storage and named units-last constants. `BoundedVec`, `BoundedMap`, and `BoundedJsonValue` cap allocation hints, reject MAX+1 before retaining it, and check depth before descending. Safe constructors validate prebuilt values. Transparent serde preserves canonical JSON. `LocalMessage.content` is now `Option<BoundedJsonValue>`. Targeted tests pass below/at/above owning-entity deserialization for direct lists/maps/extras and LocalMessage nested arrays/objects/depth, plus constructor and streaming paths. Dependency audit confirms only necessary additions: `serde_json` for production bounded JSON/extras serialization and `chrono-tz` for validated IANA timezones; entity modules depend only inward on `lotta-domain` IDs/scalars.
  - *Status:* ☒ SATISFIED.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(entities::)'` and sees schema conformance, the 25-field Schedule, dual-case channel records, and unknown-field preservation pass**
  - *Claim:* The `entities::` module tests pass with a case for every `$defs` entity.
  - *Evidence to collect:* Run the filter and confirm the summary reports at least 14 schema-conformance cases and zero failures.
  - *Final-gate fresh evidence (2026-08-14):* The exact review selector ran 33 tests and passed all 33 with zero failures. It includes the independently authored 14-pair schema case, 14 individual typed schema round trips, full Schedule checks, complete channel key sets, extras preservation, all 48 presence cases, transcript names/union behavior, and direct/nested bound checks. Every narrower certificate selector had a nonzero intended count and passed: schema 1, schedule 2, channel 1, extras 1; the targeted presence/bounds selector ran 4/4.
  - *Status:* ☒ SATISFIED.

## Regression check

- `AgentId` and `ConversationId` remain embedded throughout the entities; `cargo nextest run --manifest-path /Volumes/Delorean/code/five-letters/lotta-workspaces/task-03/Cargo.toml -p lotta-domain -E 'test(ids::) | test(entities::)'` ran 40 tests and passed all 40: ☒ PRESERVED

## Residue

Persisted encoding of these entities to disk is Tasks 23–24; this task owns only the in-memory type and its serde shape. Transcript entry loading tolerances are Task 25.

## Conclusion

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Correctness verdict **CORRECT**. Completeness verdict **DONE**. O1–O6 are SATISFIED and regression is PRESERVED. The final independent gate found no remaining remediation: every certificate selector was nonzero and green; entity selector 33/33, targeted presence/bounds 4/4, workspace 48/48, Task 02+03 40/40, fmt, Clippy, deny, rustdoc, whitespace, dependency, forbidden-pattern, line-width, and function-size audits all passed.
