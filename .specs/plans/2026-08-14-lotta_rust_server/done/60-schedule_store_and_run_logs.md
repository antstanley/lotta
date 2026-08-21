# Task 60 — Schedule store, cron parsing, and run logs

**Plan:** [plan.md](../plan.md) · **Certificate:** [60-schedule_store_and_run_logs-certificate.md](60-schedule_store_and_run_logs-certificate.md)

**Implements:** [01-domain-model.md §Schedule](../../../01-domain-model.md#schedule) · [canonical-types.schema.json $defs.Schedule](../../../canonical-types.schema.json)
**Depends on:** 27
**Produces:** `crons.json` persistence for the full Schedule shape with cron and interval parsing, IANA timezones, and rotated per-schedule run logs
**Pointers:** `crates/lotta-runtime/src/schedule/store.rs`, `schedule/cron.rs`, `schedule/run_log.rs`; reference: `../letta-code/src/cron/cron-file.ts`, `../letta-code/src/cron/parse-interval.ts`, `../letta-code/src/cron/run-log.ts`, `../letta-code/src/types/schedule-protocol.ts`

## Steps

- [x] Persist and load the full `$defs.Schedule` shape from `${LETTA_HOME:-~/.letta}/crons.json` through the Task 27 side store
- [x] Parse cron expressions and the baseline interval forms, resolving next-fire times in the schedule's IANA timezone
- [x] Compute `jitter_offset_ms` with the baseline rules: late jitter bounded by min(10% of period, 15 min) and under one tick for recurring, early jitter up to 90 s for one-shots at :00/:30, and none otherwise
- [x] Maintain the `active`/`fired`/`missed`/`cancelled` lifecycle with fire, miss, and fail counters and `cancel_reason`
- [x] Append run history to `runs/<schedule-id>.jsonl` and rotate at `SCHEDULE_RUN_LOG_KEEP_LINES` and `SCHEDULE_RUN_LOG_BYTES_MAX`
- [x] Round-trip the `fixtures/persistence/` `crons.json` and run-log fixtures

## Definition of done

- [x] The full `$defs.Schedule` shape persists and loads from `crons.json`, round-tripping the fixture without losing a field
- [x] Cron expressions and the baseline interval forms both parse, and next-fire times resolve in the schedule's IANA timezone including a DST transition
- [x] `jitter_offset_ms` follows the baseline rules for recurring, one-shot at :00/:30, and other one-shots
- [x] The four-state lifecycle, fire/miss/fail counters, and `cancel_reason` behave as `01-domain-model.md` §Schedule specifies
- [x] Run history appends to `runs/<schedule-id>.jsonl` and rotates at both bounds
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(schedule::)'` and sees the 25-field round trip, cron and interval parsing across DST, baseline jitter offsets, the four-state lifecycle, and run-log rotation pass
