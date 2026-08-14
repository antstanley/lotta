# Task 04 — Domain runtime entities and state machines

**Plan:** [plan.md](../plan.md) · **Certificate:** [04-domain_runtime_entities_and_state_machines-certificate.md](04-domain_runtime_entities_and_state_machines-certificate.md)

**Implements:** [01-domain-model.md §Runtime entities](../../../01-domain-model.md#runtime-entities) · [01-domain-model.md §Listener runtime](../../../01-domain-model.md#listener-runtime) · [01-domain-model.md §Connection](../../../01-domain-model.md#connection) · [01-domain-model.md §Conversation runtime](../../../01-domain-model.md#conversation-runtime) · [01-domain-model.md §Turn and lease](../../../01-domain-model.md#turn-and-lease) · [01-domain-model.md §Approval](../../../01-domain-model.md#approval) · [01-domain-model.md §External tool registration](../../../01-domain-model.md#external-tool-registration) · [01-domain-model.md §Queue item](../../../01-domain-model.md#queue-item) · [01-domain-model.md §Relationships](../../../01-domain-model.md#relationships) · [01-domain-model.md §State machines](../../../01-domain-model.md#state-machines) · [01-domain-model.md §Turn lifecycle](../../../01-domain-model.md#turn-lifecycle) · [01-domain-model.md §Input disposition](../../../01-domain-model.md#input-disposition) · [01-domain-model.md §Conversation archival](../../../01-domain-model.md#conversation-archival) · [03-runtime-and-turns.md §Lifecycle owner](../../../03-runtime-and-turns.md#lifecycle-owner) · [canonical-types.schema.json $defs.RuntimeConnection/ConversationRuntimeSnapshot/QueueItem/ApprovalRequest/ExternalToolRegistration](../../../canonical-types.schema.json)
**Depends on:** 02, 03
**Produces:** the four-state turn machine, lease tokens, and runtime projections that make contradictory activity states unrepresentable
**Pointers:** `crates/lotta-domain/src/runtime/`, `crates/lotta-domain/src/runtime/turn_state.rs`, `crates/lotta-domain/src/runtime/queue_item.rs`, `crates/lotta-domain/src/runtime/approval.rs`; reference: `../letta-code/src/websocket/listener/turn-lifecycle.ts`, `../letta-code/src/websocket/listener/conversation-runtime.ts`, `../letta-code/src/types/protocol.ts`

## Steps

- [ ] Define `TurnState` with exactly `Idle`, `Command { lease }`, `Active { lease, turn_id, run_id }`, and `Cancelling { lease, turn_id, run_id }`, matching `03-runtime-and-turns.md` §Lifecycle owner and `$defs.ConversationRuntimeSnapshot.turn_state`
- [ ] Define `TurnLease` as an unforgeable generation token with no public constructor outside the lifecycle owner
- [ ] Derive `is_processing`, `loop_status`, active run IDs, and last stop reason from `TurnState` so no parallel boolean can contradict it
- [ ] Define `InputDisposition` covering prior-disposition replay for a duplicate `client_message_id`, `started`, `queued`, and `rejected`
- [ ] Define `QueueItem` with wire removal dispositions `dequeued`/`cancelled` and internal drop reasons `buffer_limit`/`stale_generation`
- [ ] Define `ApprovalRequest`, `ExternalToolRegistration`, `RuntimeConnection`, and `ConversationRuntimeSnapshot` to their `$defs` shapes, and add transition tests for the turn, disposition, and archival machines

## Definition of done

- [ ] `TurnState` has exactly the four variants `Idle`, `Command`, `Active`, `Cancelling`, and every legal and illegal transition of the `01-domain-model.md` §Turn lifecycle diagram is covered by a test
- [ ] A pending approval leaves the state `Active` rather than introducing a terminal or parallel state, and UI projections derive from `TurnState` alone
- [ ] `InputDisposition` returns the prior disposition for a repeated `client_message_id` instead of executing twice
- [ ] `QueueItem` models both wire removal dispositions and both internal drop reasons with the baseline spellings
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(runtime::)'` and sees the four-state machine, approval-stays-active, duplicate-disposition, and queue wire-name cases pass
