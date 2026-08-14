# Task 55 — Provider and tool loop branches and typed stop reasons

**Plan:** [plan.md](../plan.md) · **Certificate:** [55-provider_tool_loop_branches-certificate.md](55-provider_tool_loop_branches-certificate.md)

**Implements:** [03-runtime-and-turns.md §Provider and tool loop](../../../03-runtime-and-turns.md#provider-and-tool-loop)
**Depends on:** 19, 33, 48, 54
**Produces:** the four tool-call branches, context-pressure and retry paths, and the distinct typed stop reasons of the §Provider and tool loop diagram
**Pointers:** `crates/lotta-runtime/src/turn/branches.rs`, `turn/stop_reason.rs`; reference: `../letta-code/src/backend/dev/provider-turn-executor.ts`, `../letta-code/src/websocket/listener/turn.ts`, `../letta-code/src/websocket/listener/turn-events.ts`

## Steps

- [ ] Implement the deny-by-policy branch producing a structured denied tool result
- [ ] Implement the needs-approval branch emitting `control_request` and waiting on the lease
- [ ] Implement the external branch issuing a controller request and waiting
- [ ] Implement the local branch dispatching to the bounded executor and appending the tool result
- [ ] Implement the context-pressure path (compact once, recompile, bounded retry) and the retryable-error path (emit retry, back off, retry) using the Task 48 policy
- [ ] Keep empty response, context overflow, transport failure, provider quota error, and user cancellation as distinct typed stop reasons, and never change the persisted model on fallback

## Definition of done

- [ ] All four tool-call branches are reachable and produce their documented result: denied, approval-pending, external, and local
- [ ] The five stop reasons remain distinct typed values that survive persistence
- [ ] Context pressure compacts once, recompiles, and retries under a bound; a retryable error emits a retry event and backs off using the Task 48 policy
- [ ] Provider fallback never changes the persisted model unless a user command does
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(turn::branches) + test(turn::stop_reasons) + test(turn::context_pressure) + test(turn::retryable_error) + test(turn::fallback_preserves_model)'` and sees every branch and stop reason covered
