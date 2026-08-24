# Task 71 — WebSocket conversation management command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [71-ws_conversation_management_commands-certificate.md](71-ws_conversation_management_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups) · [01-domain-model.md §Conversation](../../../01-domain-model.md#conversation)
**Depends on:** 20, 23, 24, 28, 58
**Produces:** list, retrieve, create, update, recompile, fork, messages, and compact for conversations, including the transcript rewrite that fork requires
**Pointers:** `crates/lotta-app-server/src/ws/groups/conversations.rs`; reference: `../letta-code/src/websocket/listener/commands/agents-conversations.ts`, `../letta-code/src/backend/local/local-store.ts`, `../letta-code/src/backend/local/compaction.ts`

## Steps

- [x] Decode and route the eight commands of the §WebSocket command groups Conversation management row
- [x] Implement fork as a full transcript rewrite with a new conversation key and inherited history
- [x] Implement recompile as a prompt recompilation through Task 30
- [x] Implement compact by delegating to the Task 58 compaction flow under the turn lease
- [x] Serve messages with asc/desc, `before`/`after`, `limit`, and return-message-type filtering
- [x] Enforce `CONVERSATIONS_PER_AGENT_MAX` and emit a conversation update snapshot on mutation

## Definition of done

- [x] All eight conversation management commands decode, route, and respond, with fixture round-trip coverage
- [x] Fork performs a full transcript rewrite into a new conversation key and leaves the source conversation unchanged
- [x] Compact delegates to the Task 58 flow under the turn lease and writes exactly one compaction entry
- [x] Messages honours asc/desc, `before`/`after`, `limit`, and the return-message-type filter
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::)'` and sees eight commands, fork with an unchanged source, lease-serialized compact, and message filtering pass
