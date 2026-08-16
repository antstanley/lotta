# Task 31 — Cross-runtime persistence conformance

**Plan:** [plan.md](../plan.md) · **Certificate:** [31-cross_runtime_persistence_conformance-certificate.md](31-cross_runtime_persistence_conformance-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [04-persistence-and-memfs.md §Migration](../../../04-persistence-and-memfs.md#migration) · [04-persistence-and-memfs.md §State outside the backend root](../../../04-persistence-and-memfs.md#state-outside-the-backend-root) · [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance)
**Depends on:** 11, 23, 24, 25, 26, 27, 30
**Produces:** proof that TypeScript and Rust round-trip the backend root, provider auth, schedules, settings, and channel side stores in both directions without loss
**Pointers:** `tests/conformance/cross_runtime.rs`, `tests/conformance/harness/ts_runner.mjs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/local-backend.ts`

## Steps

- [x] Build a harness that runs the pinned TypeScript local backend against a temporary root and hands it off to the Rust store, and the reverse
- [x] Cover the backend root: agent JSON, conversation JSON, transcript JSONL, manifest, and `system-prompt.json`
- [x] Cover the side stores: `providers/auth.json`, `crons.json`, `runs/<schedule-id>.jsonl`, `settings.json`, and the channel tree
- [x] Cover all seven `04-persistence-and-memfs.md` §Migration fixture cases in both directions
- [x] Assert quiesced sequential handoff rather than concurrent mixed-runtime writes
- [x] Report any field lost in either direction with the path and the field name

## Definition of done

- [x] Every backend-root artifact round-trips in both directions with no lost field
- [x] Every side store round-trips in both directions, including `providers/auth.json` v1 and a channel tree
- [x] All seven §Migration cross-runtime cases pass, including corrupt/unsupported manifests rejected without mutation
- [x] The harness performs a quiesced sequential handoff and fails loudly if both runtimes would write concurrently
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run --test cross_runtime` and sees all backend-root, side-store, and migration cases pass in both directions
