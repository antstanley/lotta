# Task 27 — State outside the backend root

**Plan:** [plan.md](../plan.md) · **Certificate:** [27-store_side_stores-certificate.md](27-store_side_stores-certificate.md)

**Implements:** [04-persistence-and-memfs.md §State outside the backend root](../../../04-persistence-and-memfs.md#state-outside-the-backend-root)
**Depends on:** 11, 22
**Produces:** read/write access to `settings.json`, `crons.json`, schedule run logs, the channel store tree, and project-local settings at their exact baseline paths
**Pointers:** `crates/lotta-store/src/side/settings.rs`, `side/crons.rs`, `side/run_logs.rs`, `side/channels.rs`, `side/project.rs`; reference: `../letta-code/src/settings.ts`, `../letta-code/src/cron/cron-file.ts`, `../letta-code/src/cron/run-log.ts`, `../letta-code/src/channels/config.ts`, `../letta-code/src/channels/accounts.ts`

## Steps

- [ ] Resolve `~/.letta/settings.json`, `${LETTA_HOME:-~/.letta}/crons.json`, `${LETTA_HOME:-~/.letta}/runs/<schedule-id>.jsonl`, and `~/.letta/channels/` exactly as `04-persistence-and-memfs.md` §State outside the backend root specifies
- [ ] Read and write the channel tree: `pending-control-requests.json` and per-channel `config.yaml`, `accounts.json`, `routing.yaml`, `pairing.yaml`, `targets.json`
- [ ] Treat plugin-owned files below a channel directory as opaque but included
- [ ] Resolve `<workspace>/.letta/settings.json` and `settings.local.json`, included only when their workspace is in scope
- [ ] Route every side-store write through the Task 22 atomic writer
- [ ] Round-trip each path against its `fixtures/persistence/` side-store fixture

## Definition of done

- [ ] Every path named in §State outside the backend root resolves and round-trips against its fixture
- [ ] Plugin-owned files below a channel directory are preserved opaquely across a read-modify-write of a known file
- [ ] Project-local settings are read only when their workspace is in scope, and never merged from an out-of-scope workspace
- [ ] Every side-store write goes through the atomic writer, so an external change yields `storage_conflict` rather than a silent overwrite
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(side::)'` and sees every side-store path round-trip, plugin files preserved, project scope gating, and atomic writes pass
