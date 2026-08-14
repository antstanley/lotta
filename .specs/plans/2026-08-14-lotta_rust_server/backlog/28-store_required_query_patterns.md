# Task 28 — Required query patterns and message projections

**Plan:** [plan.md](../plan.md) · **Certificate:** [28-store_required_query_patterns-certificate.md](28-store_required_query_patterns-certificate.md)

**Implements:** [01-domain-model.md §Required query patterns](../../../01-domain-model.md#required-query-patterns) · [01-domain-model.md §Schema definition map](../../../01-domain-model.md#schema-definition-map)
**Depends on:** 23, 24, 25
**Produces:** the ten required query behaviors including the `letta-msg-`/`ui-msg-` projection mapping and deterministic list ordering
**Pointers:** `crates/lotta-store/src/query/mod.rs`, `query/projection.rs`, `query/search.rs`; reference: `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/local-message.ts`, `../letta-code/src/backend/local/transcript-search.ts`

## Steps

- [ ] Implement agent-by-ID with 404 on absent or wrong local prefix, and agents-by-filters with deterministic order and name/query/tag/hidden filters
- [ ] Implement conversations-for-agent with archived/hidden/tag filters, stable ordering, and cursor behavior; resolve conversation-by-ID with agent scope so `default` never crosses agents
- [ ] Implement messages-for-conversation with asc/desc, `before`/`after`, `limit`, and return-message-type filter
- [ ] Implement messages-for-agent merging scoped conversations without losing chronological semantics
- [ ] Implement message-by-projected-ID so every projection key resolves to the same source local message, mapping `letta-msg-` API projections onto `ui-msg-` transcript messages
- [ ] Implement resume-tail, transcript search with agent/conversation filters, and runtime-subscriber listing in stable ordinal order

## Definition of done

- [ ] All ten §Required query patterns have an implementation and a test asserting the stated required behavior
- [ ] Every projection key resolves to the same source local message
- [ ] `default` never crosses agents and a wrong-prefix agent ID returns 404 rather than a lookup miss
- [ ] List ordering is deterministic and cursor behavior is stable across repeated calls with unchanged state
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(query::)'` and sees one passing case per row of `01-domain-model.md` §Required query patterns, including the projection-identity and scope-isolation cases
