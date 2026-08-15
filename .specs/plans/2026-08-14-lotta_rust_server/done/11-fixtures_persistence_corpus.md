# Task 11 — Persistence fixture corpus extraction

**Plan:** [plan.md](../plan.md) · **Certificate:** [11-fixtures_persistence_corpus-certificate.md](11-fixtures_persistence_corpus-certificate.md)

**Implements:** [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [architecture-principles.md §Workspace layout](../../../architecture-principles.md#workspace-layout) · [04-persistence-and-memfs.md §Migration](../../../04-persistence-and-memfs.md#migration) · [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition)
**Depends on:** 09
**Produces:** a sanitized `fixtures/persistence/` corpus covering the seven cross-runtime cases, checked in before any store code that it constrains
**Pointers:** `fixtures/persistence/`, `tools/extract-persistence-fixtures.mjs`, `crates/lotta-testkit/src/fixtures/persistence.rs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/paths.ts`, `../letta-code/src/backend/local/transcript-migration.ts`

## Steps

- [x] Generate a reference local-backend root with the pinned TypeScript runtime covering both conversation key forms (`base64url("default:" + agent-id)` and `base64url("conversation:" + conversation-id)`)
- [x] Capture the seven `04-persistence-and-memfs.md` §Migration cross-runtime cases as fixture directories: current TypeScript state, Rust-target state, unversioned and versioned-legacy transcripts, baseline-tolerated versioned rows, orphan-result repair input, and corrupt/unsupported manifests
- [x] Capture an interrupted-append and an interrupted-replacement fixture for recovery coverage
- [x] Capture `providers/auth.json` version 1, `settings.json`, `crons.json`, a `runs/<schedule-id>.jsonl`, and a `~/.letta/channels/<channel-id>/` tree as side-store fixtures
- [x] Sanitize every fixture: no credentials, no user transcripts, no provider payloads, per `development-guidelines.md` §Repository hygiene
- [x] Expose the corpus through the testkit loader with a case index that the store tasks assert against

## Definition of done

- [x] All seven `04-persistence-and-memfs.md` §Migration cross-runtime cases are present as named fixture directories, plus the interrupted-append and interrupted-replacement recovery cases
- [x] Both baseline conversation directory key forms appear verbatim in the corpus
- [x] Side-store fixtures cover `settings.json`, `crons.json`, `runs/<schedule-id>.jsonl`, `providers/auth.json` v1, and a channel directory with `config.yaml`, `accounts.json`, `routing.yaml`, `pairing.yaml`, and `targets.json`
- [x] No fixture contains a credential, a real transcript, or an unsanitized provider payload
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::)'` and sees the nine cases, both key forms, every side store, and the sanitization scan pass
