# Task 02 — Domain IDs, scalar newtypes, and runtime scope

**Plan:** [plan.md](../plan.md) · **Certificate:** [02-domain_ids_and_scalars-certificate.md](02-domain_ids_and_scalars-certificate.md)

**Implements:** [01-domain-model.md §ID scheme](../../../01-domain-model.md#id-scheme) · [01-domain-model.md §Schema definition map](../../../01-domain-model.md#schema-definition-map) · [architecture-principles.md §IDs](../../../architecture-principles.md#ids) · [architecture-principles.md §Time](../../../architecture-principles.md#time) · [canonical-types.schema.json $defs.NonEmptyString/Timestamp/AgentId/ConversationId/MessageId/RunId/RuntimeScope](../../../canonical-types.schema.json)
**Depends on:** 01
**Produces:** typed, opaque ID newtypes and validated scalars that accept every baseline ID form and never rewrite a client-supplied ID
**Pointers:** `crates/lotta-domain/src/ids.rs`, `crates/lotta-domain/src/scalars.rs`, `crates/lotta-domain/src/scope.rs`; reference: `../letta-code/src/backend/local/paths.ts`, `../letta-code/src/websocket/listener/scope.ts`, `../letta-code/src/types/protocol_v2.ts`

## Steps

- [ ] Define `AgentId`, `ConversationId`, `MessageId`, `RunId` newtypes that serialize as opaque strings
- [ ] Implement generation-side validation for the six prefixes in `01-domain-model.md` §ID scheme (`agent-local-`, `local-conv-`, `letta-msg-`, `ui-msg-`, `local-run-`, `resp_letta_`) and input-side acceptance of any non-empty conversation ID
- [ ] Model the virtual `default` conversation ID as agent-scoped, so it is never globally unique
- [ ] Define `RuntimeScope { agent_id, conversation_id, acting_user_id: Option<_> }` as the runtime key struct
- [ ] Define `NonEmptyString` and RFC3339 `Timestamp` newtypes with a `Clock`-sourced constructor and no wall-clock access
- [ ] Add proptests covering valid generation forms, rejected generation forms, and accepted arbitrary input IDs

## Definition of done

- [ ] Generated IDs are validated against the six `01-domain-model.md` §ID scheme prefixes, while arbitrary non-empty conversation IDs are accepted on input and returned byte-identical
- [ ] `default` is agent-scoped: two `RuntimeScope` values sharing `conversation_id = "default"` but differing in `agent_id` are distinct keys
- [ ] `RuntimeScope` carries optional `acting_user_id` and serializes to the `canonical-types.schema.json` `$defs.RuntimeScope` shape
- [ ] `Timestamp` is RFC3339 UTC, constructed only through a `Clock` value, and `NonEmptyString` rejects the empty string
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(ids::) + test(scope::) + test(scalars::)'` and sees generation validation, arbitrary-ID acceptance, agent-scoped `default`, and clock-free timestamps pass
