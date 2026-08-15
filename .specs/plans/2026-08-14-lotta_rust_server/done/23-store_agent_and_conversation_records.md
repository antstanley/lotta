# Task 23 — Agent and conversation record persistence

**Plan:** [plan.md](../plan.md) · **Certificate:** [23-store_agent_and_conversation_records-certificate.md](23-store_agent_and_conversation_records-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Agent and conversation records](../../../04-persistence-and-memfs.md#agent-and-conversation-records) · [01-domain-model.md §Agent](../../../01-domain-model.md#agent) · [01-domain-model.md §Conversation](../../../01-domain-model.md#conversation) · [01-domain-model.md §Conversation archival](../../../01-domain-model.md#conversation-archival)
**Depends on:** 03, 11, 22
**Produces:** agent JSON and conversation snapshots that round-trip against the checked-in corpus, preserve unknown fields, and refresh on external mtime change
**Pointers:** `crates/lotta-store/src/agent.rs`, `src/conversation.rs`, `src/refresh.rs`; reference: `../letta-code/src/backend/local/local-agent-record.ts`, `../letta-code/src/backend/local/local-store.ts`

## Steps

- [x] Read and write normalized `LocalAgentRecord` JSON at `agents/<base64url(agent-id)>.json`
- [x] Preserve unknown compatible fields across read-modify-write so a newer reference runtime loses nothing
- [x] Write `conversation.json` as a formatted snapshot and keep `in_context_message_ids` authoritative
- [x] Poll record mtimes and refresh loaded agent/conversation snapshots when another process writes them
- [x] Implement archive/unarchive setting and clearing `archived_at`, and conversation-level model override without mutating the agent
- [x] Enforce `AGENTS_MAX` and `CONVERSATIONS_PER_AGENT_MAX` with below/at/above tests

## Definition of done

- [x] Every agent and conversation fixture in `fixtures/persistence/` reads, re-serializes, and compares equal to its source
- [x] Unknown compatible fields survive read-modify-write on both record types
- [x] An external write is detected by mtime and refreshes the loaded snapshot
- [x] Archiving sets `archived_at`, unarchiving clears it, a conversation model override never mutates the agent, and `AGENTS_MAX`/`CONVERSATIONS_PER_AGENT_MAX` reject at the limit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(agent::) + test(conversation::) + test(refresh::)'` and sees corpus round-trips, unknown-field preservation, external-write refresh, archival, and creation caps pass
