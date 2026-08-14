# Task 03 — Domain persistent entities and schema conformance

**Plan:** [plan.md](../plan.md) · **Certificate:** [03-domain_persistent_entities-certificate.md](03-domain_persistent_entities-certificate.md)

**Implements:** [01-domain-model.md §Persistent entities](../../../01-domain-model.md#persistent-entities) · [01-domain-model.md §Agent](../../../01-domain-model.md#agent) · [01-domain-model.md §Conversation](../../../01-domain-model.md#conversation) · [01-domain-model.md §Transcript entry](../../../01-domain-model.md#transcript-entry) · [01-domain-model.md §Provider connection](../../../01-domain-model.md#provider-connection) · [01-domain-model.md §Run](../../../01-domain-model.md#run) · [01-domain-model.md §Schedule](../../../01-domain-model.md#schedule) · [01-domain-model.md §Channel account and route](../../../01-domain-model.md#channel-account-and-route) · [canonical-types.schema.json $defs.Agent/Conversation/LocalMessage/TranscriptManifest/SessionEntry/MessageEntry/CompactionEntry/ProviderConnection/ModelDescriptor/Run/Schedule/ChannelAccount/ChannelRoute/MemoryBlockInput](../../../canonical-types.schema.json)
**Depends on:** 02
**Produces:** every persisted entity as a Rust type that round-trips through the exact canonical-types schema shape, including the 25-field Schedule and the snake_case channel records
**Pointers:** `crates/lotta-domain/src/entities/`, `crates/lotta-domain/src/entities/schedule.rs`, `crates/lotta-domain/src/entities/channel.rs`, `crates/lotta-domain/src/entities/transcript.rs`; reference: `../letta-code/src/backend/local/local-agent-record.ts`, `../letta-code/src/backend/local/local-message.ts`, `../letta-code/src/cron/cron-file.ts`, `../letta-code/src/channels/accounts.ts`

## Steps

- [ ] Define `Agent`, `Conversation`, `Run`, `LocalMessage`, `ProviderConnection`, and `ModelDescriptor` with the exact required/optional field split of `canonical-types.schema.json`
- [ ] Define `TranscriptManifest`, `SessionEntry`, `MessageEntry`, `CompactionEntry`, and the `TranscriptEntry` union with camelCase wire names (`parentId`, `firstKeptEntryId`, `tokensBefore`)
- [ ] Define `Schedule` with all 25 required fields including `timezone`, `jitter_offset_ms`, `cancel_reason`, the `active`/`fired`/`missed`/`cancelled` status, fire/miss/fail counters, and the one-shot `scheduled_for`/`fired_at`/`missed_at` timestamps
- [ ] Define `ChannelAccount` and `ChannelRoute` with runtime camelCase projections and canonical snake_case `accounts.json` serialization, including `group_policy`, `admin_users`, and `user_allowed_commands`
- [ ] Preserve unknown compatible fields on `Agent` and `Conversation` in a captured extras map so read-modify-write cannot drop a newer reference runtime's data
- [ ] Add a schema-conformance test that validates each entity's serialized form against its `$defs` entry rather than a hand-written literal

## Definition of done

- [ ] Every entity round-trips through its `canonical-types.schema.json` `$defs` shape with the exact required-field set
- [ ] `Schedule` carries all 25 required fields including IANA `timezone`, `jitter_offset_ms`, the four-state lifecycle, fire/miss counters, `cancel_reason`, and the one-shot timestamps
- [ ] `ChannelAccount` and `ChannelRoute` serialize snake_case for `accounts.json` and camelCase for runtime projections, with `group_policy`, `admin_users`, and `user_allowed_commands` present
- [ ] Unknown compatible fields on `Agent` and `Conversation` survive a read-modify-write cycle
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(entities::)'` and sees schema conformance, the 25-field Schedule, dual-case channel records, and unknown-field preservation pass
